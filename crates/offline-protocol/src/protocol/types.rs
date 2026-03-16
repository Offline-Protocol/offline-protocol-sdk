//! Type definitions, constants, and shared state for the protocol engine.

use crate::config::{OverflowPolicy, PendingQueueConfig};
use crate::events::{Event, EventCallback, PresenceStatus};
use crate::Error;
use chrono::{DateTime, Utc};
use offline_protocol_core::{ContentType, MediaMetadata, Message, MessageId, MessagePriority};
use offline_protocol_transport::TransportType;
use serde::{Deserialize, Serialize};
use std::collections::{HashMap, HashSet, VecDeque};
use std::sync::{Arc, Mutex};
use std::time::{Duration as StdDuration, Instant};
use tracing::{debug, warn};

/// Retry interval for persisting session confirmation after a transient storage error.
pub(crate) const CONFIRMATION_RETRY_INTERVAL_SECS: i64 = 5;
/// Probe interval for reconciling pending sessions after restart.
pub(crate) const CONFIRMATION_PROBE_INTERVAL_SECS: i64 = 5;
/// Number of welcome retry records processed per tick.
pub(crate) const WELCOME_RETRY_BATCH_SIZE: usize = 20;
/// Hard TTL for outbound welcome lifecycle records.
pub(crate) const WELCOME_LIFECYCLE_TTL_SECS: i64 = 300;
/// Jitter ratio applied to welcome retry backoff delays.
pub(crate) const WELCOME_RETRY_JITTER_RATIO: f64 = 0.2;
/// Timeout waiting for explicit internet send confirmation for welcome.
pub(crate) const WELCOME_INTERNET_CONFIRM_TIMEOUT_SECS: i64 = 10;
pub(crate) const PENDING_TTL_SPIKE_WARN_THRESHOLD: usize = 25;
pub(crate) const PENDING_PEER_PRESSURE_WARN_EVERY: u32 = 10;
pub(crate) const PENDING_DROP_WARN_EVERY: u64 = 100;
pub(crate) const PENDING_EVICTION_FAILURE_WARN_EVERY: u64 = 10;
pub(crate) const MEDIA_TRANSFER_STALE_TIMEOUT_SECS: u64 = 300;
/// Maximum number of tracked known peers for service discovery.
pub(crate) const MAX_KNOWN_PEERS: usize = 1000;

/// Metadata key for the Ed25519 signature over the control message content (base64).
pub(crate) const CTRL_SIG_META_KEY: &str = "__ctrl_sig";
/// Metadata key for the sender's Ed25519 public key (base64, 32 bytes raw).
pub(crate) const CTRL_PK_META_KEY: &str = "__ctrl_pk";

/// Domain separator prepended to the canonical signing payload.
///
/// Prevents cross-context signature reuse: a signature produced for control
/// messages cannot be replayed in a future protocol extension that reuses the
/// same MLS identity key but with a different domain separator.
pub(crate) const CTRL_SIGN_DOMAIN: &[u8] = b"offline-ctrl-v1";

/// Maximum number of TOFU-pinned peer public keys to retain.
///
/// Entries are persisted via `MlsStorage` (when available) so pinned keys
/// survive process restarts and prevent key-substitution during re-pinning.
///
/// // TODO(security): There is no mechanism for legitimate key rotation. A peer
/// // who re-initializes MLS (getting a new identity key) will be permanently
/// // rejected by all peers who have TOFU-pinned the old key. Implement a key
/// // rotation protocol (e.g. signed key-update messages) or a manual TOFU
/// // reset API.
pub(crate) const MAX_TOFU_PEERS: usize = 1000;

/// Minimum age (in milliseconds) a TOFU entry must have before it can be
/// evicted by LRU. This prevents a cache-filling attack where an adversary
/// rapidly registers many fake identities to evict legitimate pinned keys.
///
/// Set to 1 hour.
pub(crate) const TOFU_MIN_EVICTION_AGE_MS: i64 = 3_600_000;

/// Entry in the TOFU key store, pairing the peer's public key with a
/// last-seen timestamp used for LRU eviction.
#[derive(Clone, Debug, Serialize, Deserialize)]
pub(crate) struct TofuEntry {
    pub(crate) public_key: Vec<u8>,
    /// Milliseconds since epoch (UTC) when we last verified a signed message
    /// from this peer.
    pub(crate) last_seen_ms: i64,
}

/// Payload for key package exchange.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub(crate) struct KeyPackagePayload {
    /// User ID of the key package owner.
    pub(crate) user_id: String,
    /// Raw key package data.
    pub(crate) key_package_data: Vec<u8>,
    /// Remaining valid lifetime in milliseconds (relative, not absolute).
    /// Receiver applies this to their local clock, avoiding clock skew issues.
    #[serde(default)]
    pub(crate) remaining_lifetime_ms: u64,
    /// Legacy absolute timestamp field — ignored on receive, kept for
    /// backward compatibility with old nodes that may still send it.
    #[serde(default)]
    pub(crate) timestamp_ms: u64,
}

/// Payload for a connection request message.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub(crate) struct ConnectionRequestPayload {
    /// Display name of the sender.
    pub(crate) sender_name: String,
    /// Timestamp of the request (Unix ms).
    pub(crate) timestamp_ms: i64,
    /// Optional MLS key package data for encrypted session setup.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub(crate) key_package: Option<Vec<u8>>,
}

/// Payload for a connection accepted message.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub(crate) struct ConnectionAcceptedPayload {
    /// Display name of the accepting party.
    pub(crate) accepted_by_name: String,
    /// Timestamp of the acceptance (Unix ms).
    #[serde(default)]
    pub(crate) timestamp_ms: i64,
    /// Optional MLS key package data for encrypted session setup.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub(crate) key_package: Option<Vec<u8>>,
}

// --- Presence, typing, and read receipt payloads ---

/// Maximum number of message IDs allowed in a single read receipt.
pub(crate) const MAX_READ_RECEIPT_IDS: usize = 256;

/// Payload for a presence update message.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub(crate) struct PresencePayload {
    /// Presence status.
    pub(crate) status: PresenceStatus,
    /// Timestamp of the update (Unix ms).
    pub(crate) timestamp_ms: i64,
}

/// Payload for a typing indicator message.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub(crate) struct TypingIndicatorPayload {
    /// Conversation identifier (recipient username for DMs, group_id for groups).
    pub(crate) conversation_id: String,
    /// Whether the user is currently typing.
    pub(crate) is_typing: bool,
    /// Timestamp of the indicator (Unix ms).
    pub(crate) timestamp_ms: i64,
}

/// Payload for a read receipt message.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub(crate) struct ReadReceiptPayload {
    /// IDs of the messages that were read.
    pub(crate) message_ids: Vec<String>,
    /// Timestamp when the messages were read (Unix ms).
    pub(crate) timestamp_ms: i64,
}

// --- Group (relay) payloads ---

#[derive(Debug, Clone, Serialize, Deserialize)]
pub(crate) struct GroupCreatedPayload {
    pub(crate) group_id: String,
    pub(crate) name: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub(crate) struct GroupMessageReceivedPayload {
    pub(crate) group_id: String,
    pub(crate) sender: String,
    pub(crate) content: String,
    pub(crate) timestamp: String,
    pub(crate) message_id: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub(crate) reply_to_msg: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub(crate) struct GroupMemberAddedPayload {
    pub(crate) group_id: String,
    pub(crate) user_id: String,
    pub(crate) added_by: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub(crate) struct GroupMemberRemovedPayload {
    pub(crate) group_id: String,
    pub(crate) user_id: String,
    pub(crate) removed_by: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub(crate) struct GroupInfoMemberPayload {
    pub(crate) user_id: String,
    pub(crate) role: String,
    pub(crate) joined_at: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub(crate) struct GroupInfoPayload {
    pub(crate) group_id: String,
    pub(crate) name: String,
    pub(crate) created_by: String,
    pub(crate) created_at: String,
    pub(crate) members: Vec<GroupInfoMemberPayload>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub(crate) struct UserGroupSummaryPayload {
    pub(crate) group_id: String,
    pub(crate) name: String,
    pub(crate) created_at: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub(crate) struct UserGroupsPayload {
    pub(crate) groups: Vec<UserGroupSummaryPayload>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub(crate) struct GroupErrorPayload {
    pub(crate) reason: String,
}

/// A received key package awaiting use for session creation.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub(crate) struct ReceivedKeyPackage {
    /// Raw MLS key package bytes.
    pub(crate) key_package_data: Vec<u8>,
    /// Local wall-clock deadline (ms since epoch) computed from the sender's
    /// `remaining_lifetime_ms`, anchored to *our* clock at receive time.
    pub(crate) local_expires_at_ms: u64,
}

/// Result of processing an internal protocol message.
pub(crate) enum InternalMessageResult {
    /// Message was consumed internally (don't surface to app).
    Consumed,
    /// Message was rejected by the security gate (spoofed sender, bad
    /// signature, TOFU violation, etc.). Like `Consumed`, the message is not
    /// surfaced to the app — but unlike `Consumed`, a delivery ACK must NOT
    /// be sent back, to avoid confirming to the attacker that the target is
    /// online and processing messages.
    SecurityRejected,
    /// Message was decrypted, here's the plaintext.
    Decrypted(String),
}

/// Pending message waiting for session establishment.
#[derive(Clone, Serialize, Deserialize)]
pub(crate) struct PendingMessage {
    /// Original plaintext content.
    pub(crate) content: String,
    /// Message priority.
    pub(crate) priority: MessagePriority,
    /// Message ID (preserved from initial creation).
    pub(crate) message_id: MessageId,
    /// Reply-to message ID if applicable.
    pub(crate) reply_to_msg: Option<MessageId>,
    /// When the message was queued (for future TTL/expiry support).
    pub(crate) queued_at: DateTime<Utc>,
}

#[derive(Clone)]
pub(crate) struct PendingDecryptMessage {
    pub(crate) peer_id: String,
    pub(crate) message_id: String,
    pub(crate) received_at: Instant,
    pub(crate) sequence: u64,
    pub(crate) message: Message,
}

#[derive(Clone)]
pub(crate) struct PendingDecryptEntryRef {
    pub(crate) peer_id: String,
    pub(crate) message_id: String,
    pub(crate) sequence: u64,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum PendingQueueLimit {
    PerPeer,
    Global,
}

impl PendingQueueLimit {
    pub(crate) fn as_str(self) -> &'static str {
        match self {
            Self::PerPeer => "per_peer",
            Self::Global => "global",
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum PendingQueueDropReason {
    OverflowDropOldest,
    OverflowDropNewest,
    TtlExpired,
}

impl PendingQueueDropReason {
    pub(crate) fn as_str(self) -> &'static str {
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
    /// Current per-peer pending queue sizes.
    pub pending_messages_per_peer: HashMap<String, usize>,
}

/// Encapsulates the bounded pending decryption queue: encrypted messages
/// received before the MLS session is ready, with per-peer and global limits,
/// TTL expiration, and configurable overflow policies.
#[derive(Default)]
pub(crate) struct PendingDecryptionQueue {
    /// Per-peer FIFO queues of encrypted messages.
    pub(crate) pending_decryption: HashMap<String, VecDeque<PendingDecryptMessage>>,
    /// Global insertion order index for deterministic global oldest eviction.
    pub(crate) pending_decryption_global_order: VecDeque<PendingDecryptEntryRef>,
    /// Live sequence IDs currently present in pending queues.
    pub(crate) pending_decryption_live_sequences: HashSet<u64>,
    /// Current number of pending encrypted messages across all peers.
    pub(crate) pending_decryption_total: usize,
    /// Monotonic sequence assigned on enqueue for deterministic tie-breaking.
    pending_decryption_next_sequence: u64,
    /// Pending queue observability counters and gauges.
    pub(crate) metrics: PendingQueueMetrics,
    /// Overflow hit count per peer for warning signal emission.
    pending_peer_overflow_hits: HashMap<String, u32>,
    /// Drop warning counters used for log-rate limiting by reason/limit.
    pending_drop_warning_counters: HashMap<String, u64>,
}

impl PendingDecryptionQueue {
    fn ttl(config: &PendingQueueConfig) -> StdDuration {
        StdDuration::from_millis(config.pending_ttl_ms)
    }

    fn update_peer_gauge(&mut self, peer_id: &str) {
        let peer_len = self
            .pending_decryption
            .get(peer_id)
            .map(VecDeque::len)
            .unwrap_or(0);
        if peer_len == 0 {
            self.metrics.pending_messages_per_peer.remove(peer_id);
        } else {
            self.metrics
                .pending_messages_per_peer
                .insert(peer_id.to_string(), peer_len);
        }
    }

    fn update_current_gauge(&mut self) {
        self.metrics.pending_messages_current = self.pending_decryption_total;
    }

    fn next_sequence(&mut self) -> u64 {
        let seq = self.pending_decryption_next_sequence;
        self.pending_decryption_next_sequence =
            self.pending_decryption_next_sequence.wrapping_add(1);
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

    fn remove_entry_by_sequence(
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
        self.update_current_gauge();

        if queue_empty {
            self.pending_decryption.remove(peer_id);
        }
        self.update_peer_gauge(peer_id);
        Some(removed)
    }

    fn record_drop(
        &mut self,
        reason: PendingQueueDropReason,
        limit_triggered: Option<PendingQueueLimit>,
        peer_id: &str,
        message_id: &str,
        overflow_policy: OverflowPolicy,
    ) {
        if matches!(
            reason,
            PendingQueueDropReason::OverflowDropOldest | PendingQueueDropReason::TtlExpired
        ) {
            self.metrics.pending_messages_evicted_total = self
                .metrics
                .pending_messages_evicted_total
                .saturating_add(1);
        }

        if matches!(
            reason,
            PendingQueueDropReason::OverflowDropOldest | PendingQueueDropReason::OverflowDropNewest
        ) {
            self.metrics.pending_messages_dropped_overflow_total = self
                .metrics
                .pending_messages_dropped_overflow_total
                .saturating_add(1);
        }

        if reason == PendingQueueDropReason::TtlExpired {
            self.metrics.pending_messages_expired_total = self
                .metrics
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
            overflow_policy = ?overflow_policy,
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

    fn record_eviction_failure(
        &mut self,
        limit_triggered: PendingQueueLimit,
        peer_id: &str,
        message_id: &str,
        detail: &str,
    ) {
        self.metrics.pending_messages_eviction_failures_total = self
            .metrics
            .pending_messages_eviction_failures_total
            .saturating_add(1);

        let count = self.metrics.pending_messages_eviction_failures_total;
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

    fn verify_invariants(&mut self, context: &str) {
        let per_peer_sum: usize = self.pending_decryption.values().map(VecDeque::len).sum();
        let live_count = self.pending_decryption_live_sequences.len();
        let current_gauge = self.metrics.pending_messages_current;
        let total = self.pending_decryption_total;
        let valid = per_peer_sum == total && live_count == total && current_gauge == total;
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
            .pending_peer_overflow_hits
            .entry(peer_id.to_string())
            .or_insert(0);
        *hits = hits.saturating_add(1);
        if *hits % PENDING_PEER_PRESSURE_WARN_EVERY == 0 {
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
        reason: PendingQueueDropReason,
        limit_triggered: PendingQueueLimit,
        overflow_policy: OverflowPolicy,
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
        if let Some(evicted) = self.remove_entry_by_sequence(&entry_ref.peer_id, entry_ref.sequence)
        {
            self.record_drop(
                reason,
                Some(limit_triggered),
                &evicted.peer_id,
                &evicted.message_id,
                overflow_policy,
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

    /// Prunes expired entries for a specific peer. Returns the count of expired entries.
    pub(crate) fn prune_expired_for_peer(
        &mut self,
        config: &PendingQueueConfig,
        peer_id: &str,
        now: Instant,
    ) -> usize {
        let mut expired_count = 0usize;
        let mut expired_sequences = Vec::new();
        let mut expired_ids = Vec::new();
        if let Some(queue) = self.pending_decryption.get(peer_id) {
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
            if self.remove_entry_by_sequence(peer_id, sequence).is_some() {
                self.record_drop(
                    PendingQueueDropReason::TtlExpired,
                    None,
                    peer_id,
                    &message_id,
                    overflow_policy,
                );
                expired_count = expired_count.saturating_add(1);
            }
        }

        if expired_count >= PENDING_TTL_SPIKE_WARN_THRESHOLD {
            warn!(
                peer_id = %peer_id,
                expired = expired_count,
                ttl_ms = config.pending_ttl_ms,
                "Pending encrypted message TTL eviction spike"
            );
        }

        self.cleanup_global_order_front();
        expired_count
    }

    /// Prunes expired entries from the front of the global order. Returns the count evicted.
    pub(crate) fn prune_expired_global_front(
        &mut self,
        config: &PendingQueueConfig,
        now: Instant,
        max_evictions: usize,
    ) -> usize {
        let mut evicted = 0usize;
        let overflow_policy = config.overflow_policy;
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

            if !Self::is_entry_expired(config, entry, now) {
                break;
            }

            if let Some(expired) = self.remove_entry_by_sequence(&front.peer_id, front.sequence) {
                self.pending_decryption_global_order.pop_front();
                self.record_drop(
                    PendingQueueDropReason::TtlExpired,
                    None,
                    &expired.peer_id,
                    &expired.message_id,
                    overflow_policy,
                );
                evicted = evicted.saturating_add(1);
            } else {
                self.pending_decryption_global_order.pop_front();
            }
        }

        if evicted >= PENDING_TTL_SPIKE_WARN_THRESHOLD {
            warn!(
                expired = evicted,
                ttl_ms = config.pending_ttl_ms,
                "Pending encrypted message TTL eviction spike"
            );
        }

        evicted
    }

    /// Enqueues an encrypted message that arrived before the MLS session was ready.
    pub(crate) fn enqueue(&mut self, config: &PendingQueueConfig, sender: &str, message: &Message) {
        self.metrics.pending_messages_received_total = self
            .metrics
            .pending_messages_received_total
            .saturating_add(1);
        let incoming_message_id = message.id.as_str();

        let now = Instant::now();
        let _ = self.prune_expired_for_peer(config, sender, now);
        let _ = self.prune_expired_global_front(config, now, 64);

        let per_peer_limit = config.max_pending_per_peer;
        let global_limit = config.max_pending_global;
        let overflow_policy = config.overflow_policy;

        let peer_len = self
            .pending_decryption
            .get(sender)
            .map(VecDeque::len)
            .unwrap_or(0);
        if peer_len >= per_peer_limit {
            self.record_peer_overflow_pressure(sender, per_peer_limit);
            match overflow_policy {
                OverflowPolicy::DropNewest => {
                    self.record_drop(
                        PendingQueueDropReason::OverflowDropNewest,
                        Some(PendingQueueLimit::PerPeer),
                        sender,
                        &incoming_message_id,
                        overflow_policy,
                    );
                    return;
                }
                OverflowPolicy::DropOldest => {
                    let evicted_sequence = self
                        .pending_decryption
                        .get(sender)
                        .and_then(|queue| queue.front().map(|entry| entry.sequence));
                    let mut evicted_any = false;
                    if let Some(sequence) = evicted_sequence {
                        if let Some(evicted) = self.remove_entry_by_sequence(sender, sequence) {
                            self.record_drop(
                                PendingQueueDropReason::OverflowDropOldest,
                                Some(PendingQueueLimit::PerPeer),
                                &evicted.peer_id,
                                &evicted.message_id,
                                overflow_policy,
                            );
                            evicted_any = true;
                        }
                    }
                    if !evicted_any {
                        self.record_eviction_failure(
                            PendingQueueLimit::PerPeer,
                            sender,
                            &incoming_message_id,
                            "drop_oldest failed to evict per-peer oldest",
                        );
                        self.record_drop(
                            PendingQueueDropReason::OverflowDropNewest,
                            Some(PendingQueueLimit::PerPeer),
                            sender,
                            &incoming_message_id,
                            overflow_policy,
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
            self.record_eviction_failure(
                PendingQueueLimit::PerPeer,
                sender,
                &incoming_message_id,
                "per-peer limit still saturated after eviction",
            );
            self.record_drop(
                PendingQueueDropReason::OverflowDropNewest,
                Some(PendingQueueLimit::PerPeer),
                sender,
                &incoming_message_id,
                overflow_policy,
            );
            return;
        }

        if self.pending_decryption_total >= global_limit {
            warn!(
                queue_size = self.pending_decryption_total,
                global_limit, "Pending encrypted queue at global pressure limit"
            );
            match overflow_policy {
                OverflowPolicy::DropNewest => {
                    self.record_drop(
                        PendingQueueDropReason::OverflowDropNewest,
                        Some(PendingQueueLimit::Global),
                        sender,
                        &incoming_message_id,
                        overflow_policy,
                    );
                    return;
                }
                OverflowPolicy::DropOldest => {
                    while self.pending_decryption_total >= global_limit {
                        if !self.evict_global_oldest(
                            PendingQueueDropReason::OverflowDropOldest,
                            PendingQueueLimit::Global,
                            overflow_policy,
                        ) {
                            self.record_eviction_failure(
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
            self.record_drop(
                PendingQueueDropReason::OverflowDropNewest,
                Some(PendingQueueLimit::Global),
                sender,
                &incoming_message_id,
                overflow_policy,
            );
            return;
        }

        let sequence = self.next_sequence();
        let entry = PendingDecryptMessage {
            peer_id: sender.to_string(),
            message_id: incoming_message_id.clone(),
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
        self.update_peer_gauge(sender);
        self.update_current_gauge();
        self.cleanup_global_order_front();
        self.verify_invariants("enqueue");

        let peer_len_post_insert = self
            .pending_decryption
            .get(sender)
            .map(VecDeque::len)
            .unwrap_or(0);
        if self.pending_decryption_total > global_limit || peer_len_post_insert > per_peer_limit {
            let _ = self.remove_entry_by_sequence(sender, sequence);
            self.record_eviction_failure(
                if self.pending_decryption_total > global_limit {
                    PendingQueueLimit::Global
                } else {
                    PendingQueueLimit::PerPeer
                },
                sender,
                &incoming_message_id,
                "post-insert hard-bound check failed; rolled back enqueue",
            );
            self.record_drop(
                PendingQueueDropReason::OverflowDropNewest,
                Some(if self.pending_decryption_total > global_limit {
                    PendingQueueLimit::Global
                } else {
                    PendingQueueLimit::PerPeer
                }),
                sender,
                &incoming_message_id,
                overflow_policy,
            );
        }
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

        let messages = match self.pending_decryption.remove(sender) {
            Some(msgs) => msgs,
            None => return Vec::new(),
        };

        if messages.is_empty() {
            return Vec::new();
        }

        let drained: Vec<PendingDecryptMessage> = messages.into_iter().collect();
        for entry in &drained {
            self.pending_decryption_live_sequences
                .remove(&entry.sequence);
            self.pending_decryption_total = self.pending_decryption_total.saturating_sub(1);
        }
        self.update_peer_gauge(sender);
        self.update_current_gauge();
        self.cleanup_global_order_front();
        self.verify_invariants("drain_for_peer");

        drained
    }
}

/// Durable state for a peer MLS session.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub(crate) enum SessionState {
    Pending,
    Confirmed,
}

impl SessionState {
    pub(crate) fn as_str(&self) -> &'static str {
        match self {
            Self::Pending => "Pending",
            Self::Confirmed => "Confirmed",
        }
    }
}

/// Durable lifecycle states for outbound Welcome delivery.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub(crate) enum WelcomeDeliveryState {
    Created,
    SendAttempted,
    Sent,
    Failed,
    Expired,
}

impl WelcomeDeliveryState {
    pub(crate) fn as_str(&self) -> &'static str {
        match self {
            Self::Created => "Created",
            Self::SendAttempted => "SendAttempted",
            Self::Sent => "Sent",
            Self::Failed => "Failed",
            Self::Expired => "Expired",
        }
    }
}

/// Durable metadata for outbound Welcome reliability handling.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub(crate) struct WelcomeLifecycleRecord {
    pub(crate) peer_id: String,
    pub(crate) group_id: String,
    pub(crate) state: WelcomeDeliveryState,
    pub(crate) attempt: u32,
    pub(crate) welcome_message: Message,
    pub(crate) next_retry_at: Option<DateTime<Utc>>,
    pub(crate) last_reason_code: Option<crate::events::WelcomeReasonCode>,
    pub(crate) last_transport_error: Option<String>,
    pub(crate) created_at: DateTime<Utc>,
    pub(crate) expires_at: DateTime<Utc>,
}

/// Storage key types for message persistence.
pub(crate) mod storage_keys {
    /// Key type for pending encrypted messages.
    pub const PENDING_MESSAGES: &str = "pending_messages";
    /// Key type for persisted per-peer MLS session confirmation state.
    pub const SESSION_STATES: &str = "session_states";
    /// Key type for persisted per-peer received key packages (survives restart).
    pub const PEER_KEY_PACKAGES: &str = "peer_key_packages";
    /// Key type for persisted per-peer outbound welcome lifecycle state.
    pub const WELCOME_LIFECYCLES: &str = "welcome_lifecycles";
    /// Key type for the Lamport clock value.
    pub const LAMPORT_CLOCK: &str = "lamport_clock";
    /// Key ID for the single Lamport clock entry.
    pub const LAMPORT_CLOCK_ID: &str = "current";
    /// Key type for persisted TOFU (Trust-On-First-Use) peer public keys.
    pub const TOFU_KEYS: &str = "tofu_keys";
}

/// Protocol state.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ProtocolState {
    /// Protocol is not started.
    Stopped,
    /// Protocol is running.
    Running,
    /// Protocol is paused (background mode).
    Paused,
}

/// Shared state protected by mutex.
pub(crate) struct SharedState {
    /// Current protocol state.
    pub(crate) state: ProtocolState,

    /// Event handlers registered by the application.
    pub(crate) event_handlers: Vec<EventCallback>,

    /// Received messages queue.
    pub(crate) received_messages: Vec<Message>,
}

impl SharedState {
    pub(crate) fn new() -> Self {
        Self {
            state: ProtocolState::Stopped,
            event_handlers: Vec::new(),
            received_messages: Vec::new(),
        }
    }

    pub(crate) fn emit_event(&self, event: Event) {
        for handler in &self.event_handlers {
            handler(event.clone());
        }
    }
}

/// Helper function to lock a mutex and convert poison errors to protocol errors.
pub(crate) fn lock_shared_state(
    state: &Arc<Mutex<SharedState>>,
) -> std::result::Result<std::sync::MutexGuard<'_, SharedState>, Error> {
    state
        .lock()
        .map_err(|_| Error::Other("Shared state mutex poisoned".to_string()))
}

#[derive(Clone)]
pub(crate) struct OutboxEntry {
    pub(crate) message: Message,
    pub(crate) attempt_count: u32,
    pub(crate) first_sent_at: DateTime<Utc>,
    pub(crate) last_sent_at: DateTime<Utc>,
    pub(crate) last_transport: Option<TransportType>,
}

#[derive(Clone)]
pub(crate) struct PendingMediaMetadataEntry {
    pub(crate) content_type: ContentType,
    pub(crate) media_metadata: Option<MediaMetadata>,
    pub(crate) last_updated_at: Instant,
}

#[derive(Clone)]
pub(crate) struct OutboundMediaTransfer {
    pub(crate) content_type: ContentType,
    pub(crate) recipient: String,
    pub(crate) pinned_transport: TransportType,
    pub(crate) total_chunks: u32,
    pub(crate) delivered_chunks: HashSet<u32>,
    pub(crate) last_updated_at: Instant,
    pub(crate) media_metadata: Option<MediaMetadata>,
}

pub(crate) enum OutboundSendPreparation {
    Ready(String),
    Queued(MessageId),
}
