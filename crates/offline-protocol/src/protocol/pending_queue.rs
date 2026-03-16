//! Pending decryption queue: bounded storage for encrypted messages received
//! before the MLS session is ready, with per-peer and global limits, TTL
//! expiration, and configurable overflow policies.

use super::{
    lock_shared_state, InternalMessageResult, OfflineProtocol, PendingDecryptEntryRef,
    PendingDecryptMessage, PendingQueueDropReason, PendingQueueLimit, PENDING_DROP_WARN_EVERY,
    PENDING_EVICTION_FAILURE_WARN_EVERY, PENDING_PEER_PRESSURE_WARN_EVERY,
    PENDING_TTL_SPIKE_WARN_THRESHOLD,
};
use crate::events::Event;
use chrono::Utc;
use offline_protocol_core::Message;
use std::collections::VecDeque;
use std::time::{Duration as StdDuration, Instant};
use tracing::{debug, info, warn};

impl OfflineProtocol {
    fn pending_queue_ttl(&self) -> StdDuration {
        StdDuration::from_millis(self.config.encryption.pending_queue.pending_ttl_ms)
    }

    fn update_pending_peer_gauge(&mut self, peer_id: &str) {
        let peer_len = self
            .pending_decryption
            .get(peer_id)
            .map(VecDeque::len)
            .unwrap_or(0);
        if peer_len == 0 {
            self.pending_queue_metrics
                .pending_messages_per_peer
                .remove(peer_id);
        } else {
            self.pending_queue_metrics
                .pending_messages_per_peer
                .insert(peer_id.to_string(), peer_len);
        }
    }

    fn update_pending_queue_current_gauge(&mut self) {
        self.pending_queue_metrics.pending_messages_current = self.pending_decryption_total;
    }

    fn next_pending_sequence(&mut self) -> u64 {
        let seq = self.pending_decryption_next_sequence;
        self.pending_decryption_next_sequence =
            self.pending_decryption_next_sequence.wrapping_add(1);
        seq
    }

    fn is_pending_entry_expired(&self, entry: &PendingDecryptMessage, now: Instant) -> bool {
        now.saturating_duration_since(entry.received_at) >= self.pending_queue_ttl()
    }

    fn cleanup_global_order_front(&mut self) {
        while let Some(front) = self.pending_decryption_global_order.front() {
            if self
                .pending_decryption_live_sequences
                .contains(&front.sequence)
            {
                break;
            }
            self.pending_decryption_global_order.pop_front();
        }
    }

    fn remove_pending_entry_by_sequence(
        &mut self,
        peer_id: &str,
        sequence: u64,
    ) -> Option<PendingDecryptMessage> {
        let (removed, queue_empty) = {
            let queue = self.pending_decryption.get_mut(peer_id)?;
            let position = queue.iter().position(|entry| entry.sequence == sequence)?;
            let removed = queue.remove(position)?;
            (removed, queue.is_empty())
        };

        self.pending_decryption_live_sequences.remove(&sequence);
        self.pending_decryption_total = self.pending_decryption_total.saturating_sub(1);
        self.update_pending_queue_current_gauge();

        if queue_empty {
            self.pending_decryption.remove(peer_id);
        }
        self.update_pending_peer_gauge(peer_id);
        Some(removed)
    }

    fn record_pending_drop(
        &mut self,
        reason: PendingQueueDropReason,
        limit_triggered: Option<PendingQueueLimit>,
        peer_id: &str,
        message_id: &str,
    ) {
        if matches!(
            reason,
            PendingQueueDropReason::OverflowDropOldest | PendingQueueDropReason::TtlExpired
        ) {
            self.pending_queue_metrics.pending_messages_evicted_total = self
                .pending_queue_metrics
                .pending_messages_evicted_total
                .saturating_add(1);
        }

        if matches!(
            reason,
            PendingQueueDropReason::OverflowDropOldest | PendingQueueDropReason::OverflowDropNewest
        ) {
            self.pending_queue_metrics
                .pending_messages_dropped_overflow_total = self
                .pending_queue_metrics
                .pending_messages_dropped_overflow_total
                .saturating_add(1);
        }

        if reason == PendingQueueDropReason::TtlExpired {
            self.pending_queue_metrics.pending_messages_expired_total = self
                .pending_queue_metrics
                .pending_messages_expired_total
                .saturating_add(1);
        }

        let limit_label = limit_triggered
            .map(PendingQueueLimit::as_str)
            .unwrap_or("ttl");
        let counter_key = format!("{}:{}", reason.as_str(), limit_label);
        let drop_count = {
            let counter = self
                .pending_drop_warning_counters
                .entry(counter_key)
                .or_insert(0);
            *counter = counter.saturating_add(1);
            *counter
        };

        debug!(
            reason = reason.as_str(),
            peer_id = %peer_id,
            message_id = %message_id,
            queue_size = self.pending_decryption_total,
            limit_triggered = limit_label,
            overflow_policy = ?self.config.encryption.pending_queue.overflow_policy,
            "Dropped pending encrypted message"
        );
        if drop_count == 1 || drop_count % PENDING_DROP_WARN_EVERY == 0 {
            warn!(
                reason = reason.as_str(),
                limit_triggered = limit_label,
                drops = drop_count,
                queue_size = self.pending_decryption_total,
                "Pending encrypted message drops continuing"
            );
        }
    }

    fn record_pending_eviction_failure(
        &mut self,
        limit_triggered: PendingQueueLimit,
        peer_id: &str,
        message_id: &str,
        detail: &str,
    ) {
        self.pending_queue_metrics
            .pending_messages_eviction_failures_total = self
            .pending_queue_metrics
            .pending_messages_eviction_failures_total
            .saturating_add(1);

        let count = self
            .pending_queue_metrics
            .pending_messages_eviction_failures_total;
        if count == 1 || count % PENDING_EVICTION_FAILURE_WARN_EVERY == 0 {
            warn!(
                limit_triggered = limit_triggered.as_str(),
                peer_id = %peer_id,
                message_id = %message_id,
                failures = count,
                detail = detail,
                queue_size = self.pending_decryption_total,
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

    fn verify_pending_queue_invariants(&mut self, context: &str) {
        let per_peer_sum: usize = self.pending_decryption.values().map(VecDeque::len).sum();
        let live_count = self.pending_decryption_live_sequences.len();
        let current_gauge = self.pending_queue_metrics.pending_messages_current;
        let total = self.pending_decryption_total;
        let valid = per_peer_sum == total && live_count == total && current_gauge == total;
        if valid {
            return;
        }

        self.pending_queue_metrics
            .pending_queue_invariant_violations_total = self
            .pending_queue_metrics
            .pending_queue_invariant_violations_total
            .saturating_add(1);
        warn!(
            context = context,
            per_peer_sum,
            live_count,
            current_gauge,
            total,
            violations = self
                .pending_queue_metrics
                .pending_queue_invariant_violations_total,
            "Pending queue invariant violation detected"
        );
    }

    fn record_peer_overflow_pressure(&mut self, peer_id: &str) {
        let hits = self
            .pending_peer_overflow_hits
            .entry(peer_id.to_string())
            .or_insert(0);
        *hits = hits.saturating_add(1);
        if *hits % PENDING_PEER_PRESSURE_WARN_EVERY == 0 {
            warn!(
                peer_id = %peer_id,
                overflow_hits = *hits,
                per_peer_limit = self.config.encryption.pending_queue.max_pending_per_peer,
                "Peer repeatedly hitting pending queue limits"
            );
        }
    }

    fn evict_global_oldest(
        &mut self,
        reason: PendingQueueDropReason,
        limit_triggered: PendingQueueLimit,
    ) -> bool {
        self.cleanup_global_order_front();
        let Some(entry_ref) = self.pending_decryption_global_order.pop_front() else {
            return false;
        };
        if !self
            .pending_decryption_live_sequences
            .contains(&entry_ref.sequence)
        {
            return false;
        }
        if let Some(evicted) =
            self.remove_pending_entry_by_sequence(&entry_ref.peer_id, entry_ref.sequence)
        {
            self.record_pending_drop(
                reason,
                Some(limit_triggered),
                &evicted.peer_id,
                &evicted.message_id,
            );
            return true;
        }
        warn!(
            peer_id = %entry_ref.peer_id,
            message_id = %entry_ref.message_id,
            sequence = entry_ref.sequence,
            "Failed to evict pending message by global order reference"
        );
        false
    }

    pub(super) fn prune_expired_pending_for_peer(&mut self, peer_id: &str, now: Instant) -> usize {
        let mut expired_count = 0usize;
        let mut expired_sequences = Vec::new();
        let mut expired_ids = Vec::new();
        if let Some(queue) = self.pending_decryption.get(peer_id) {
            for entry in queue {
                if self.is_pending_entry_expired(entry, now) {
                    expired_sequences.push(entry.sequence);
                    expired_ids.push(entry.message_id.clone());
                } else {
                    break;
                }
            }
        }

        for (sequence, message_id) in expired_sequences.into_iter().zip(expired_ids) {
            if self
                .remove_pending_entry_by_sequence(peer_id, sequence)
                .is_some()
            {
                self.record_pending_drop(
                    PendingQueueDropReason::TtlExpired,
                    None,
                    peer_id,
                    &message_id,
                );
                expired_count = expired_count.saturating_add(1);
            }
        }

        if expired_count >= PENDING_TTL_SPIKE_WARN_THRESHOLD {
            warn!(
                peer_id = %peer_id,
                expired = expired_count,
                ttl_ms = self.config.encryption.pending_queue.pending_ttl_ms,
                "Pending encrypted message TTL eviction spike"
            );
        }

        self.cleanup_global_order_front();
        expired_count
    }

    pub(super) fn prune_expired_pending_global_front(
        &mut self,
        now: Instant,
        max_evictions: usize,
    ) -> usize {
        let mut evicted = 0usize;
        while evicted < max_evictions {
            self.cleanup_global_order_front();
            let Some(front) = self.pending_decryption_global_order.front().cloned() else {
                break;
            };
            if !self
                .pending_decryption_live_sequences
                .contains(&front.sequence)
            {
                self.pending_decryption_global_order.pop_front();
                continue;
            }

            let Some(queue) = self.pending_decryption.get(&front.peer_id) else {
                self.pending_decryption_global_order.pop_front();
                continue;
            };
            let Some(entry) = queue.iter().find(|entry| entry.sequence == front.sequence) else {
                self.pending_decryption_global_order.pop_front();
                continue;
            };

            if !self.is_pending_entry_expired(entry, now) {
                break;
            }

            if let Some(expired) =
                self.remove_pending_entry_by_sequence(&front.peer_id, front.sequence)
            {
                self.pending_decryption_global_order.pop_front();
                self.record_pending_drop(
                    PendingQueueDropReason::TtlExpired,
                    None,
                    &expired.peer_id,
                    &expired.message_id,
                );
                evicted = evicted.saturating_add(1);
            } else {
                self.pending_decryption_global_order.pop_front();
            }
        }

        if evicted >= PENDING_TTL_SPIKE_WARN_THRESHOLD {
            warn!(
                expired = evicted,
                ttl_ms = self.config.encryption.pending_queue.pending_ttl_ms,
                "Pending encrypted message TTL eviction spike"
            );
        }

        evicted
    }

    pub(super) fn enqueue_pending_decryption(&mut self, sender: &str, message: &Message) {
        self.pending_queue_metrics.pending_messages_received_total = self
            .pending_queue_metrics
            .pending_messages_received_total
            .saturating_add(1);
        let incoming_message_id = message.id.as_str();

        let now = Instant::now();
        let _ = self.prune_expired_pending_for_peer(sender, now);
        let _ = self.prune_expired_pending_global_front(now, 64);

        let per_peer_limit = self.config.encryption.pending_queue.max_pending_per_peer;
        let global_limit = self.config.encryption.pending_queue.max_pending_global;
        let overflow_policy = self.config.encryption.pending_queue.overflow_policy;

        let peer_len = self
            .pending_decryption
            .get(sender)
            .map(VecDeque::len)
            .unwrap_or(0);
        if peer_len >= per_peer_limit {
            self.record_peer_overflow_pressure(sender);
            match overflow_policy {
                crate::config::OverflowPolicy::DropNewest => {
                    self.record_pending_drop(
                        PendingQueueDropReason::OverflowDropNewest,
                        Some(PendingQueueLimit::PerPeer),
                        sender,
                        &incoming_message_id,
                    );
                    return;
                }
                crate::config::OverflowPolicy::DropOldest => {
                    let evicted_sequence = self
                        .pending_decryption
                        .get(sender)
                        .and_then(|queue| queue.front().map(|entry| entry.sequence));
                    let mut evicted_any = false;
                    if let Some(sequence) = evicted_sequence {
                        if let Some(evicted) =
                            self.remove_pending_entry_by_sequence(sender, sequence)
                        {
                            self.record_pending_drop(
                                PendingQueueDropReason::OverflowDropOldest,
                                Some(PendingQueueLimit::PerPeer),
                                &evicted.peer_id,
                                &evicted.message_id,
                            );
                            evicted_any = true;
                        }
                    }
                    if !evicted_any {
                        self.record_pending_eviction_failure(
                            PendingQueueLimit::PerPeer,
                            sender,
                            &incoming_message_id,
                            "drop_oldest failed to evict per-peer oldest",
                        );
                        self.record_pending_drop(
                            PendingQueueDropReason::OverflowDropNewest,
                            Some(PendingQueueLimit::PerPeer),
                            sender,
                            &incoming_message_id,
                        );
                        return;
                    }
                }
            }
        }
        let peer_len_after = self
            .pending_decryption
            .get(sender)
            .map(VecDeque::len)
            .unwrap_or(0);
        if peer_len_after >= per_peer_limit {
            self.record_pending_eviction_failure(
                PendingQueueLimit::PerPeer,
                sender,
                &incoming_message_id,
                "per-peer limit still saturated after eviction",
            );
            self.record_pending_drop(
                PendingQueueDropReason::OverflowDropNewest,
                Some(PendingQueueLimit::PerPeer),
                sender,
                &incoming_message_id,
            );
            return;
        }

        if self.pending_decryption_total >= global_limit {
            warn!(
                queue_size = self.pending_decryption_total,
                global_limit, "Pending encrypted queue at global pressure limit"
            );
            match overflow_policy {
                crate::config::OverflowPolicy::DropNewest => {
                    self.record_pending_drop(
                        PendingQueueDropReason::OverflowDropNewest,
                        Some(PendingQueueLimit::Global),
                        sender,
                        &incoming_message_id,
                    );
                    return;
                }
                crate::config::OverflowPolicy::DropOldest => {
                    while self.pending_decryption_total >= global_limit {
                        if !self.evict_global_oldest(
                            PendingQueueDropReason::OverflowDropOldest,
                            PendingQueueLimit::Global,
                        ) {
                            self.record_pending_eviction_failure(
                                PendingQueueLimit::Global,
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
        if self.pending_decryption_total >= global_limit {
            self.record_pending_drop(
                PendingQueueDropReason::OverflowDropNewest,
                Some(PendingQueueLimit::Global),
                sender,
                &incoming_message_id,
            );
            return;
        }

        let sequence = self.next_pending_sequence();
        let entry = PendingDecryptMessage {
            peer_id: sender.to_string(),
            message_id: message.id.as_str().to_string(),
            received_at: now,
            sequence,
            message: message.clone(),
        };

        self.pending_decryption
            .entry(sender.to_string())
            .or_default()
            .push_back(entry.clone());
        self.pending_decryption_global_order
            .push_back(PendingDecryptEntryRef {
                peer_id: sender.to_string(),
                message_id: entry.message_id.clone(),
                sequence,
            });
        self.pending_decryption_live_sequences.insert(sequence);
        self.pending_decryption_total = self.pending_decryption_total.saturating_add(1);
        self.update_pending_peer_gauge(sender);
        self.update_pending_queue_current_gauge();
        self.cleanup_global_order_front();
        self.verify_pending_queue_invariants("enqueue");

        let peer_len_post_insert = self
            .pending_decryption
            .get(sender)
            .map(VecDeque::len)
            .unwrap_or(0);
        if self.pending_decryption_total > global_limit || peer_len_post_insert > per_peer_limit {
            let _ = self.remove_pending_entry_by_sequence(sender, sequence);
            self.record_pending_eviction_failure(
                if self.pending_decryption_total > global_limit {
                    PendingQueueLimit::Global
                } else {
                    PendingQueueLimit::PerPeer
                },
                sender,
                &incoming_message_id,
                "post-insert hard-bound check failed; rolled back enqueue",
            );
            self.record_pending_drop(
                PendingQueueDropReason::OverflowDropNewest,
                Some(if self.pending_decryption_total > global_limit {
                    PendingQueueLimit::Global
                } else {
                    PendingQueueLimit::PerPeer
                }),
                sender,
                &incoming_message_id,
            );
        }
    }

    /// Processes encrypted messages that were received before the session was established.
    ///
    /// This handles the case where encrypted messages arrive before the Welcome message.
    /// After the session is confirmed (via Welcome), we re-process these queued messages.
    pub(super) fn process_pending_decryption(&mut self, sender: &str) {
        let now = Instant::now();
        let _ = self.prune_expired_pending_for_peer(sender, now);

        let messages = match self.pending_decryption.remove(sender) {
            Some(msgs) => msgs,
            None => return,
        };

        if messages.is_empty() {
            return;
        }

        let drained: Vec<PendingDecryptMessage> = messages.into_iter().collect();
        let drained_count = drained.len();
        for entry in &drained {
            self.pending_decryption_live_sequences
                .remove(&entry.sequence);
            self.pending_decryption_total = self.pending_decryption_total.saturating_sub(1);
        }
        self.update_pending_peer_gauge(sender);
        self.update_pending_queue_current_gauge();
        self.cleanup_global_order_front();
        self.verify_pending_queue_invariants("process_pending_decryption_drain");

        info!(
            sender = %sender,
            count = drained_count,
            "Processing pending encrypted messages"
        );

        for entry in drained {
            let msg = entry.message;
            if let Some(result) = self.process_internal_message(&msg) {
                match result {
                    InternalMessageResult::Decrypted(content) => {
                        let mut decrypted_msg = msg.clone();
                        decrypted_msg.content = content.clone();
                        decrypted_msg
                            .metadata
                            .insert("encrypted".to_string(), "true".to_string());
                        decrypted_msg
                            .metadata
                            .insert("delayed_decrypt".to_string(), "true".to_string());

                        self.lamport_clock.merge(decrypted_msg.lamport_clock);
                        self.persist_lamport_clock();

                        if let Ok(mut state) = lock_shared_state(&self.shared_state) {
                            state.received_messages.push(decrypted_msg.clone());
                            let event = Event::MessageReceived {
                                message_id: decrypted_msg.id.as_str().to_string(),
                                sender: decrypted_msg.sender.as_str().to_string(),
                                recipient: decrypted_msg.recipient.as_str().to_string(),
                                content,
                                hop_count: decrypted_msg.hop_count.value(),
                                transport: "delayed".to_string(),
                                timestamp: Utc::now().timestamp_millis(),
                                lamport_clock: decrypted_msg.lamport_clock.value(),
                                reply_to_msg: decrypted_msg
                                    .reply_to_msg
                                    .as_ref()
                                    .map(|id| id.as_str().to_string()),
                                content_type: decrypted_msg.content_type.to_string(),
                                media_metadata: decrypted_msg.media_metadata.clone(),
                            };
                            state.emit_event(event);
                        }

                        debug!(message_id = %msg.id, "Processed delayed encrypted message");
                    }
                    InternalMessageResult::Consumed => {
                        debug!(message_id = %msg.id, "Delayed message was consumed internally");
                    }
                    InternalMessageResult::SecurityRejected => {
                        debug!(message_id = %msg.id, "Delayed message was rejected by security gate");
                    }
                }
            }
        }
    }
}
