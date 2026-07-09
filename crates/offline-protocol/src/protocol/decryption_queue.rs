//! Bounded pending decryption queue for encrypted messages received before
//! the MLS session is ready, with per-peer and global limits, TTL expiration,
//! and configurable overflow policies.

use crate::config::{OverflowPolicy, PendingQueueConfig};
use offline_protocol_core::Message;
use std::collections::{HashMap, HashSet, VecDeque};
use std::time::{Duration as StdDuration, Instant};
use tracing::{debug, warn};

const TTL_SPIKE_WARN_THRESHOLD: usize = 25;
const PEER_PRESSURE_WARN_EVERY: u32 = 10;
const DROP_WARN_EVERY: u64 = 100;
const EVICTION_FAILURE_WARN_EVERY: u64 = 10;

#[derive(Clone)]
pub(crate) struct PendingDecryptMessage {
    pub(crate) peer_id: String,
    pub(crate) message_id: String,
    pub(crate) received_at: Instant,
    pub(crate) sequence: u64,
    pub(crate) message: Message,
}

#[derive(Clone)]
struct EntryRef {
    peer_id: String,
    message_id: String,
    sequence: u64,
}

/// A message dropped from the pending queue (overflow or TTL expiry),
/// together with the machine-readable drop reason. Returned to the protocol
/// layer so it can surface user-visible consequences — in particular an
/// encrypted media chunk that was already ACKed and dedup-marked on receipt
/// and therefore can never be recovered.
pub(crate) struct DroppedPendingMessage {
    pub(crate) message: Message,
    pub(crate) reason: &'static str,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum QueueLimit {
    PerPeer,
    Global,
    PerPeerBytes,
    GlobalBytes,
}

impl QueueLimit {
    fn as_str(self) -> &'static str {
        match self {
            Self::PerPeer => "per_peer",
            Self::Global => "global",
            Self::PerPeerBytes => "per_peer_bytes",
            Self::GlobalBytes => "global_bytes",
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum DropReason {
    OverflowDropOldest,
    OverflowDropNewest,
    TtlExpired,
}

impl DropReason {
    fn as_str(self) -> &'static str {
        match self {
            Self::OverflowDropOldest => "overflow_drop_oldest",
            Self::OverflowDropNewest => "overflow_drop_newest",
            Self::TtlExpired => "ttl_expired",
        }
    }
}

/// Counters and gauges for pending encrypted message queue pressure.
#[derive(Debug, Clone, Default)]
pub struct PendingQueueMetrics {
    /// Total encrypted messages received before session readiness.
    pub pending_messages_received_total: u64,
    /// Total queued messages evicted from pending storage.
    pub pending_messages_evicted_total: u64,
    /// Total messages dropped due to overflow policy decisions.
    pub pending_messages_dropped_overflow_total: u64,
    /// Total pending messages expired due to TTL.
    pub pending_messages_expired_total: u64,
    /// Number of failed eviction attempts while enforcing hard bounds.
    pub pending_messages_eviction_failures_total: u64,
    /// Number of detected pending queue invariant violations.
    pub pending_queue_invariant_violations_total: u64,
    /// Current number of messages in pending queues across all peers.
    pub pending_messages_current: usize,
    /// Current payload bytes (content plus binary content) queued across all peers.
    pub pending_bytes_current: usize,
    /// Current per-peer pending queue sizes.
    pub pending_messages_per_peer: HashMap<String, usize>,
}

/// Encapsulates the bounded pending decryption queue: encrypted messages
/// received before the MLS session is ready, with per-peer and global limits,
/// TTL expiration, and configurable overflow policies.
#[derive(Default)]
pub(crate) struct PendingDecryptionQueue {
    /// Per-peer FIFO queues of encrypted messages.
    queues: HashMap<String, VecDeque<PendingDecryptMessage>>,
    /// Global insertion order index for deterministic global oldest eviction.
    global_order: VecDeque<EntryRef>,
    /// Live sequence IDs currently present in pending queues.
    live_sequences: HashSet<u64>,
    /// Current number of pending encrypted messages across all peers.
    total: usize,
    /// Current payload bytes queued across all peers.
    total_bytes: usize,
    /// Current payload bytes queued per peer.
    peer_bytes: HashMap<String, usize>,
    /// Monotonic sequence assigned on enqueue for deterministic tie-breaking.
    next_seq: u64,
    /// Pending queue observability counters and gauges.
    metrics: PendingQueueMetrics,
    /// Overflow hit count per peer for warning signal emission.
    peer_overflow_hits: HashMap<String, u32>,
    /// Drop warning counters used for log-rate limiting by reason/limit.
    drop_warning_counters: HashMap<String, u64>,
}

impl PendingDecryptionQueue {
    // ---- Public accessors ----

    /// Returns a reference to the current metrics snapshot.
    pub(crate) fn metrics(&self) -> &PendingQueueMetrics {
        &self.metrics
    }

    /// Returns true when no peers have pending messages.
    #[cfg(test)]
    pub(crate) fn is_empty(&self) -> bool {
        self.queues.is_empty()
    }

    /// Returns true when the given peer has queued messages.
    #[cfg(test)]
    pub(crate) fn contains_peer(&self, peer_id: &str) -> bool {
        self.queues.contains_key(peer_id)
    }

    /// Returns the number of pending messages for a specific peer.
    #[cfg(test)]
    pub(crate) fn peer_queue_len(&self, peer_id: &str) -> usize {
        self.queues.get(peer_id).map(VecDeque::len).unwrap_or(0)
    }

    /// Returns the total number of pending messages across all peers.
    #[cfg(test)]
    pub(crate) fn total(&self) -> usize {
        self.total
    }

    /// Returns the total payload bytes queued across all peers.
    #[cfg(test)]
    pub(crate) fn total_bytes(&self) -> usize {
        self.total_bytes
    }

    /// Returns a reference to a specific entry in a peer's queue.
    #[cfg(test)]
    pub(crate) fn peek_entry(&self, peer_id: &str, index: usize) -> Option<&PendingDecryptMessage> {
        self.queues.get(peer_id).and_then(|q| q.get(index))
    }

    /// Returns the maximum queue length across all peers.
    #[cfg(test)]
    pub(crate) fn max_peer_queue_len(&self) -> usize {
        self.queues.values().map(VecDeque::len).max().unwrap_or(0)
    }

    /// Overrides the `received_at` timestamp of the front entry for a peer (fault injection).
    #[cfg(test)]
    pub(crate) fn set_front_received_at(&mut self, peer_id: &str, at: Instant) {
        if let Some(queue) = self.queues.get_mut(peer_id) {
            if let Some(front) = queue.front_mut() {
                front.received_at = at;
            }
        }
    }

    /// Clears the global order index to simulate index corruption (fault injection).
    #[cfg(test)]
    pub(crate) fn corrupt_clear_global_order(&mut self) {
        self.global_order.clear();
    }

    // ---- Internal helpers ----

    fn ttl(config: &PendingQueueConfig) -> StdDuration {
        StdDuration::from_millis(config.pending_ttl_ms)
    }

    fn update_peer_gauge(&mut self, peer_id: &str) {
        let peer_len = self.queues.get(peer_id).map(VecDeque::len).unwrap_or(0);
        if peer_len == 0 {
            self.metrics.pending_messages_per_peer.remove(peer_id);
        } else {
            self.metrics
                .pending_messages_per_peer
                .insert(peer_id.to_string(), peer_len);
        }
    }

    fn update_current_gauge(&mut self) {
        self.metrics.pending_messages_current = self.total;
        self.metrics.pending_bytes_current = self.total_bytes;
    }

    /// Payload footprint of a queued message: text content plus binary content.
    fn message_bytes(message: &Message) -> usize {
        message.content.len()
            + message
                .binary_content
                .as_ref()
                .map(|binary| binary.len())
                .unwrap_or(0)
    }

    fn peer_bytes_for(&self, peer_id: &str) -> usize {
        self.peer_bytes.get(peer_id).copied().unwrap_or(0)
    }

    fn next_sequence(&mut self) -> u64 {
        let seq = self.next_seq;
        self.next_seq = self.next_seq.wrapping_add(1);
        seq
    }

    fn is_entry_expired(
        config: &PendingQueueConfig,
        entry: &PendingDecryptMessage,
        now: Instant,
    ) -> bool {
        now.saturating_duration_since(entry.received_at) >= Self::ttl(config)
    }

    fn cleanup_global_order_front(&mut self) {
        while let Some(front) = self.global_order.front() {
            if self.live_sequences.contains(&front.sequence) {
                break;
            }
            self.global_order.pop_front();
        }
    }

    fn remove_entry_by_sequence(
        &mut self,
        peer_id: &str,
        sequence: u64,
    ) -> Option<PendingDecryptMessage> {
        let (removed, queue_empty) = {
            let queue = self.queues.get_mut(peer_id)?;
            let position = queue.iter().position(|entry| entry.sequence == sequence)?;
            let removed = queue.remove(position)?;
            (removed, queue.is_empty())
        };

        self.live_sequences.remove(&sequence);
        self.total = self.total.saturating_sub(1);
        let removed_bytes = Self::message_bytes(&removed.message);
        self.total_bytes = self.total_bytes.saturating_sub(removed_bytes);
        if let Some(bytes) = self.peer_bytes.get_mut(peer_id) {
            *bytes = bytes.saturating_sub(removed_bytes);
        }
        self.update_current_gauge();

        if queue_empty {
            self.queues.remove(peer_id);
            self.peer_bytes.remove(peer_id);
        }
        self.update_peer_gauge(peer_id);
        Some(removed)
    }

    fn record_drop(
        &mut self,
        reason: DropReason,
        limit_triggered: Option<QueueLimit>,
        peer_id: &str,
        message_id: &str,
        overflow_policy: OverflowPolicy,
    ) {
        if matches!(
            reason,
            DropReason::OverflowDropOldest | DropReason::TtlExpired
        ) {
            self.metrics.pending_messages_evicted_total = self
                .metrics
                .pending_messages_evicted_total
                .saturating_add(1);
        }

        if matches!(
            reason,
            DropReason::OverflowDropOldest | DropReason::OverflowDropNewest
        ) {
            self.metrics.pending_messages_dropped_overflow_total = self
                .metrics
                .pending_messages_dropped_overflow_total
                .saturating_add(1);
        }

        if reason == DropReason::TtlExpired {
            self.metrics.pending_messages_expired_total = self
                .metrics
                .pending_messages_expired_total
                .saturating_add(1);
        }

        let limit_label = limit_triggered.map(QueueLimit::as_str).unwrap_or("ttl");
        let counter_key = format!("{}:{}", reason.as_str(), limit_label);
        let drop_count = {
            let counter = self.drop_warning_counters.entry(counter_key).or_insert(0);
            *counter = counter.saturating_add(1);
            *counter
        };

        debug!(
            reason = reason.as_str(),
            peer_id = %peer_id,
            message_id = %message_id,
            queue_size = self.total,
            limit_triggered = limit_label,
            overflow_policy = ?overflow_policy,
            "Dropped pending encrypted message"
        );
        if drop_count == 1 || drop_count % DROP_WARN_EVERY == 0 {
            warn!(
                reason = reason.as_str(),
                limit_triggered = limit_label,
                drops = drop_count,
                queue_size = self.total,
                "Pending encrypted message drops continuing"
            );
        }
    }

    fn record_eviction_failure(
        &mut self,
        limit_triggered: QueueLimit,
        peer_id: &str,
        message_id: &str,
        detail: &str,
    ) {
        self.metrics.pending_messages_eviction_failures_total = self
            .metrics
            .pending_messages_eviction_failures_total
            .saturating_add(1);

        let count = self.metrics.pending_messages_eviction_failures_total;
        if count == 1 || count.is_multiple_of(EVICTION_FAILURE_WARN_EVERY) {
            warn!(
                limit_triggered = limit_triggered.as_str(),
                peer_id = %peer_id,
                message_id = %message_id,
                failures = count,
                detail = detail,
                queue_size = self.total,
                "Pending queue eviction failure detected"
            );
        } else {
            debug!(
                limit_triggered = limit_triggered.as_str(),
                peer_id = %peer_id,
                message_id = %message_id,
                detail = detail,
                "Pending queue eviction failure detected"
            );
        }
    }

    fn verify_invariants(&mut self, context: &str) {
        let per_peer_sum: usize = self.queues.values().map(VecDeque::len).sum();
        let live_count = self.live_sequences.len();
        let current_gauge = self.metrics.pending_messages_current;
        let total = self.total;
        let peer_bytes_sum: usize = self.peer_bytes.values().sum();
        let valid = per_peer_sum == total
            && live_count == total
            && current_gauge == total
            && peer_bytes_sum == self.total_bytes;
        if valid {
            return;
        }

        self.metrics.pending_queue_invariant_violations_total = self
            .metrics
            .pending_queue_invariant_violations_total
            .saturating_add(1);
        warn!(
            context = context,
            per_peer_sum,
            live_count,
            current_gauge,
            total,
            violations = self.metrics.pending_queue_invariant_violations_total,
            "Pending queue invariant violation detected"
        );
    }

    fn record_peer_overflow_pressure(&mut self, peer_id: &str, per_peer_limit: usize) {
        let hits = self
            .peer_overflow_hits
            .entry(peer_id.to_string())
            .or_insert(0);
        *hits = hits.saturating_add(1);
        if hits.is_multiple_of(PEER_PRESSURE_WARN_EVERY) {
            warn!(
                peer_id = %peer_id,
                overflow_hits = *hits,
                per_peer_limit,
                "Peer repeatedly hitting pending queue limits"
            );
        }
    }

    fn evict_global_oldest(
        &mut self,
        reason: DropReason,
        limit_triggered: QueueLimit,
        overflow_policy: OverflowPolicy,
    ) -> Option<PendingDecryptMessage> {
        self.cleanup_global_order_front();
        let entry_ref = self.global_order.pop_front()?;
        if !self.live_sequences.contains(&entry_ref.sequence) {
            return None;
        }
        if let Some(evicted) = self.remove_entry_by_sequence(&entry_ref.peer_id, entry_ref.sequence)
        {
            self.record_drop(
                reason,
                Some(limit_triggered),
                &evicted.peer_id,
                &evicted.message_id,
                overflow_policy,
            );
            return Some(evicted);
        }
        warn!(
            peer_id = %entry_ref.peer_id,
            message_id = %entry_ref.message_id,
            sequence = entry_ref.sequence,
            "Failed to evict pending message by global order reference"
        );
        None
    }

    // ---- Public queue operations ----

    /// Prunes expired entries for a specific peer. Returns the dropped entries.
    pub(crate) fn prune_expired_for_peer(
        &mut self,
        config: &PendingQueueConfig,
        peer_id: &str,
        now: Instant,
    ) -> Vec<DroppedPendingMessage> {
        let mut expired_entries = Vec::new();
        let mut expired_sequences = Vec::new();
        let mut expired_ids = Vec::new();
        if let Some(queue) = self.queues.get(peer_id) {
            for entry in queue {
                if Self::is_entry_expired(config, entry, now) {
                    expired_sequences.push(entry.sequence);
                    expired_ids.push(entry.message_id.clone());
                } else {
                    break;
                }
            }
        }

        let overflow_policy = config.overflow_policy;
        for (sequence, message_id) in expired_sequences.into_iter().zip(expired_ids) {
            if let Some(expired) = self.remove_entry_by_sequence(peer_id, sequence) {
                self.record_drop(
                    DropReason::TtlExpired,
                    None,
                    peer_id,
                    &message_id,
                    overflow_policy,
                );
                expired_entries.push(DroppedPendingMessage {
                    message: expired.message,
                    reason: DropReason::TtlExpired.as_str(),
                });
            }
        }

        if expired_entries.len() >= TTL_SPIKE_WARN_THRESHOLD {
            warn!(
                peer_id = %peer_id,
                expired = expired_entries.len(),
                ttl_ms = config.pending_ttl_ms,
                "Pending encrypted message TTL eviction spike"
            );
        }

        self.cleanup_global_order_front();
        expired_entries
    }

    /// Prunes expired entries from the front of the global order. Returns the dropped entries.
    pub(crate) fn prune_expired_global_front(
        &mut self,
        config: &PendingQueueConfig,
        now: Instant,
        max_evictions: usize,
    ) -> Vec<DroppedPendingMessage> {
        let mut evicted = Vec::new();
        let overflow_policy = config.overflow_policy;
        while evicted.len() < max_evictions {
            self.cleanup_global_order_front();
            let Some(front) = self.global_order.front().cloned() else {
                break;
            };
            if !self.live_sequences.contains(&front.sequence) {
                self.global_order.pop_front();
                continue;
            }

            let Some(queue) = self.queues.get(&front.peer_id) else {
                self.global_order.pop_front();
                continue;
            };
            let Some(entry) = queue.iter().find(|entry| entry.sequence == front.sequence) else {
                self.global_order.pop_front();
                continue;
            };

            if !Self::is_entry_expired(config, entry, now) {
                break;
            }

            if let Some(expired) = self.remove_entry_by_sequence(&front.peer_id, front.sequence) {
                self.global_order.pop_front();
                self.record_drop(
                    DropReason::TtlExpired,
                    None,
                    &expired.peer_id,
                    &expired.message_id,
                    overflow_policy,
                );
                evicted.push(DroppedPendingMessage {
                    message: expired.message,
                    reason: DropReason::TtlExpired.as_str(),
                });
            } else {
                self.global_order.pop_front();
            }
        }

        if evicted.len() >= TTL_SPIKE_WARN_THRESHOLD {
            warn!(
                expired = evicted.len(),
                ttl_ms = config.pending_ttl_ms,
                "Pending encrypted message TTL eviction spike"
            );
        }

        evicted
    }

    /// Enqueues an encrypted message that arrived before the MLS session was ready.
    ///
    /// Returns every message dropped in the process — TTL-expired entries,
    /// entries evicted to make room, or the incoming message itself when it
    /// could not be admitted — so the protocol layer can surface the loss.
    pub(crate) fn enqueue(
        &mut self,
        config: &PendingQueueConfig,
        sender: &str,
        message: &Message,
    ) -> Vec<DroppedPendingMessage> {
        self.metrics.pending_messages_received_total = self
            .metrics
            .pending_messages_received_total
            .saturating_add(1);
        let incoming_message_id = message.id.as_str();

        let now = Instant::now();
        let mut dropped = self.prune_expired_for_peer(config, sender, now);
        dropped.extend(self.prune_expired_global_front(config, now, 64));

        let per_peer_limit = config.max_pending_per_peer;
        let global_limit = config.max_pending_global;
        let per_peer_bytes_limit = config.max_pending_bytes_per_peer;
        let global_bytes_limit = config.max_pending_bytes_global;
        let overflow_policy = config.overflow_policy;
        let incoming_bytes = Self::message_bytes(message);

        // A message that can never fit within the byte budgets is dropped
        // outright — evicting the entire queue would not make room for it.
        // This also guarantees the byte-eviction loops below terminate.
        if incoming_bytes > per_peer_bytes_limit || incoming_bytes > global_bytes_limit {
            let limit = if incoming_bytes > per_peer_bytes_limit {
                QueueLimit::PerPeerBytes
            } else {
                QueueLimit::GlobalBytes
            };
            self.record_drop(
                DropReason::OverflowDropNewest,
                Some(limit),
                sender,
                &incoming_message_id,
                overflow_policy,
            );
            dropped.push(DroppedPendingMessage {
                message: message.clone(),
                reason: DropReason::OverflowDropNewest.as_str(),
            });
            return dropped;
        }

        let peer_len = self.queues.get(sender).map(VecDeque::len).unwrap_or(0);
        if peer_len >= per_peer_limit {
            self.record_peer_overflow_pressure(sender, per_peer_limit);
            match overflow_policy {
                OverflowPolicy::DropNewest => {
                    self.record_drop(
                        DropReason::OverflowDropNewest,
                        Some(QueueLimit::PerPeer),
                        sender,
                        &incoming_message_id,
                        overflow_policy,
                    );
                    dropped.push(DroppedPendingMessage {
                        message: message.clone(),
                        reason: DropReason::OverflowDropNewest.as_str(),
                    });
                    return dropped;
                }
                OverflowPolicy::DropOldest => {
                    let evicted_sequence = self
                        .queues
                        .get(sender)
                        .and_then(|queue| queue.front().map(|entry| entry.sequence));
                    let mut evicted_any = false;
                    if let Some(sequence) = evicted_sequence {
                        if let Some(evicted) = self.remove_entry_by_sequence(sender, sequence) {
                            self.record_drop(
                                DropReason::OverflowDropOldest,
                                Some(QueueLimit::PerPeer),
                                &evicted.peer_id,
                                &evicted.message_id,
                                overflow_policy,
                            );
                            dropped.push(DroppedPendingMessage {
                                message: evicted.message,
                                reason: DropReason::OverflowDropOldest.as_str(),
                            });
                            evicted_any = true;
                        }
                    }
                    if !evicted_any {
                        self.record_eviction_failure(
                            QueueLimit::PerPeer,
                            sender,
                            &incoming_message_id,
                            "drop_oldest failed to evict per-peer oldest",
                        );
                        self.record_drop(
                            DropReason::OverflowDropNewest,
                            Some(QueueLimit::PerPeer),
                            sender,
                            &incoming_message_id,
                            overflow_policy,
                        );
                        dropped.push(DroppedPendingMessage {
                            message: message.clone(),
                            reason: DropReason::OverflowDropNewest.as_str(),
                        });
                        return dropped;
                    }
                }
            }
        }
        let peer_len_after = self.queues.get(sender).map(VecDeque::len).unwrap_or(0);
        if peer_len_after >= per_peer_limit {
            self.record_eviction_failure(
                QueueLimit::PerPeer,
                sender,
                &incoming_message_id,
                "per-peer limit still saturated after eviction",
            );
            self.record_drop(
                DropReason::OverflowDropNewest,
                Some(QueueLimit::PerPeer),
                sender,
                &incoming_message_id,
                overflow_policy,
            );
            dropped.push(DroppedPendingMessage {
                message: message.clone(),
                reason: DropReason::OverflowDropNewest.as_str(),
            });
            return dropped;
        }

        if self.peer_bytes_for(sender) + incoming_bytes > per_peer_bytes_limit {
            self.record_peer_overflow_pressure(sender, per_peer_limit);
            if overflow_policy == OverflowPolicy::DropNewest {
                self.record_drop(
                    DropReason::OverflowDropNewest,
                    Some(QueueLimit::PerPeerBytes),
                    sender,
                    &incoming_message_id,
                    overflow_policy,
                );
                dropped.push(DroppedPendingMessage {
                    message: message.clone(),
                    reason: DropReason::OverflowDropNewest.as_str(),
                });
                return dropped;
            }
            // DropOldest: evict from the peer's front until the incoming
            // message fits. Terminates: each eviction shrinks the peer's byte
            // total, and an empty queue leaves it at 0 (the incoming message
            // fits by the oversized pre-check above).
            while self.peer_bytes_for(sender) + incoming_bytes > per_peer_bytes_limit {
                let evicted_sequence = self
                    .queues
                    .get(sender)
                    .and_then(|queue| queue.front().map(|entry| entry.sequence));
                let evicted = evicted_sequence
                    .and_then(|sequence| self.remove_entry_by_sequence(sender, sequence));
                match evicted {
                    Some(evicted) => {
                        self.record_drop(
                            DropReason::OverflowDropOldest,
                            Some(QueueLimit::PerPeerBytes),
                            &evicted.peer_id,
                            &evicted.message_id,
                            overflow_policy,
                        );
                        dropped.push(DroppedPendingMessage {
                            message: evicted.message,
                            reason: DropReason::OverflowDropOldest.as_str(),
                        });
                    }
                    None => {
                        self.record_eviction_failure(
                            QueueLimit::PerPeerBytes,
                            sender,
                            &incoming_message_id,
                            "drop_oldest failed to evict per-peer oldest for byte budget",
                        );
                        self.record_drop(
                            DropReason::OverflowDropNewest,
                            Some(QueueLimit::PerPeerBytes),
                            sender,
                            &incoming_message_id,
                            overflow_policy,
                        );
                        dropped.push(DroppedPendingMessage {
                            message: message.clone(),
                            reason: DropReason::OverflowDropNewest.as_str(),
                        });
                        return dropped;
                    }
                }
            }
        }

        if self.total >= global_limit {
            warn!(
                queue_size = self.total,
                global_limit, "Pending encrypted queue at global pressure limit"
            );
            match overflow_policy {
                OverflowPolicy::DropNewest => {
                    self.record_drop(
                        DropReason::OverflowDropNewest,
                        Some(QueueLimit::Global),
                        sender,
                        &incoming_message_id,
                        overflow_policy,
                    );
                    dropped.push(DroppedPendingMessage {
                        message: message.clone(),
                        reason: DropReason::OverflowDropNewest.as_str(),
                    });
                    return dropped;
                }
                OverflowPolicy::DropOldest => {
                    while self.total >= global_limit {
                        match self.evict_global_oldest(
                            DropReason::OverflowDropOldest,
                            QueueLimit::Global,
                            overflow_policy,
                        ) {
                            Some(evicted) => dropped.push(DroppedPendingMessage {
                                message: evicted.message,
                                reason: DropReason::OverflowDropOldest.as_str(),
                            }),
                            None => {
                                self.record_eviction_failure(
                                    QueueLimit::Global,
                                    sender,
                                    &incoming_message_id,
                                    "drop_oldest failed to evict global oldest",
                                );
                                break;
                            }
                        }
                    }
                }
            }
        }
        if self.total >= global_limit {
            self.record_drop(
                DropReason::OverflowDropNewest,
                Some(QueueLimit::Global),
                sender,
                &incoming_message_id,
                overflow_policy,
            );
            dropped.push(DroppedPendingMessage {
                message: message.clone(),
                reason: DropReason::OverflowDropNewest.as_str(),
            });
            return dropped;
        }

        if self.total_bytes + incoming_bytes > global_bytes_limit {
            warn!(
                queue_bytes = self.total_bytes,
                global_bytes_limit, "Pending encrypted queue at global byte budget"
            );
            if overflow_policy == OverflowPolicy::DropNewest {
                self.record_drop(
                    DropReason::OverflowDropNewest,
                    Some(QueueLimit::GlobalBytes),
                    sender,
                    &incoming_message_id,
                    overflow_policy,
                );
                dropped.push(DroppedPendingMessage {
                    message: message.clone(),
                    reason: DropReason::OverflowDropNewest.as_str(),
                });
                return dropped;
            }
            while self.total_bytes + incoming_bytes > global_bytes_limit {
                match self.evict_global_oldest(
                    DropReason::OverflowDropOldest,
                    QueueLimit::GlobalBytes,
                    overflow_policy,
                ) {
                    Some(evicted) => dropped.push(DroppedPendingMessage {
                        message: evicted.message,
                        reason: DropReason::OverflowDropOldest.as_str(),
                    }),
                    None => {
                        self.record_eviction_failure(
                            QueueLimit::GlobalBytes,
                            sender,
                            &incoming_message_id,
                            "drop_oldest failed to evict global oldest for byte budget",
                        );
                        break;
                    }
                }
            }
            if self.total_bytes + incoming_bytes > global_bytes_limit {
                self.record_drop(
                    DropReason::OverflowDropNewest,
                    Some(QueueLimit::GlobalBytes),
                    sender,
                    &incoming_message_id,
                    overflow_policy,
                );
                dropped.push(DroppedPendingMessage {
                    message: message.clone(),
                    reason: DropReason::OverflowDropNewest.as_str(),
                });
                return dropped;
            }
        }

        let sequence = self.next_sequence();
        let entry = PendingDecryptMessage {
            peer_id: sender.to_string(),
            message_id: incoming_message_id.clone(),
            received_at: now,
            sequence,
            message: message.clone(),
        };

        self.queues
            .entry(sender.to_string())
            .or_default()
            .push_back(entry.clone());
        self.global_order.push_back(EntryRef {
            peer_id: sender.to_string(),
            message_id: entry.message_id.clone(),
            sequence,
        });
        self.live_sequences.insert(sequence);
        self.total = self.total.saturating_add(1);
        self.total_bytes = self.total_bytes.saturating_add(incoming_bytes);
        let peer_bytes = self.peer_bytes.entry(sender.to_string()).or_insert(0);
        *peer_bytes = peer_bytes.saturating_add(incoming_bytes);
        self.update_peer_gauge(sender);
        self.update_current_gauge();
        self.cleanup_global_order_front();
        self.verify_invariants("enqueue");

        let peer_len_post_insert = self.queues.get(sender).map(VecDeque::len).unwrap_or(0);
        let over_global = self.total > global_limit || self.total_bytes > global_bytes_limit;
        let over_peer = peer_len_post_insert > per_peer_limit
            || self.peer_bytes_for(sender) > per_peer_bytes_limit;
        if over_global || over_peer {
            let _ = self.remove_entry_by_sequence(sender, sequence);
            let limit = if over_global {
                QueueLimit::Global
            } else {
                QueueLimit::PerPeer
            };
            self.record_eviction_failure(
                limit,
                sender,
                &incoming_message_id,
                "post-insert hard-bound check failed; rolled back enqueue",
            );
            self.record_drop(
                DropReason::OverflowDropNewest,
                Some(limit),
                sender,
                &incoming_message_id,
                overflow_policy,
            );
            dropped.push(DroppedPendingMessage {
                message: message.clone(),
                reason: DropReason::OverflowDropNewest.as_str(),
            });
        }

        dropped
    }

    /// Drains all pending messages for a peer, updating bookkeeping.
    /// Returns the drained messages for the caller to process.
    pub(crate) fn drain_for_peer(
        &mut self,
        config: &PendingQueueConfig,
        sender: &str,
    ) -> Vec<PendingDecryptMessage> {
        let now = Instant::now();
        let _ = self.prune_expired_for_peer(config, sender, now);

        let messages = match self.queues.remove(sender) {
            Some(msgs) => msgs,
            None => return Vec::new(),
        };

        if messages.is_empty() {
            return Vec::new();
        }

        let drained: Vec<PendingDecryptMessage> = messages.into_iter().collect();
        for entry in &drained {
            self.live_sequences.remove(&entry.sequence);
            self.total = self.total.saturating_sub(1);
            self.total_bytes = self
                .total_bytes
                .saturating_sub(Self::message_bytes(&entry.message));
        }
        self.peer_bytes.remove(sender);
        self.update_peer_gauge(sender);
        self.update_current_gauge();
        self.cleanup_global_order_front();
        self.verify_invariants("drain_for_peer");

        drained
    }
}
