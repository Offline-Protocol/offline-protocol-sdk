//! Type definitions, constants, and shared state for the protocol engine.

use crate::events::{Event, EventCallback, PresenceStatus};
use crate::Error;
use chrono::{DateTime, Utc};
use offline_protocol_core::{ContentType, MediaMetadata, Message, MessageId, MessagePriority};
use offline_protocol_transport::TransportType;
use serde::{Deserialize, Serialize};
use std::collections::HashSet;
use std::sync::{Arc, Mutex};
use std::time::Instant;

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
/// Minimum interval between session reconciliation scans (list_sessions I/O).
/// Keeps the expensive Keychain/Keystore I/O out of the hot path so that
/// sendMessage() is not blocked by Mutex contention on every process tick.
pub(crate) const RECONCILIATION_THROTTLE_MS: u64 = 2_000;
/// Lamport clock ticks between storage persistence writes. Avoids a
/// Keychain/Keystore write on every sent and received message. On crash
/// recovery, at most this many ticks are lost, which is safe — the clock
/// is only used for causal ordering and the gap is absorbed on the next
/// merge with any peer.
pub(crate) const LAMPORT_PERSIST_INTERVAL: u64 = 64;
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

/// Maximum number of blocked users to retain.
pub(crate) const MAX_BLOCKED_USERS: usize = 10_000;

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
    /// When `true`, the sender has reset their MLS session state (e.g. after
    /// unblocking) and the receiver should discard any existing session for
    /// this peer before establishing a new one.
    #[serde(default)]
    pub(crate) session_reset: bool,
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
    #[serde(skip_serializing_if = "Option::is_none")]
    pub(crate) group_name: Option<String>,
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
    /// Key type for persisted blocked user entries.
    pub const BLOCKED_USERS: &str = "blocked_users";
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
    /// Sender of the file transfer (used to drain partial transfers on block).
    pub(crate) sender: String,
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
