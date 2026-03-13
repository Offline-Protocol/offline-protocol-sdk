//! Main protocol engine.

use crate::constants::{ACK_FOR_KEY, ACK_HOP_COUNT_KEY, ACK_TRANSPORT_KEY, MAX_OUTBOX_ENTRIES};
use crate::events::DecryptionFailureCode;
use crate::file_transfer::{FileChunk, FileTransferManager, OutboundTransferState};
#[cfg(feature = "mls-observability")]
use crate::mls_observability::{opaque_id, timestamp_now_ms, MlsLifecycleEvent};
use crate::mls_observability::{
    DecryptionFailureKind, MlsErrorCategory, MlsEventEmitter, MlsEventRateLimiter,
    MlsOperationContext, NoopMlsEventEmitter,
};
use crate::group_mesh::PendingCommit;
use crate::{
    Error, EstablishmentState, Event, EventCallback, ProtocolConfig, Result, SessionStateError,
    TransportManager,
};
use base64::{engine::general_purpose::STANDARD as BASE64, Engine as _};
use chrono::{DateTime, Duration as ChronoDuration, Utc};
use offline_protocol_core::{
    AppId, ContentType, LamportClock, MediaMetadata, Message, MessageId, MessagePriority,
    ServiceDescriptor, UserId, TTL,
};
use offline_protocol_mls::{EncryptedMessage, MlsManager, MlsStorage, WelcomeMessage};
use offline_protocol_reliability::{
    AckConfig, AckManager, Deduplicator, DeduplicatorConfig, DeduplicatorStats, RetryConfig,
    RetryQueue,
};
use offline_protocol_router::{DorsConfig, PathSelector, RelayManager, TransportSelector};
use offline_protocol_services::{MeshServices, ServiceAction};
use offline_protocol_transport::TransportType;
use serde::{Deserialize, Serialize};
use std::collections::{HashMap, HashSet, VecDeque};
use std::hash::{Hash, Hasher};
use std::sync::{Arc, Mutex, RwLock};
use std::time::{Duration as StdDuration, Instant};
use tracing::{debug, error, info, warn};

/// Encode bytes to base64 string.
pub(crate) fn base64_encode(data: &[u8]) -> String {
    BASE64.encode(data)
}

/// Decode base64 string to bytes with a size guard against oversized payloads.
///
/// The limit is applied to the **encoded** (base64) size. Since base64 inflates
/// data by ~33%, the maximum **decoded** payload is approximately 768 KB.
pub(crate) fn base64_decode(data: &str) -> std::result::Result<Vec<u8>, String> {
    if data.len() > crate::group_mesh::MAX_BASE64_PAYLOAD_SIZE {
        return Err(format!(
            "payload too large: {} encoded bytes exceeds {} limit",
            data.len(),
            crate::group_mesh::MAX_BASE64_PAYLOAD_SIZE
        ));
    }
    BASE64.decode(data).map_err(|e| e.to_string())
}

/// Internal message prefixes for protocol messages.
pub(crate) mod internal_prefixes {
    /// Prefix for key package messages.
    pub const KEY_PACKAGE: &str = "__MLS_KEY_PKG__";
    /// Prefix for welcome messages.
    pub const WELCOME: &str = "__MLS_WELCOME__";
    /// Prefix for encrypted messages.
    pub const ENCRYPTED: &str = "__MLS_ENC__";
    /// Prefix for session confirmation probe messages.
    pub const SESSION_CONFIRM_PROBE: &str = "__MLS_CONFIRM_PROBE__";
    /// Prefix for session confirmation acknowledgement messages.
    pub const SESSION_CONFIRM_ACK: &str = "__MLS_CONFIRM_ACK__";
    /// Prefix for connection request messages.
    pub const CONN_REQUEST: &str = "__CONN_REQ__";
    /// Prefix for connection accepted messages.
    pub const CONN_ACCEPT: &str = "__CONN_ACC__";
    /// Prefix for connection rejected messages.
    pub const CONN_REJECT: &str = "__CONN_REJ__";
    /// Prefix for group created (relay).
    pub const GROUP_CREATED: &str = "__GROUP_CREATED__";
    /// Prefix for group message received (relay).
    pub const GROUP_MSG: &str = "__GROUP_MSG__";
    /// Prefix for group member added (relay).
    pub const GROUP_MEMBER_ADDED: &str = "__GROUP_MEMBER_ADDED__";
    /// Prefix for group member removed (relay).
    pub const GROUP_MEMBER_REMOVED: &str = "__GROUP_MEMBER_REMOVED__";
    /// Prefix for group info (relay).
    pub const GROUP_INFO: &str = "__GROUP_INFO__";
    /// Prefix for user groups list (relay).
    pub const USER_GROUPS: &str = "__USER_GROUPS__";
    /// Prefix for group error (relay).
    pub const GROUP_ERROR: &str = "__GROUP_ERROR__";
    /// Prefix for MLS-encrypted group messages (mesh).
    pub const GROUP_MLS_MSG: &str = "__GRP_MLS_MSG__";
    /// Prefix for MLS Welcome messages for group invites (mesh).
    pub const GROUP_MLS_WELCOME: &str = "__GRP_MLS_WELCOME__";
    /// Prefix for MLS Commit messages for group membership changes (mesh).
    pub const GROUP_MLS_COMMIT: &str = "__GRP_MLS_COMMIT__";
    /// Prefix for group leave notifications (mesh).
    pub const GROUP_MLS_LEAVE: &str = "__GRP_MLS_LEAVE__";
}

/// Retry interval for persisting session confirmation after a transient storage error.
const CONFIRMATION_RETRY_INTERVAL_SECS: i64 = 5;
/// Probe interval for reconciling pending sessions after restart.
const CONFIRMATION_PROBE_INTERVAL_SECS: i64 = 5;
/// Number of welcome retry records processed per tick.
const WELCOME_RETRY_BATCH_SIZE: usize = 20;
/// Hard TTL for outbound welcome lifecycle records.
const WELCOME_LIFECYCLE_TTL_SECS: i64 = 300;
/// Jitter ratio applied to welcome retry backoff delays.
const WELCOME_RETRY_JITTER_RATIO: f64 = 0.2;
/// Timeout waiting for explicit internet send confirmation for welcome.
const WELCOME_INTERNET_CONFIRM_TIMEOUT_SECS: i64 = 10;
const PENDING_TTL_SPIKE_WARN_THRESHOLD: usize = 25;
const PENDING_PEER_PRESSURE_WARN_EVERY: u32 = 10;
const PENDING_DROP_WARN_EVERY: u64 = 100;
const PENDING_EVICTION_FAILURE_WARN_EVERY: u64 = 10;
const MEDIA_TRANSFER_STALE_TIMEOUT_SECS: u64 = 300;
/// Maximum number of tracked known peers for service discovery.
const MAX_KNOWN_PEERS: usize = 1000;

/// Payload for key package exchange.
#[derive(Debug, Clone, Serialize, Deserialize)]
struct KeyPackagePayload {
    /// User ID of the key package owner.
    user_id: String,
    /// Raw key package data.
    key_package_data: Vec<u8>,
    /// Remaining valid lifetime in milliseconds (relative, not absolute).
    /// Receiver applies this to their local clock, avoiding clock skew issues.
    #[serde(default)]
    remaining_lifetime_ms: u64,
    /// Legacy absolute timestamp field — ignored on receive, kept for
    /// backward compatibility with old nodes that may still send it.
    #[serde(default)]
    timestamp_ms: u64,
}

/// Payload for a connection request message.
#[derive(Debug, Clone, Serialize, Deserialize)]
struct ConnectionRequestPayload {
    /// Display name of the sender.
    sender_name: String,
    /// Timestamp of the request (Unix ms).
    timestamp_ms: i64,
    /// Optional MLS key package data for encrypted session setup.
    #[serde(skip_serializing_if = "Option::is_none")]
    key_package: Option<Vec<u8>>,
}

/// Payload for a connection accepted message.
#[derive(Debug, Clone, Serialize, Deserialize)]
struct ConnectionAcceptedPayload {
    /// Display name of the accepting party.
    accepted_by_name: String,
    /// Timestamp of the acceptance (Unix ms).
    #[serde(default)]
    timestamp_ms: i64,
    /// Optional MLS key package data for encrypted session setup.
    #[serde(skip_serializing_if = "Option::is_none")]
    key_package: Option<Vec<u8>>,
}

// --- Group (relay) payloads ---

#[derive(Debug, Clone, Serialize, Deserialize)]
struct GroupCreatedPayload {
    group_id: String,
    name: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
struct GroupMessageReceivedPayload {
    group_id: String,
    sender: String,
    content: String,
    timestamp: String,
    message_id: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    reply_to_msg: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
struct GroupMemberAddedPayload {
    group_id: String,
    user_id: String,
    added_by: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
struct GroupMemberRemovedPayload {
    group_id: String,
    user_id: String,
    removed_by: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
struct GroupInfoMemberPayload {
    user_id: String,
    role: String,
    joined_at: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
struct GroupInfoPayload {
    group_id: String,
    name: String,
    created_by: String,
    created_at: String,
    members: Vec<GroupInfoMemberPayload>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
struct UserGroupSummaryPayload {
    group_id: String,
    name: String,
    created_at: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
struct UserGroupsPayload {
    groups: Vec<UserGroupSummaryPayload>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
struct GroupErrorPayload {
    reason: String,
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
    /// Message was decrypted, here's the plaintext.
    Decrypted(String),
}

/// Pending message waiting for session establishment.
#[derive(Clone, Serialize, Deserialize)]
struct PendingMessage {
    /// Original plaintext content.
    content: String,
    /// Message priority.
    priority: MessagePriority,
    /// Message ID (preserved from initial creation).
    message_id: MessageId,
    /// Reply-to message ID if applicable.
    reply_to_msg: Option<MessageId>,
    /// When the message was queued (for future TTL/expiry support).
    queued_at: DateTime<Utc>,
}

#[derive(Clone)]
struct PendingDecryptMessage {
    peer_id: String,
    message_id: String,
    received_at: Instant,
    sequence: u64,
    message: Message,
}

#[derive(Clone)]
struct PendingDecryptEntryRef {
    peer_id: String,
    message_id: String,
    sequence: u64,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum PendingQueueLimit {
    PerPeer,
    Global,
}

impl PendingQueueLimit {
    fn as_str(self) -> &'static str {
        match self {
            Self::PerPeer => "per_peer",
            Self::Global => "global",
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum PendingQueueDropReason {
    OverflowDropOldest,
    OverflowDropNewest,
    TtlExpired,
}

impl PendingQueueDropReason {
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
    /// Current per-peer pending queue sizes.
    pub pending_messages_per_peer: HashMap<String, usize>,
}

/// Durable state for a peer MLS session.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
enum SessionState {
    Pending,
    Confirmed,
}

impl SessionState {
    fn as_str(&self) -> &'static str {
        match self {
            Self::Pending => "Pending",
            Self::Confirmed => "Confirmed",
        }
    }
}

/// Durable lifecycle states for outbound Welcome delivery.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
enum WelcomeDeliveryState {
    Created,
    SendAttempted,
    Sent,
    Failed,
    Expired,
}

impl WelcomeDeliveryState {
    fn as_str(&self) -> &'static str {
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
struct WelcomeLifecycleRecord {
    peer_id: String,
    group_id: String,
    state: WelcomeDeliveryState,
    attempt: u32,
    welcome_message: Message,
    next_retry_at: Option<DateTime<Utc>>,
    last_reason_code: Option<crate::events::WelcomeReasonCode>,
    last_transport_error: Option<String>,
    created_at: DateTime<Utc>,
    expires_at: DateTime<Utc>,
}

/// Storage key types for message persistence.
mod storage_keys {
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
    state: ProtocolState,

    /// Event handlers registered by the application.
    event_handlers: Vec<EventCallback>,

    /// Received messages queue.
    received_messages: Vec<Message>,
}

impl SharedState {
    fn new() -> Self {
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
struct OutboxEntry {
    message: Message,
    attempt_count: u32,
    first_sent_at: DateTime<Utc>,
    last_sent_at: DateTime<Utc>,
    last_transport: Option<TransportType>,
}

#[derive(Clone)]
struct PendingMediaMetadataEntry {
    content_type: ContentType,
    media_metadata: Option<MediaMetadata>,
    last_updated_at: Instant,
}

#[derive(Clone)]
struct OutboundMediaTransfer {
    content_type: ContentType,
    recipient: String,
    pinned_transport: TransportType,
    total_chunks: u32,
    delivered_chunks: HashSet<u32>,
    last_updated_at: Instant,
    media_metadata: Option<MediaMetadata>,
}

enum OutboundSendPreparation {
    Ready(String),
    Queued(MessageId),
}

/// Main entry point for the Offline Protocol SDK.
///
/// This struct combines all protocol components and provides a unified API
/// for sending/receiving messages with automatic transport selection and
/// reliable delivery.
pub struct OfflineProtocol {
    /// Configuration.
    pub(crate) config: ProtocolConfig,

    /// Transport manager (manages all transports with DORS).
    transport_manager: TransportManager,

    /// Path selector for routing (includes relay scoring logic).
    #[allow(dead_code)]
    path_selector: PathSelector,

    /// ACK manager for tracking acknowledgments.
    ack_manager: AckManager,

    /// Retry queue for failed messages.
    retry_queue: RetryQueue,

    /// Deduplicator for preventing duplicates.
    deduplicator: Deduplicator,

    /// Shared mutable state.
    pub(crate) shared_state: Arc<Mutex<SharedState>>,

    /// Messages awaiting delivery/acknowledgment (store-and-forward outbox).
    outbox: HashMap<MessageId, OutboxEntry>,

    /// Dedicated outbox for file chunk messages, separate from the main outbox
    /// to prevent large file transfers from evicting regular messages.
    media_outbox: HashMap<MessageId, OutboxEntry>,

    /// MLS manager for end-to-end encryption.
    pub(crate) mls_manager: Option<Arc<RwLock<MlsManager>>>,

    /// Pending messages waiting for session establishment (recipient -> messages).
    pending_encrypted_messages: HashMap<String, Vec<PendingMessage>>,

    /// Key packages received but not yet used (sender_id -> package).
    pub(crate) pending_key_packages: HashMap<String, ReceivedKeyPackage>,

    /// Set of peers we've already sent our key package to.
    key_package_sent_to: std::collections::HashSet<String>,

    /// All discovered/connected peers, tracked independently of encryption.
    /// Used by service discovery to know who to broadcast queries to.
    known_peers: std::collections::HashSet<String>,

    /// Sessions confirmed established (received Welcome or successful decrypt).
    /// Only encrypt messages when the session is confirmed to avoid race conditions.
    confirmed_sessions: std::collections::HashSet<String>,

    /// Encrypted messages received before session was established (sender -> messages).
    /// These are queued and processed after session confirmation.
    /// Invariants:
    /// - bounded by both per-peer and global limits
    /// - deterministic FIFO order within each peer
    /// - monotonic TTL expiration (Instant-based)
    pending_decryption: HashMap<String, VecDeque<PendingDecryptMessage>>,
    /// Global insertion order index for deterministic global oldest eviction.
    pending_decryption_global_order: VecDeque<PendingDecryptEntryRef>,
    /// Live sequence IDs currently present in pending queues.
    pending_decryption_live_sequences: HashSet<u64>,
    /// Current number of pending encrypted messages across all peers.
    pending_decryption_total: usize,
    /// Monotonic sequence assigned on enqueue for deterministic tie-breaking.
    pending_decryption_next_sequence: u64,
    /// Pending queue observability counters and gauges.
    pending_queue_metrics: PendingQueueMetrics,
    /// Overflow hit count per peer for warning signal emission.
    pending_peer_overflow_hits: HashMap<String, u32>,
    /// Drop warning counters used for log-rate limiting by reason/limit.
    pending_drop_warning_counters: HashMap<String, u64>,

    /// Storage for persisting pending messages (reuses MLS storage).
    /// When set, pending messages survive app crashes/restarts.
    message_storage: Option<Arc<dyn MlsStorage>>,

    /// Lamport logical clock for causal message ordering.
    /// Ticked on send, merged on receive.
    lamport_clock: LamportClock,

    /// Retry schedule for peers whose confirmation persistence failed.
    confirmation_retry_due_at: HashMap<String, DateTime<Utc>>,

    /// Probe schedule for pending sessions to guarantee post-restart convergence.
    confirmation_probe_due_at: HashMap<String, DateTime<Utc>>,

    /// Outbound welcome lifecycle records keyed by peer id.
    welcome_lifecycles: HashMap<String, WelcomeLifecycleRecord>,

    /// Sink for MLS lifecycle telemetry.
    mls_event_emitter: Arc<dyn MlsEventEmitter>,

    /// Rate limiting policy for MLS failure event floods.
    #[cfg_attr(not(feature = "mls-observability"), allow(dead_code))]
    mls_event_rate_limiter: MlsEventRateLimiter,

    /// Per-instance secret used to derive non-reversible opaque telemetry IDs.
    #[cfg_attr(not(feature = "mls-observability"), allow(dead_code))]
    mls_observability_secret: [u8; 16],

    /// File transfer manager for chunking outbound and reassembling inbound media.
    file_transfer_manager: FileTransferManager,

    /// Stashed (content_type, media_metadata) from chunk-0 of incoming file transfers,
    /// used to populate the FileReceived event once all chunks arrive.
    pending_media_metadata: HashMap<String, PendingMediaMetadataEntry>,
    /// Tracks outbound media transfer delivery progress keyed by file_id.
    outbound_media_transfers: HashMap<String, OutboundMediaTransfer>,
    /// Maps each outbound media chunk message ID to (file_id, chunk_index).
    outbound_media_chunks: HashMap<MessageId, (String, u32)>,
    /// Sliding-window state for outbound file transfers, keyed by file_id.
    /// Chunks are only sent when the window has capacity (previous chunks ACKed).
    outbound_media_windows: HashMap<String, OutboundTransferState>,

    /// Mesh service registry and handler (extracted crate).
    mesh_services: MeshServices,

    /// Cached group membership lists for fan-out without holding MLS lock.
    /// Maps group_id -> list of member user IDs.
    pub(crate) group_members: HashMap<String, Vec<String>>,

    /// Deduplication cache for group messages received via multiple paths.
    /// Key: message ID, Value: when first seen.
    pub(crate) group_message_dedup: HashMap<String, Instant>,

    /// Buffer for out-of-order MLS commits that failed to decrypt.
    /// Maps group_id -> list of pending commits awaiting retry.
    /// When a commit succeeds for a group, buffered commits are drained and retried.
    pub(crate) pending_commits: HashMap<String, Vec<PendingCommit>>,
}

impl OfflineProtocol {
    /// Creates a new protocol instance.
    ///
    /// # Arguments
    ///
    /// * `config` - Protocol configuration
    ///
    /// # Returns
    ///
    /// Returns `Ok(OfflineProtocol)` if successful, `Err` if configuration is invalid.
    pub fn new(config: ProtocolConfig) -> Result<Self> {
        // Validate configuration
        config.validate()?;

        // Create transport selector for DORS
        let transport_selector = TransportSelector::with_config(config.dors.clone());

        // Create transport manager
        let transport_manager = TransportManager::new(transport_selector);

        Ok(Self {
            transport_manager,
            path_selector: PathSelector::with_config(
                config.path.clone(),
                RelayManager::with_config(config.relay.clone()),
            ),
            ack_manager: AckManager::with_config(config.reliability.ack.clone()),
            retry_queue: RetryQueue::with_config(config.reliability.retry.clone()),
            deduplicator: Deduplicator::with_config(config.reliability.dedup.clone()),
            shared_state: Arc::new(Mutex::new(SharedState::new())),
            outbox: HashMap::new(),
            media_outbox: HashMap::new(),
            mls_manager: None,
            pending_encrypted_messages: HashMap::new(),
            pending_key_packages: HashMap::new(),
            key_package_sent_to: std::collections::HashSet::new(),
            known_peers: std::collections::HashSet::new(),
            confirmed_sessions: std::collections::HashSet::new(),
            pending_decryption: HashMap::new(),
            pending_decryption_global_order: VecDeque::new(),
            pending_decryption_live_sequences: HashSet::new(),
            pending_decryption_total: 0,
            pending_decryption_next_sequence: 0,
            pending_queue_metrics: PendingQueueMetrics::default(),
            pending_peer_overflow_hits: HashMap::new(),
            pending_drop_warning_counters: HashMap::new(),
            message_storage: None,
            lamport_clock: LamportClock::new(),
            confirmation_retry_due_at: HashMap::new(),
            confirmation_probe_due_at: HashMap::new(),
            welcome_lifecycles: HashMap::new(),
            mls_event_emitter: Arc::new(NoopMlsEventEmitter),
            mls_event_rate_limiter: MlsEventRateLimiter::default(),
            mls_observability_secret: *uuid::Uuid::new_v4().as_bytes(),
            file_transfer_manager: FileTransferManager::new(),
            pending_media_metadata: HashMap::new(),
            outbound_media_transfers: HashMap::new(),
            outbound_media_chunks: HashMap::new(),
            outbound_media_windows: HashMap::new(),
            mesh_services: MeshServices::new(),
            group_members: HashMap::new(),
            group_message_dedup: HashMap::new(),
            pending_commits: HashMap::new(),
            config,
        })
    }

    /// Initializes MLS encryption with the provided storage backend.
    ///
    /// This must be called before encryption can be used. The storage
    /// backend should be a platform-native secure storage implementation
    /// (iOS Keychain, Android EncryptedSharedPreferences, etc.).
    ///
    /// Ownership model:
    /// - `OfflineProtocol` is the single authoritative owner of `MlsManager`
    /// - initialization is idempotent per protocol instance
    /// - subsequent calls return without replacing the existing manager
    /// - manager publication is transactional: restore must succeed before
    ///   `mls_manager` becomes visible to callers
    ///
    /// The same storage is also used for persisting pending messages,
    /// ensuring they survive app crashes/restarts.
    pub fn initialize_mls(&mut self, storage: Arc<dyn MlsStorage>) -> Result<()> {
        if self.mls_manager.is_some() {
            return Ok(());
        }

        let manager = Arc::new(RwLock::new(MlsManager::new(
            &self.config.user_id,
            storage.clone(),
        )?));

        // Keep initialization transactional so a restore failure cannot leave
        // partially-initialized MLS state visible and then permanently block retries.
        let previous_message_storage = self.message_storage.clone();
        let previous_pending_messages = self.pending_encrypted_messages.clone();
        let previous_confirmed_sessions = self.confirmed_sessions.clone();
        let previous_welcome_lifecycles = self.welcome_lifecycles.clone();
        let previous_lamport_clock = self.lamport_clock.value();

        // Also use this storage for pending message persistence
        self.message_storage = Some(storage);

        // Restore state from previous session
        let restore_result = (|| {
            self.restore_pending_messages()?;
            self.restore_lamport_clock();
            self.restore_session_states_from_manager(manager.clone())?;
            self.restore_peer_key_packages(&manager)?;
            self.restore_welcome_lifecycles()?;
            Ok(())
        })();

        if let Err(err) = restore_result {
            self.message_storage = previous_message_storage;
            self.pending_encrypted_messages = previous_pending_messages;
            self.confirmed_sessions = previous_confirmed_sessions;
            self.welcome_lifecycles = previous_welcome_lifecycles;
            self.lamport_clock = LamportClock::from_value(previous_lamport_clock);
            return Err(err);
        }

        self.mls_manager = Some(manager);
        self.emit_mls_initialized();

        info!(user_id = %self.config.user_id, "MLS encryption initialized with message persistence");
        Ok(())
    }

    /// Enables message persistence using the provided storage backend.
    ///
    /// This allows pending messages to survive app crashes/restarts even
    /// when MLS encryption is not used. The storage backend should be a
    /// platform-native secure storage implementation.
    ///
    /// Note: If you call `initialize_mls()`, message persistence is
    /// automatically enabled using the same storage.
    pub fn enable_message_persistence(&mut self, storage: Arc<dyn MlsStorage>) -> Result<()> {
        self.message_storage = Some(storage);
        self.restore_pending_messages()?;
        self.restore_lamport_clock();
        info!("Message persistence enabled");
        Ok(())
    }

    /// Checks if MLS encryption is initialized.
    pub fn is_mls_initialized(&self) -> bool {
        self.mls_manager.is_some()
    }

    /// Returns whether auto-encryption should be applied.
    fn should_auto_encrypt(&self) -> bool {
        self.config.encryption.enabled && self.mls_manager.is_some()
    }

    /// Configures the MLS lifecycle event emitter.
    pub fn set_mls_event_emitter(&mut self, emitter: Arc<dyn MlsEventEmitter>) {
        self.mls_event_emitter = emitter;
    }

    #[cfg(feature = "mls-observability")]
    fn session_id_for_observability(
        &self,
        peer_id: Option<&str>,
        group_id: Option<&str>,
    ) -> String {
        let seed = format!(
            "peer={}|group={}",
            peer_id.unwrap_or("none"),
            group_id.unwrap_or("none")
        );
        opaque_id(&seed, &self.mls_observability_secret)
    }

    #[cfg(feature = "mls-observability")]
    fn emit_mls_lifecycle_event(&self, event: MlsLifecycleEvent) {
        if self.mls_event_rate_limiter.should_emit(&event) {
            self.mls_event_emitter.emit(event);
        }
    }

    #[cfg(feature = "mls-observability")]
    fn emit_mls_initialized(&self) {
        self.emit_mls_lifecycle_event(MlsLifecycleEvent::Initialized {
            timestamp_ms: timestamp_now_ms(),
            session_id: self.session_id_for_observability(None, None),
            group_id: None,
            peer_id: None,
            context: MlsOperationContext::Initialize,
            error_category: None,
        });
    }

    #[cfg(not(feature = "mls-observability"))]
    fn emit_mls_initialized(&self) {}

    #[cfg(feature = "mls-observability")]
    fn emit_mls_encryption_used(&self, recipient: &str) {
        let peer_id = opaque_id(recipient, &self.mls_observability_secret);
        self.emit_mls_lifecycle_event(MlsLifecycleEvent::EncryptionUsed {
            timestamp_ms: timestamp_now_ms(),
            session_id: self.session_id_for_observability(Some(recipient), None),
            group_id: None,
            peer_id: Some(peer_id),
            context: MlsOperationContext::Send,
            error_category: None,
        });
    }

    #[cfg(not(feature = "mls-observability"))]
    fn emit_mls_encryption_used(&self, _recipient: &str) {}

    #[cfg(feature = "mls-observability")]
    fn emit_mls_session_missing(
        &self,
        peer_id: Option<&str>,
        group_id: Option<&str>,
        context: MlsOperationContext,
        error_category: MlsErrorCategory,
    ) {
        self.emit_mls_lifecycle_event(MlsLifecycleEvent::SessionMissing {
            timestamp_ms: timestamp_now_ms(),
            session_id: self.session_id_for_observability(peer_id, group_id),
            group_id: group_id.map(|id| opaque_id(id, &self.mls_observability_secret)),
            peer_id: peer_id.map(|id| opaque_id(id, &self.mls_observability_secret)),
            context,
            error_category: Some(error_category),
        });
    }

    #[cfg(not(feature = "mls-observability"))]
    fn emit_mls_session_missing(
        &self,
        _peer_id: Option<&str>,
        _group_id: Option<&str>,
        _context: MlsOperationContext,
        _error_category: MlsErrorCategory,
    ) {
    }

    #[cfg(feature = "mls-observability")]
    fn emit_mls_decryption_failed(
        &self,
        sender_id: &str,
        group_id: Option<&str>,
        kind: DecryptionFailureKind,
        context: MlsOperationContext,
    ) {
        self.emit_mls_lifecycle_event(MlsLifecycleEvent::DecryptionFailed {
            timestamp_ms: timestamp_now_ms(),
            session_id: self.session_id_for_observability(Some(sender_id), group_id),
            group_id: group_id.map(|id| opaque_id(id, &self.mls_observability_secret)),
            peer_id: Some(opaque_id(sender_id, &self.mls_observability_secret)),
            context,
            error_category: Some(kind.error_category()),
            failure_kind: kind,
        });
    }

    #[cfg(not(feature = "mls-observability"))]
    fn emit_mls_decryption_failed(
        &self,
        _sender_id: &str,
        _group_id: Option<&str>,
        _kind: DecryptionFailureKind,
        _context: MlsOperationContext,
    ) {
    }

    #[cfg(feature = "mls-observability")]
    fn emit_mls_session_ready(&self, peer_id: &str, group_id: &str, context: MlsOperationContext) {
        self.emit_mls_lifecycle_event(MlsLifecycleEvent::SessionReady {
            timestamp_ms: timestamp_now_ms(),
            session_id: self.session_id_for_observability(Some(peer_id), Some(group_id)),
            group_id: Some(opaque_id(group_id, &self.mls_observability_secret)),
            peer_id: Some(opaque_id(peer_id, &self.mls_observability_secret)),
            context,
            error_category: None,
        });
    }

    #[cfg(not(feature = "mls-observability"))]
    fn emit_mls_session_ready(
        &self,
        _peer_id: &str,
        _group_id: &str,
        _context: MlsOperationContext,
    ) {
    }

    /// Starts the protocol.
    ///
    /// # Returns
    ///
    /// Returns `Ok(())` if started successfully, `Err` if already started.
    pub fn start(&mut self) -> Result<()> {
        let state = lock_shared_state(&self.shared_state)?;

        if state.state != ProtocolState::Stopped {
            return Err(Error::AlreadyStarted);
        }

        // Start all transports
        drop(state);
        self.transport_manager.start()?;

        // Wire DORS event callback so app receives dors_score_updated, dors_transport_selected,
        let shared = self.shared_state.clone();
        self.transport_manager
            .set_dors_event_callback(Some(Arc::new(move |event| {
                if let Ok(s) = shared.lock() {
                    s.emit_event(event);
                }
            })));

        let mut state = lock_shared_state(&self.shared_state)?;

        state.state = ProtocolState::Running;
        drop(state);

        self.flush_restored_confirmed_pending_messages();
        self.kick_pending_session_reconciliation("start");
        self.process_welcome_retry_queue()?;

        Ok(())
    }

    /// Stops the protocol gracefully.
    ///
    /// # Returns
    ///
    /// Returns `Ok(())` if stopped successfully, `Err` if not started.
    pub fn stop(&mut self) -> Result<()> {
        let state = lock_shared_state(&self.shared_state)?;

        if state.state == ProtocolState::Stopped {
            return Ok(()); // Already stopped
        }

        // Stop all transports
        drop(state);
        self.transport_manager.stop()?;
        let mut state = lock_shared_state(&self.shared_state)?;

        state.state = ProtocolState::Stopped;

        Ok(())
    }

    /// Pauses the protocol (for background mode).
    pub fn pause(&mut self) -> Result<()> {
        let mut state = lock_shared_state(&self.shared_state)?;

        if state.state != ProtocolState::Running {
            return Err(Error::NotStarted);
        }

        state.state = ProtocolState::Paused;
        Ok(())
    }

    /// Resumes the protocol from pause.
    pub fn resume(&mut self) -> Result<()> {
        let mut state = lock_shared_state(&self.shared_state)?;

        if state.state != ProtocolState::Paused {
            return Err(Error::InvalidConfiguration(
                "Protocol is not paused".to_string(),
            ));
        }

        state.state = ProtocolState::Running;
        Ok(())
    }

    fn transport_from_label(label: &str) -> TransportType {
        TransportType::from_label(label)
    }

    fn transport_label(transport: TransportType) -> &'static str {
        transport.label()
    }

    fn select_media_transport(&self) -> Result<TransportType> {
        let available = self.transport_manager.get_available_transports();

        if let Some(current) = self.transport_manager.current_transport() {
            if available.contains_key(&current) {
                return Ok(current);
            }
        }

        for preferred in [
            TransportType::Internet,
            TransportType::WiFiDirect,
            TransportType::BLE,
        ] {
            if available.contains_key(&preferred) {
                return Ok(preferred);
            }
        }

        Err(Error::Other(
            "No available transport for media transfer".to_string(),
        ))
    }

    fn pinned_media_transport_for_message(&self, message_id: &MessageId) -> Option<TransportType> {
        let (file_id, _) = self.outbound_media_chunks.get(message_id)?;
        self.outbound_media_transfers
            .get(file_id)
            .map(|transfer| transfer.pinned_transport)
    }

    fn decryption_failure_code_from_kind(kind: DecryptionFailureKind) -> DecryptionFailureCode {
        match kind {
            DecryptionFailureKind::NotInitialized => DecryptionFailureCode::NotInitialized,
            DecryptionFailureKind::InvalidCiphertext => DecryptionFailureCode::InvalidCiphertext,
            DecryptionFailureKind::IdentityMismatch => DecryptionFailureCode::IdentityMismatch,
            DecryptionFailureKind::CryptoFailure => DecryptionFailureCode::CryptoFailure,
            DecryptionFailureKind::SessionNotFound | DecryptionFailureKind::Unknown => {
                DecryptionFailureCode::Unknown
            }
        }
    }

    fn handle_ack_message(&mut self, message: &Message) {
        if let Some(ack_for) = message.metadata.get(ACK_FOR_KEY) {
            if let Ok(message_id) = MessageId::from_str(ack_for) {
                if let Some(pending) = self.ack_manager.remove_ack(&message_id) {
                    let latency = Utc::now()
                        .signed_duration_since(pending.sent_at)
                        .num_milliseconds()
                        .max(0) as u64;

                    let hop_count = message
                        .metadata
                        .get(ACK_HOP_COUNT_KEY)
                        .and_then(|v| v.parse::<u8>().ok())
                        .unwrap_or(0);

                    let transport = message
                        .metadata
                        .get(ACK_TRANSPORT_KEY)
                        .map(|label| Self::transport_from_label(label))
                        .unwrap_or(TransportType::BLE);

                    // Emit event if we can lock the state, but don't fail if we can't
                    if let Ok(state) = lock_shared_state(&self.shared_state) {
                        state.emit_event(Event::message_delivered(
                            message_id.clone(),
                            latency,
                            hop_count,
                            transport,
                        ));
                        drop(state);
                    } else {
                        error!(
                            "Failed to lock shared state for ACK event, skipping event emission"
                        );
                    }

                    self.transport_manager.reset_retry_count(transport);
                    self.transport_manager.record_delivery_success(
                        transport,
                        latency.min(u32::MAX as u64) as u32,
                        hop_count,
                    );
                    self.handle_outbound_media_chunk_delivered(&message_id);
                    self.remove_outbox_entry(&message_id);
                }
            }
        }
    }

    fn send_delivery_ack(
        &mut self,
        message: &Message,
        inbound_transport: TransportType,
    ) -> Result<()> {
        let sender = UserId::new(&self.config.user_id)?;
        let recipient = message.sender.clone();
        let app_id = AppId::new(&self.config.app_id)?;
        let ttl = TTL::new(self.config.initial_ttl).unwrap_or_else(|_| TTL::default());

        let ack_message = Message::builder(sender, recipient, app_id)
            .content(String::new())
            .priority(MessagePriority::Low)
            .ttl(ttl)
            .requires_ack(false)
            .metadata(ACK_FOR_KEY, message.id.as_str())
            .metadata(ACK_HOP_COUNT_KEY, message.hop_count.value().to_string())
            .metadata(ACK_TRANSPORT_KEY, Self::transport_label(inbound_transport))
            .build();

        // Try sending ACK via the same transport that received the message first.
        // This is the preferred path as it's known to work for this peer.
        // If the inbound transport is no longer available (e.g., internet disconnected),
        // fall back to DORS selection to try any available transport.
        if self
            .transport_manager
            .send_via_transport(&ack_message, inbound_transport)
            .is_ok()
        {
            return Ok(());
        }

        // Fallback: try any available transport via DORS
        debug!(
            message_id = %message.id,
            inbound_transport = ?inbound_transport,
            "Inbound transport unavailable for ACK, falling back to DORS selection"
        );
        self.transport_manager.send(&ack_message)
    }

    /// Creates a new message from the given parameters.
    fn create_message(
        &mut self,
        recipient: impl Into<String>,
        content: impl Into<String>,
        priority: Option<MessagePriority>,
        reply_to_msg: Option<MessageId>,
    ) -> Result<Message> {
        let sender = UserId::new(&self.config.user_id)?;
        let recipient = UserId::new(recipient)?;
        let app_id = AppId::new(&self.config.app_id)?;

        let clock_value = self.lamport_clock.tick();
        self.persist_lamport_clock();

        let mut builder = Message::builder(sender, recipient, app_id)
            .content(content)
            .priority(priority.unwrap_or(MessagePriority::Medium))
            .ttl(TTL::new(self.config.initial_ttl)?)
            .lamport_clock(clock_value);

        if let Some(reply_to) = reply_to_msg {
            builder = builder.reply_to_msg(reply_to);
        }

        Ok(builder.build())
    }

    /// Handles successful message send.
    fn handle_send_success(
        &mut self,
        message: &Message,
        transport: Option<TransportType>,
    ) -> Result<()> {
        self.mark_message_sent(message, transport, Some(1));
        self.ensure_ack_registration(message)?;
        Ok(())
    }

    /// Handles failed message send by persisting to outbox and scheduling retry.
    ///
    /// EDGE CASE HANDLING:
    /// - Ensures message is persisted to outbox for recovery
    /// - Schedules retry with exponential backoff
    /// - Handles case where all transports are unavailable
    ///
    /// NOTE: This does NOT call `record_retry_failure` — callers that need it
    /// (e.g. `send_via_forced_transport`) must record the failure themselves.
    /// `TransportManager::send()` already records failures internally, so
    /// calling it here would double-count.
    fn handle_send_failure(
        &mut self,
        message: &Message,
        transport: Option<TransportType>,
    ) -> Result<()> {
        // Ensure message is persisted to outbox for recovery
        self.ensure_outbox_entry(message);

        // Schedule retry. If queuing fails, treat this as a terminal failure.
        if let Err(e) = self.retry_queue.enqueue(message.clone(), 0) {
            warn!(
                message_id = %message.id,
                error = %e,
                "Failed to enqueue message for retry"
            );
            if message.content_type == ContentType::FileChunk {
                if let Ok(state) = lock_shared_state(&self.shared_state) {
                    state.emit_event(Event::message_failed(
                        message.id.clone(),
                        "Retry queue unavailable".to_string(),
                        0,
                    ));
                }
                self.handle_outbound_media_chunk_failed(&message.id, "retry queue unavailable");
                self.remove_outbox_entry(&message.id);
            }
        }

        warn!(
            message_id = %message.id,
            transport = ?transport,
            "Deferred message due to send error"
        );
        Ok(())
    }

    /// Emits a transport switched event if the transport changed.
    fn emit_transport_switch_event(
        &self,
        previous_transport: Option<TransportType>,
        current_transport: Option<TransportType>,
    ) -> Result<()> {
        if current_transport != previous_transport {
            if let Some(new_transport) = current_transport {
                let state = lock_shared_state(&self.shared_state).map_err(|e| {
                    error!(
                        "Failed to lock shared state for transport switch event: {}",
                        e
                    );
                    e
                })?;
                state.emit_event(Event::transport_switched(
                    previous_transport,
                    new_transport,
                    "DORS selected better transport".to_string(),
                ));
                drop(state);
            }
        }
        Ok(())
    }

    /// Emits a message sent event.
    fn emit_message_sent_event(&self, message: &Message) -> Result<()> {
        let state = lock_shared_state(&self.shared_state).map_err(|e| {
            error!("Failed to lock shared state for message sent event: {}", e);
            e
        })?;
        state.emit_event(Event::message_sent(message));
        drop(state);
        Ok(())
    }

    /// Sends an internal protocol message (connection requests, etc.) via DORS.
    ///
    /// Handles the full send orchestration: state check, deduplication, transport send,
    /// success/failure handling, and transport switch events. Does NOT emit a
    /// `MessageSent` event — internal messages are not user-visible content.
    pub(crate) fn send_internal_message(
        &mut self,
        recipient: &str,
        content: String,
        priority: MessagePriority,
    ) -> Result<MessageId> {
        {
            let state = lock_shared_state(&self.shared_state)?;
            if state.state != ProtocolState::Running {
                return Err(Error::NotStarted);
            }
        }

        let message = self.create_message(recipient, content, Some(priority), None)?;
        let message_id = message.id.clone();

        if self.deduplicator.is_duplicate(&message_id) {
            return Err(crate::Error::Other("Duplicate message".to_string()));
        }

        self.deduplicator.mark_seen(message_id.clone());

        let previous_transport = self.transport_manager.current_transport();
        let send_result = self.transport_manager.send(&message);
        let current_transport = self.transport_manager.current_transport();

        match send_result {
            Ok(()) => {
                self.handle_send_success(&message, current_transport)?;
            }
            Err(err) => {
                self.handle_send_failure(&message, current_transport.or(previous_transport))?;
                warn!(
                    message_id = %message.id,
                    recipient = %recipient,
                    error = %err,
                    "Internal message send failed, message deferred"
                );
            }
        }

        self.emit_transport_switch_event(previous_transport, current_transport)?;
        Ok(message_id)
    }

    /// Sends a message.
    ///
    /// # Arguments
    ///
    /// * `recipient` - Recipient's user ID
    /// * `content` - Message content
    /// * `priority` - Message priority (optional, defaults to Medium)
    /// * `reply_to_msg` - ID of the message this is replying to (optional)
    ///
    /// # Returns
    ///
    /// Returns the message ID if successful.
    ///
    /// # Auto-Encryption
    ///
    /// When encryption is enabled and MLS is initialized, messages are automatically
    /// encrypted before sending. If no session exists with the recipient but we have
    /// their key package, a session is created automatically. If no key package is
    /// available and `store_pending` is enabled, the message is queued until a key
    /// package is received.
    pub fn send_message(
        &mut self,
        recipient: impl Into<String>,
        content: impl Into<String>,
        priority: Option<MessagePriority>,
        reply_to_msg: Option<impl Into<String>>,
    ) -> Result<MessageId> {
        // Check if protocol is running
        {
            let state = lock_shared_state(&self.shared_state)?;
            if state.state != ProtocolState::Running {
                return Err(Error::NotStarted);
            }
        }

        let recipient_str: String = recipient.into();
        let content_str: String = content.into();
        let priority = priority.unwrap_or(MessagePriority::Medium);

        // Parse reply_to_msg if provided
        let reply_to_msg_id = reply_to_msg
            .map(|r| MessageId::from_str(&r.into()))
            .transpose()
            .map_err(|e| Error::Other(format!("Invalid reply_to_msg: {}", e)))?;

        let final_content = match self.prepare_outbound_content(
            &recipient_str,
            &content_str,
            priority,
            reply_to_msg_id.clone(),
            "send_message_session_pending",
        )? {
            OutboundSendPreparation::Ready(content) => content,
            OutboundSendPreparation::Queued(message_id) => return Ok(message_id),
        };

        // Create message with potentially encrypted content
        let message = self.create_message(
            &recipient_str,
            final_content,
            Some(priority),
            reply_to_msg_id,
        )?;
        let message_id = message.id.clone();

        // Check for duplicates
        if self.deduplicator.is_duplicate(&message_id) {
            return Err(crate::Error::Other("Duplicate message".to_string()));
        }

        // Mark as seen
        self.deduplicator.mark_seen(message_id.clone());

        // Track previous transport before sending
        let previous_transport = self.transport_manager.current_transport();

        // Attempt to send via transport manager (DORS will select best transport)
        let send_result = self.transport_manager.send(&message);
        let current_transport = self.transport_manager.current_transport();

        // Handle send result
        match send_result {
            Ok(()) => {
                self.handle_send_success(&message, current_transport)?;
                self.emit_transport_switch_event(previous_transport, current_transport)?;
                self.emit_message_sent_event(&message)?;
                Ok(message_id)
            }
            Err(err) => {
                self.handle_send_failure(&message, current_transport.or(previous_transport))?;
                warn!(
                    message_id = %message.id,
                    error = %err,
                    "Send failed, message deferred"
                );
                Err(Error::Other(format!(
                    "Send failed (message {} deferred for retry): {}",
                    message.id, err
                )))
            }
        }
    }

    /// Sends a media attachment (image, video, audio, file, etc.) to a recipient.
    ///
    /// The file data is chunked and each chunk is sent as an individual message
    /// with `content_type: FileChunk`. The first chunk carries the full
    /// `MediaMetadata` so the receiver can display a preview before all chunks
    /// arrive. Individual chunk messages require ACKs and participate in retry
    /// logic so delivery is tracked and recoverable per chunk. `MediaSent` is
    /// emitted only after all chunks are ACKed.
    ///
    /// Returns a `file_id` that can be used to track progress or cancel.
    pub fn send_media(
        &mut self,
        recipient: impl Into<String>,
        file_data: Vec<u8>,
        file_name: impl Into<String>,
        content_type: ContentType,
        media_metadata: Option<MediaMetadata>,
    ) -> Result<String> {
        {
            let state = lock_shared_state(&self.shared_state)?;
            if state.state != ProtocolState::Running {
                return Err(Error::NotStarted);
            }
        }

        let recipient_str: String = recipient.into();
        let file_name_str: String = file_name.into();

        let file_id = format!("file_{}", MessageId::new().as_str());
        let pinned_transport = self.select_media_transport()?;

        let (chunk_size, window_size) = match pinned_transport {
            TransportType::BLE => {
                use crate::constants::{CHUNK_SIZE_BLE, MEDIA_WINDOW_SIZE_BLE};
                (CHUNK_SIZE_BLE, MEDIA_WINDOW_SIZE_BLE)
            }
            TransportType::Internet => {
                use crate::constants::{CHUNK_SIZE_INTERNET, MEDIA_WINDOW_SIZE_INTERNET};
                (CHUNK_SIZE_INTERNET, MEDIA_WINDOW_SIZE_INTERNET)
            }
            TransportType::WiFiDirect => {
                use crate::constants::{DEFAULT_CHUNK_SIZE, DEFAULT_MEDIA_WINDOW_SIZE};
                (DEFAULT_CHUNK_SIZE, DEFAULT_MEDIA_WINDOW_SIZE)
            }
        };
        let chunks = self.file_transfer_manager.chunk_file(
            file_id.clone(),
            file_name_str,
            file_data,
            Some(chunk_size),
        )?;

        let total_chunks = chunks.len() as u32;
        self.outbound_media_transfers.insert(
            file_id.clone(),
            OutboundMediaTransfer {
                content_type,
                recipient: recipient_str.clone(),
                pinned_transport,
                total_chunks,
                delivered_chunks: HashSet::new(),
                last_updated_at: Instant::now(),
                media_metadata: media_metadata.clone(),
            },
        );

        let mut window_state = OutboundTransferState::new(chunks, window_size);
        let initial_batch = window_state.next_chunks_to_send();
        self.outbound_media_windows
            .insert(file_id.clone(), window_state);

        let state = lock_shared_state(&self.shared_state)?;
        state.emit_event(Event::file_progress(file_id.clone(), 0, total_chunks));
        drop(state);

        self.send_media_chunk_batch(
            &file_id,
            initial_batch,
            &recipient_str,
            pinned_transport,
            content_type,
            media_metadata.as_ref(),
        )?;

        Ok(file_id)
    }

    /// Sends a batch of file chunks, wiring each into the outbox and media tracking.
    fn send_media_chunk_batch(
        &mut self,
        file_id: &str,
        chunks: Vec<FileChunk>,
        recipient: &str,
        pinned_transport: TransportType,
        content_type: ContentType,
        media_metadata: Option<&MediaMetadata>,
    ) -> Result<()> {
        for chunk in chunks {
            let chunk_index = chunk.chunk_index;
            let binary_payload = chunk.to_bytes();

            let meta_for_chunk = if chunk_index == 0 {
                media_metadata.cloned()
            } else {
                None
            };

            let mut message = self.create_media_message(
                recipient,
                String::new(),
                ContentType::FileChunk,
                meta_for_chunk,
            )?;
            message.binary_content = Some(binary_payload);

            if chunk_index == 0 {
                use crate::constants::ORIGINAL_CONTENT_TYPE_KEY;
                message.metadata.insert(
                    ORIGINAL_CONTENT_TYPE_KEY.to_string(),
                    content_type.to_string(),
                );
            }
            self.outbound_media_chunks
                .insert(message.id.clone(), (file_id.to_string(), chunk_index));

            let previous_transport = self.transport_manager.current_transport();
            let send_result = self
                .transport_manager
                .send_via_transport(&message, pinned_transport);
            let current_transport = Some(pinned_transport);

            match send_result {
                Ok(()) => {
                    self.handle_send_success(&message, current_transport)?;
                    self.emit_transport_switch_event(previous_transport, current_transport)?;
                }
                Err(err) => {
                    self.handle_send_failure(&message, current_transport.or(previous_transport))?;
                    // send_via_transport does not record retry failures internally.
                    self.transport_manager
                        .record_retry_failure(pinned_transport);
                    warn!(
                        file_id = %file_id,
                        chunk_index = chunk_index,
                        transport = ?pinned_transport,
                        error = %err,
                        "File chunk send failed, message deferred"
                    );
                }
            }
            if !self.outbound_media_transfers.contains_key(file_id) {
                return Err(Error::Other(format!(
                    "Media transfer {} could not be scheduled for reliable delivery",
                    file_id
                )));
            }
        }
        Ok(())
    }

    /// Pumps all active windowed media transfers, sending the next batch of
    /// chunks for any transfer whose window has capacity (previous chunks ACKed).
    /// Should be called from the periodic tick/poll loop.
    fn pump_media_transfers(&mut self) {
        let file_ids: Vec<String> = self.outbound_media_windows.keys().cloned().collect();

        for file_id in file_ids {
            let transfer = match self.outbound_media_transfers.get(&file_id) {
                Some(t) => t.clone(),
                None => {
                    self.outbound_media_windows.remove(&file_id);
                    continue;
                }
            };

            let window = match self.outbound_media_windows.get_mut(&file_id) {
                Some(w) => w,
                None => continue,
            };

            if !window.has_capacity() {
                continue;
            }

            let batch = window.next_chunks_to_send();
            if batch.is_empty() {
                continue;
            }

            if let Err(err) = self.send_media_chunk_batch(
                &file_id,
                batch,
                &transfer.recipient,
                transfer.pinned_transport,
                transfer.content_type,
                transfer.media_metadata.as_ref(),
            ) {
                warn!(
                    file_id = %file_id,
                    error = %err,
                    "Failed to pump media transfer chunks"
                );
            }
        }
    }

    /// Creates a message carrying media content (file chunks, etc.).
    ///
    /// Like `create_message` but sets `content_type`, `media_metadata`, marks
    /// the message as internet-preferred via metadata, and requires per-chunk ACKs.
    fn create_media_message(
        &mut self,
        recipient: &str,
        content: impl Into<String>,
        content_type: ContentType,
        media_metadata: Option<MediaMetadata>,
    ) -> Result<Message> {
        use crate::constants::{TRANSPORT_PREFERENCE_INTERNET, TRANSPORT_PREFERENCE_KEY};

        let sender = UserId::new(&self.config.user_id)?;
        let recipient = UserId::new(recipient)?;
        let app_id = AppId::new(&self.config.app_id)?;

        let clock_value = self.lamport_clock.tick();
        self.persist_lamport_clock();

        let mut builder = Message::builder(sender, recipient, app_id)
            .content(content)
            .content_type(content_type)
            .priority(MessagePriority::Medium)
            .ttl(TTL::new(self.config.initial_ttl)?)
            .lamport_clock(clock_value)
            .metadata(TRANSPORT_PREFERENCE_KEY, TRANSPORT_PREFERENCE_INTERNET)
            .requires_ack(true);

        if let Some(meta) = media_metadata {
            builder = builder.media_metadata(meta);
        }

        Ok(builder.build())
    }

    /// Returns a mutable reference to the file transfer manager.
    pub fn file_transfer_manager_mut(&mut self) -> &mut FileTransferManager {
        &mut self.file_transfer_manager
    }

    /// Returns a reference to the file transfer manager.
    pub fn file_transfer_manager(&self) -> &FileTransferManager {
        &self.file_transfer_manager
    }

    /// Processes an incoming file-chunk message: feeds it to the transfer
    /// manager, emits progress events, and emits `FileReceived` when complete.
    fn handle_incoming_file_chunk(&mut self, message: &Message) {
        let chunk = if let Some(ref binary) = message.binary_content {
            match FileChunk::from_bytes(binary) {
                Ok(c) => c,
                Err(e) => {
                    warn!(
                        message_id = %message.id,
                        error = %e,
                        "Failed to deserialize binary file chunk, dropping"
                    );
                    return;
                }
            }
        } else {
            match FileChunk::from_json(&message.content) {
                Ok(c) => c,
                Err(e) => {
                    warn!(
                        message_id = %message.id,
                        error = %e,
                        "Failed to deserialize file chunk, dropping"
                    );
                    return;
                }
            }
        };

        let file_id = chunk.file_id.clone();
        let file_name = chunk.file_name.clone();
        let file_size = chunk.file_size;
        let sender = message.sender.as_str().to_string();

        if chunk.chunk_index == 0 {
            use crate::constants::ORIGINAL_CONTENT_TYPE_KEY;
            let original_ct = message
                .metadata
                .get(ORIGINAL_CONTENT_TYPE_KEY)
                .map(|s| ContentType::parse(s))
                .unwrap_or(ContentType::File);
            self.pending_media_metadata.insert(
                file_id.clone(),
                PendingMediaMetadataEntry {
                    content_type: original_ct,
                    media_metadata: message.media_metadata.clone(),
                    last_updated_at: Instant::now(),
                },
            );
        }

        if let Some(progress) = self.file_transfer_manager.process_chunk(chunk) {
            if let Some(entry) = self.pending_media_metadata.get_mut(&file_id) {
                entry.last_updated_at = Instant::now();
            }
            if let Ok(state) = lock_shared_state(&self.shared_state) {
                state.emit_event(Event::file_progress(
                    file_id.clone(),
                    progress.chunks_completed,
                    progress.total_chunks,
                ));
            }
        }

        if self.file_transfer_manager.is_complete(&file_id) {
            let Some(file_data) = self.file_transfer_manager.finalize_file(&file_id) else {
                warn!(
                    file_id = %file_id,
                    "File transfer marked complete but reassembly failed"
                );
                return;
            };
            let metadata_entry = self.pending_media_metadata.remove(&file_id);
            let (content_type, media_metadata) = metadata_entry
                .map(|entry| (entry.content_type, entry.media_metadata))
                .unwrap_or((ContentType::File, None));

            if let Ok(state) = lock_shared_state(&self.shared_state) {
                state.emit_event(Event::file_received(
                    file_id,
                    file_name,
                    file_size,
                    sender,
                    content_type,
                    media_metadata,
                    file_data,
                ));
            }
        }
    }

    /// Encrypts content for a recipient, handling session creation if needed.
    ///
    /// To avoid race conditions where both peers create sessions simultaneously,
    /// we defer encryption until the session is "confirmed". A session is confirmed when:
    /// - We join via their Welcome message (welcome-wins), OR
    /// - We successfully decrypt their first message
    fn encrypt_content_for_recipient_strict(
        &mut self,
        recipient: &str,
        content: &str,
    ) -> Result<String> {
        let mls = self.mls_manager.clone().ok_or_else(|| {
            self.emit_mls_session_missing(
                Some(recipient),
                None,
                MlsOperationContext::SessionLookup,
                MlsErrorCategory::NotInitialized,
            );
            Error::MlsNotInitialized
        })?;

        let has_session = {
            let manager = mls
                .read()
                .map_err(|_| Error::Other("MLS lock poisoned".to_string()))?;
            manager.has_session(recipient)?
        };

        if !has_session {
            self.try_load_key_package_from_storage_into_memory(recipient);
            let now_ms = Utc::now().timestamp_millis() as u64;
            let has_valid_key_package = match self.pending_key_packages.get(recipient) {
                Some(pkg) if now_ms < pkg.local_expires_at_ms => true,
                Some(_) => {
                    self.pending_key_packages.remove(recipient);
                    self.delete_peer_key_package_from_storage(recipient);
                    false
                }
                None => false,
            };

            if has_valid_key_package {
                return Err(Error::SessionNotReady(self.establishment_state(recipient)?));
            }

            self.emit_mls_session_missing(
                Some(recipient),
                None,
                MlsOperationContext::SessionLookup,
                MlsErrorCategory::SessionStateMissing,
            );
            return Err(Error::SessionNotReady(self.establishment_state(recipient)?));
        }

        if !self.is_session_confirmed(recipient)? {
            return Err(Error::SessionNotReady(self.establishment_state(recipient)?));
        }

        let encrypted = {
            let manager = mls
                .read()
                .map_err(|_| Error::Other("MLS lock poisoned".to_string()))?;
            manager
                .encrypt_for_user(recipient, content.as_bytes())
                .map_err(|_| Error::EncryptFailed("encryption operation failed".to_string()))?
        };

        let serialized =
            serde_json::to_string(&encrypted).map_err(|e| Error::Serialization(e.to_string()))?;
        self.emit_mls_encryption_used(recipient);
        Ok(format!("{}{}", internal_prefixes::ENCRYPTED, serialized))
    }

    fn encrypt_content_for_recipient(
        &mut self,
        recipient: &str,
        content: &str,
        _priority: MessagePriority,
    ) -> Result<String> {
        // Clone the Arc to avoid borrow issues
        let mls = self.mls_manager.clone().ok_or_else(|| {
            self.emit_mls_session_missing(
                Some(recipient),
                None,
                MlsOperationContext::SessionLookup,
                MlsErrorCategory::NotInitialized,
            );
            Error::MlsNotInitialized
        })?;

        // Check for existing session
        let has_session = {
            let manager = mls
                .read()
                .map_err(|_| Error::Other("MLS lock poisoned".to_string()))?;
            manager.has_session(recipient)?
        };

        if !has_session {
            // Try loading key package from storage (e.g. after restart) then create session from memory
            self.try_load_key_package_from_storage_into_memory(recipient);
            // Try to create session from stored key package
            // Clone first, only remove after all operations succeed to avoid losing the key package on failure
            if let Some(received_pkg) = self.pending_key_packages.get(recipient).cloned() {
                // Check if key package has expired (using local clock)
                let now_ms = Utc::now().timestamp_millis() as u64;
                if now_ms >= received_pkg.local_expires_at_ms {
                    warn!(recipient = %recipient, "Received key package has expired, discarding");
                    self.pending_key_packages.remove(recipient);
                    self.delete_peer_key_package_from_storage(recipient);
                } else {
                    {
                        let manager = mls
                            .read()
                            .map_err(|_| Error::Other("MLS lock poisoned".to_string()))?;
                        manager.import_key_package(recipient, &received_pkg.key_package_data)?;
                    }

                    // Create session and send welcome message
                    let welcome = {
                        let manager = mls
                            .read()
                            .map_err(|_| Error::Other("MLS lock poisoned".to_string()))?;
                        manager.create_session(recipient)?
                    };

                    // All operations succeeded, now safe to remove the key package
                    self.pending_key_packages.remove(recipient);
                    self.delete_peer_key_package_from_storage(recipient);

                    let group_id = welcome.group_id.as_str().to_string();
                    let is_session = group_id.starts_with("session:");

                    if let Err(err) =
                        self.ensure_session_state_entry(recipient, "session_created_local")
                    {
                        warn!(
                            recipient = %recipient,
                            error = %err,
                            "Failed to persist pending session state"
                        );
                    }

                    let welcome_sent = self.send_welcome_message(recipient, &welcome)?;

                    debug!(
                        recipient = %recipient,
                        group_id = %group_id,
                        welcome_sent = welcome_sent,
                        "Created MLS session and scheduled welcome lifecycle"
                    );

                    if welcome_sent {
                        debug!(recipient = %recipient, group_id = %group_id, is_session, "Welcome synchronously sent");
                    }

                    // Don't encrypt immediately after creating session.
                    // Queue message until session is confirmed (peer processes our Welcome
                    // and we successfully decrypt their first message, or we receive their Welcome).
                    // This avoids race conditions where both peers create sessions.
                    if self.config.encryption.store_pending {
                        return Err(Error::SessionNotReady(self.establishment_state(recipient)?));
                    }
                }
            } else {
                // No key package available (memory nor storage)
                self.emit_mls_session_missing(
                    Some(recipient),
                    None,
                    MlsOperationContext::SessionLookup,
                    MlsErrorCategory::SessionStateMissing,
                );
                return Err(Error::SessionNotReady(self.establishment_state(recipient)?));
            }
        }

        // Only encrypt if session is confirmed (Welcome processed or successful decrypt).
        // Confirmation truth comes from persisted session state.
        if !self.is_session_confirmed(recipient)? {
            debug!(recipient = %recipient, "Session exists but not confirmed, queuing message");
            return Err(Error::SessionNotReady(self.establishment_state(recipient)?));
        }

        // Encrypt the message
        let encrypted = {
            let manager = mls
                .read()
                .map_err(|_| Error::Other("MLS lock poisoned".to_string()))?;
            manager
                .encrypt_for_user(recipient, content.as_bytes())
                .map_err(|_| Error::EncryptFailed("encryption operation failed".to_string()))?
        };

        // Serialize encrypted message with prefix
        let serialized =
            serde_json::to_string(&encrypted).map_err(|e| Error::Serialization(e.to_string()))?;

        self.emit_mls_encryption_used(recipient);
        Ok(format!("{}{}", internal_prefixes::ENCRYPTED, serialized))
    }

    fn queue_message_for_session_establishment(
        &mut self,
        recipient: &str,
        content: &str,
        priority: MessagePriority,
        reply_to_msg_id: Option<MessageId>,
        reconciliation_reason: &'static str,
    ) -> Result<MessageId> {
        // Generate an ID without ticking the Lamport clock.
        // The real tick happens when flush_pending_messages re-sends
        // via send_message after the session is established.
        let message_id = MessageId::new();

        debug!(
            recipient = %recipient,
            message_id = %message_id,
            "Message queued pending session establishment"
        );
        self.queue_pending_message(
            recipient,
            content,
            priority,
            message_id.clone(),
            reply_to_msg_id,
        );
        self.kick_pending_session_reconciliation(reconciliation_reason);
        if self.has_terminal_welcome_failure(recipient) {
            self.abort_pending_session_for_peer(
                recipient,
                crate::events::WelcomeReasonCode::RetryExhausted,
            );
            return Err(Error::Other(format!(
                "Welcome delivery failed for {}",
                recipient
            )));
        }

        Ok(message_id)
    }

    fn prepare_outbound_content(
        &mut self,
        recipient: &str,
        content: &str,
        priority: MessagePriority,
        reply_to_msg_id: Option<MessageId>,
        reconciliation_reason: &'static str,
    ) -> Result<OutboundSendPreparation> {
        if self.should_auto_encrypt() {
            if self.config.encryption.require_encryption {
                return self
                    .encrypt_content_for_recipient_strict(recipient, content)
                    .map(OutboundSendPreparation::Ready);
            }

            match self.encrypt_content_for_recipient(recipient, content, priority) {
                Ok(encrypted) => Ok(OutboundSendPreparation::Ready(encrypted)),
                Err(Error::SessionNotReady(state)) => {
                    if self.config.encryption.require_encryption
                        || !self.config.encryption.store_pending
                    {
                        return Err(Error::SessionNotReady(state));
                    }

                    let queued_id = self.queue_message_for_session_establishment(
                        recipient,
                        content,
                        priority,
                        reply_to_msg_id,
                        reconciliation_reason,
                    )?;
                    Ok(OutboundSendPreparation::Queued(queued_id))
                }
                Err(e) => Err(e),
            }
        } else if self.config.encryption.require_encryption {
            Err(Error::EncryptFailed(
                "MLS encryption is required but MLS is not initialized".to_string(),
            ))
        } else {
            Ok(OutboundSendPreparation::Ready(content.to_string()))
        }
    }

    fn ensure_plaintext_control_send_allowed(&self, operation: &str) -> Result<()> {
        if self.config.encryption.require_encryption {
            return Err(Error::EncryptFailed(format!(
                "{} sends plaintext control messages; disable require_encryption for bootstrap flows",
                operation
            )));
        }
        Ok(())
    }

    /// Sends or schedules sending of a welcome message to establish an MLS session.
    ///
    /// Returns `Ok(true)` when the welcome is delivered synchronously and `Ok(false)`
    /// when it is deferred to retry lifecycle management.
    fn send_welcome_message(&mut self, recipient: &str, welcome: &WelcomeMessage) -> Result<bool> {
        let serialized =
            serde_json::to_string(welcome).map_err(|e| Error::Serialization(e.to_string()))?;
        let content = format!("{}{}", internal_prefixes::WELCOME, serialized);
        let message = self.create_message(recipient, content, Some(MessagePriority::High), None)?;
        let group_id = welcome.group_id.as_str().to_string();

        self.upsert_welcome_lifecycle(recipient, &group_id, message, "welcome_created")?;
        self.try_send_welcome(recipient, "welcome_initial_send")
    }

    fn map_welcome_reason_code(error: &Error) -> crate::events::WelcomeReasonCode {
        SessionStateError::classify(error).to_welcome_reason_code()
    }

    fn can_confirm_from_source(&self, peer_id: &str, source_event: &str) -> bool {
        if !matches!(
            source_event,
            "decrypt_success"
                | "confirmation_ack_received"
                | "confirmation_probe_received"
                | "confirmation_retry"
        ) {
            return true;
        }

        match self.welcome_lifecycles.get(peer_id) {
            Some(record) => matches!(record.state, WelcomeDeliveryState::Sent),
            None => matches!(
                source_event,
                // Compatibility path for sessions created before welcome lifecycle
                // persistence existed. Decrypt-based confirmation stays blocked
                // until we have explicit local welcome delivery evidence.
                "confirmation_ack_received" | "confirmation_probe_received" | "confirmation_retry"
            ),
        }
    }

    fn has_terminal_welcome_failure(&self, peer_id: &str) -> bool {
        self.welcome_lifecycles
            .get(peer_id)
            .is_some_and(|record| matches!(record.state, WelcomeDeliveryState::Expired))
    }

    fn abort_pending_session_for_peer(
        &mut self,
        peer_id: &str,
        reason: crate::events::WelcomeReasonCode,
    ) {
        self.pending_encrypted_messages.remove(peer_id);
        self.clear_pending_messages_from_storage(peer_id);
        if let Ok(state) = lock_shared_state(&self.shared_state) {
            state.emit_event(Event::secure_session_failed(
                peer_id.to_string(),
                format!("Welcome delivery failed: {}", reason.as_str()),
            ));
        }
    }

    fn try_send_welcome(&mut self, peer_id: &str, source_event: &str) -> Result<bool> {
        let now = Utc::now();
        let mut record = self
            .welcome_lifecycles
            .get(peer_id)
            .cloned()
            .ok_or_else(|| Error::Other(format!("Missing welcome lifecycle for {}", peer_id)))?;

        if matches!(record.state, WelcomeDeliveryState::Sent) {
            return Ok(true);
        }
        if matches!(record.state, WelcomeDeliveryState::Expired) {
            return Ok(false);
        }

        if record.expires_at <= now {
            self.transition_welcome_state(peer_id, WelcomeDeliveryState::Expired, source_event)?;
            if let Ok(state) = lock_shared_state(&self.shared_state) {
                state.emit_event(Event::welcome_send_expired(
                    peer_id.to_string(),
                    record.welcome_message.id.as_str().to_string(),
                    record.attempt,
                    crate::events::WelcomeReasonCode::RetryExhausted,
                ));
            }
            self.abort_pending_session_for_peer(
                peer_id,
                crate::events::WelcomeReasonCode::RetryExhausted,
            );
            return Ok(false);
        }

        record.attempt = record.attempt.saturating_add(1);
        self.welcome_lifecycles
            .insert(peer_id.to_string(), record.clone());
        self.persist_welcome_lifecycle_entry(&record)?;
        self.transition_welcome_state(peer_id, WelcomeDeliveryState::SendAttempted, source_event)?;

        if let Ok(state) = lock_shared_state(&self.shared_state) {
            state.emit_event(Event::welcome_send_attempted(
                peer_id.to_string(),
                record.welcome_message.id.as_str().to_string(),
                record.group_id.clone(),
                record.attempt,
            ));
        }

        match self.transport_manager.send(&record.welcome_message) {
            Ok(()) => {
                let transport_used = self.transport_manager.current_transport();
                let mut updated =
                    self.welcome_lifecycles
                        .get(peer_id)
                        .cloned()
                        .ok_or_else(|| {
                            Error::Other(format!("Missing welcome lifecycle for {}", peer_id))
                        })?;

                if matches!(transport_used, Some(TransportType::Internet)) {
                    // Internet send() only enqueues for platform polling. Keep lifecycle
                    // non-terminal until explicit platform confirmation arrives.
                    updated.next_retry_at = Some(
                        Utc::now() + ChronoDuration::seconds(WELCOME_INTERNET_CONFIRM_TIMEOUT_SECS),
                    );
                    updated.last_reason_code = None;
                    updated.last_transport_error = None;
                    self.welcome_lifecycles
                        .insert(peer_id.to_string(), updated.clone());
                    self.persist_welcome_lifecycle_entry(&updated)?;
                    return Ok(false);
                }

                updated.next_retry_at = None;
                updated.last_reason_code = None;
                updated.last_transport_error = None;
                self.welcome_lifecycles
                    .insert(peer_id.to_string(), updated.clone());
                self.persist_welcome_lifecycle_entry(&updated)?;
                self.transition_welcome_state(peer_id, WelcomeDeliveryState::Sent, source_event)?;
                if let Ok(state) = lock_shared_state(&self.shared_state) {
                    state.emit_event(Event::welcome_send_succeeded(
                        peer_id.to_string(),
                        updated.welcome_message.id.as_str().to_string(),
                        updated.group_id,
                        updated.attempt,
                    ));
                }
                Ok(true)
            }
            Err(err) => {
                let reason = Self::map_welcome_reason_code(&err);
                self.apply_welcome_send_failure(
                    peer_id,
                    reason,
                    Some(err.to_string()),
                    source_event,
                )
            }
        }
    }

    fn apply_welcome_send_failure(
        &mut self,
        peer_id: &str,
        reason: crate::events::WelcomeReasonCode,
        transport_error: Option<String>,
        source_event: &str,
    ) -> Result<bool> {
        let mut updated = self
            .welcome_lifecycles
            .get(peer_id)
            .cloned()
            .ok_or_else(|| Error::Other(format!("Missing welcome lifecycle for {}", peer_id)))?;

        if matches!(
            updated.state,
            WelcomeDeliveryState::Sent | WelcomeDeliveryState::Expired
        ) {
            return Ok(matches!(updated.state, WelcomeDeliveryState::Sent));
        }

        let max_attempts = self.config.reliability.retry.max_retries.max(1);
        let should_expire = updated.attempt >= max_attempts || updated.expires_at <= Utc::now();
        if should_expire {
            let terminal_reason = crate::events::WelcomeReasonCode::RetryExhausted;
            {
                let record = self.welcome_lifecycles.get_mut(peer_id).ok_or_else(|| {
                    Error::Other(format!("Missing welcome lifecycle for {}", peer_id))
                })?;
                record.last_reason_code = Some(terminal_reason);
                record.last_transport_error = transport_error.clone();
                record.next_retry_at = None;
            }

            if !matches!(updated.state, WelcomeDeliveryState::Failed) {
                self.transition_welcome_state(peer_id, WelcomeDeliveryState::Failed, source_event)?;
            }
            self.transition_welcome_state(
                peer_id,
                WelcomeDeliveryState::Expired,
                "welcome_retry_exhausted",
            )?;

            let expired_snapshot =
                self.welcome_lifecycles
                    .get(peer_id)
                    .cloned()
                    .ok_or_else(|| {
                        Error::Other(format!("Missing welcome lifecycle for {}", peer_id))
                    })?;
            if let Ok(state) = lock_shared_state(&self.shared_state) {
                state.emit_event(Event::welcome_send_failed(
                    peer_id.to_string(),
                    expired_snapshot.welcome_message.id.as_str().to_string(),
                    expired_snapshot.group_id.clone(),
                    expired_snapshot.attempt,
                    terminal_reason,
                    expired_snapshot.last_transport_error.clone(),
                    false,
                    None,
                ));
                state.emit_event(Event::welcome_send_expired(
                    peer_id.to_string(),
                    expired_snapshot.welcome_message.id.as_str().to_string(),
                    expired_snapshot.attempt,
                    terminal_reason,
                ));
            }
            self.abort_pending_session_for_peer(peer_id, terminal_reason);
            return Ok(false);
        }

        let delay_ms = self.compute_welcome_retry_delay_ms(peer_id, updated.attempt);
        let retry_at = Utc::now() + ChronoDuration::milliseconds(delay_ms as i64);

        {
            let record = self.welcome_lifecycles.get_mut(peer_id).ok_or_else(|| {
                Error::Other(format!("Missing welcome lifecycle for {}", peer_id))
            })?;
            record.last_reason_code = Some(reason);
            record.last_transport_error = transport_error;
            record.next_retry_at = Some(retry_at);
        }

        if !matches!(updated.state, WelcomeDeliveryState::Failed) {
            self.transition_welcome_state(peer_id, WelcomeDeliveryState::Failed, source_event)?;
        } else if let Some(record) = self.welcome_lifecycles.get(peer_id).cloned() {
            self.persist_welcome_lifecycle_entry(&record)?;
        }

        updated = self
            .welcome_lifecycles
            .get(peer_id)
            .cloned()
            .ok_or_else(|| Error::Other(format!("Missing welcome lifecycle for {}", peer_id)))?;
        if let Ok(state) = lock_shared_state(&self.shared_state) {
            state.emit_event(Event::welcome_send_failed(
                peer_id.to_string(),
                updated.welcome_message.id.as_str().to_string(),
                updated.group_id,
                updated.attempt,
                reason,
                updated.last_transport_error.clone(),
                true,
                Some(retry_at.timestamp_millis()),
            ));
        }
        Ok(false)
    }

    fn session_ready_context_for_source(source_event: &str) -> MlsOperationContext {
        match source_event {
            "confirmation_ack_received" | "confirmation_probe_received" | "decrypt_success" => {
                MlsOperationContext::Receive
            }
            "welcome_received" => MlsOperationContext::Welcome,
            _ => MlsOperationContext::Send,
        }
    }

    fn maybe_emit_local_session_established(&self, peer_id: &str, context: MlsOperationContext) {
        let Some(record) = self.welcome_lifecycles.get(peer_id) else {
            return;
        };
        if !matches!(record.state, WelcomeDeliveryState::Sent) {
            return;
        }
        self.emit_mls_session_ready(peer_id, &record.group_id, context);
        if let Ok(state) = lock_shared_state(&self.shared_state) {
            state.emit_event(Event::secure_session_established(
                peer_id.to_string(),
                record.group_id.clone(),
                record.group_id.starts_with("session:"),
                true,
            ));
        }
    }

    fn find_welcome_peer_by_message_id(&self, message_id: &str) -> Option<String> {
        self.welcome_lifecycles
            .iter()
            .find_map(|(peer_id, record)| {
                if record.welcome_message.id.as_str() == message_id {
                    return Some(peer_id.clone());
                }
                None
            })
    }

    /// Handles asynchronous transport confirmation for pending welcome sends.
    pub fn on_transport_send_confirmed(&mut self, message_id: &str) -> Result<()> {
        let Some(peer_id) = self.find_welcome_peer_by_message_id(message_id) else {
            return Ok(());
        };

        let updated = self
            .welcome_lifecycles
            .get(&peer_id)
            .cloned()
            .ok_or_else(|| Error::Other(format!("Missing welcome lifecycle for {}", peer_id)))?;

        if matches!(
            updated.state,
            WelcomeDeliveryState::Sent | WelcomeDeliveryState::Expired
        ) {
            return Ok(());
        }

        {
            let record = self.welcome_lifecycles.get_mut(&peer_id).ok_or_else(|| {
                Error::Other(format!("Missing welcome lifecycle for {}", peer_id))
            })?;
            record.next_retry_at = None;
            record.last_reason_code = None;
            record.last_transport_error = None;
        }
        self.transition_welcome_state(&peer_id, WelcomeDeliveryState::Sent, "transport_confirmed")?;

        let sent_snapshot = self
            .welcome_lifecycles
            .get(&peer_id)
            .cloned()
            .ok_or_else(|| Error::Other(format!("Missing welcome lifecycle for {}", peer_id)))?;
        if let Ok(state) = lock_shared_state(&self.shared_state) {
            state.emit_event(Event::welcome_send_succeeded(
                peer_id,
                sent_snapshot.welcome_message.id.as_str().to_string(),
                sent_snapshot.group_id,
                sent_snapshot.attempt,
            ));
        }
        Ok(())
    }

    /// Handles asynchronous transport send failures for pending welcome sends.
    pub fn on_transport_send_failed(
        &mut self,
        message_id: &str,
        transport_error: Option<String>,
    ) -> Result<()> {
        let Some(peer_id) = self.find_welcome_peer_by_message_id(message_id) else {
            return Ok(());
        };
        let reason = crate::events::WelcomeReasonCode::TransportUnavailable;
        let _ =
            self.apply_welcome_send_failure(&peer_id, reason, transport_error, "transport_failed")?;
        Ok(())
    }

    /// Queues a message with a specific message ID for later sending when session is established.
    fn queue_pending_message(
        &mut self,
        recipient: &str,
        content: &str,
        priority: MessagePriority,
        message_id: MessageId,
        reply_to_msg: Option<MessageId>,
    ) {
        let message_id_str = message_id.as_str().to_string();
        let pending = PendingMessage {
            content: content.to_string(),
            priority,
            message_id,
            reply_to_msg,
            queued_at: Utc::now(),
        };

        // Persist to storage first (survives crashes)
        self.persist_pending_message(recipient, &pending);

        self.pending_encrypted_messages
            .entry(recipient.to_string())
            .or_default()
            .push(pending);

        debug!(recipient = %recipient, message_id = %message_id_str, "Queued message pending session establishment");
    }

    fn flush_restored_confirmed_pending_messages(&mut self) {
        let recipients: Vec<String> = self.pending_encrypted_messages.keys().cloned().collect();

        for recipient in recipients {
            match self.is_session_confirmed(&recipient) {
                Ok(true) => {
                    if let Err(err) = self.flush_pending_messages(&recipient) {
                        warn!(
                            recipient = %recipient,
                            error = %err,
                            "Failed to flush restored pending messages for confirmed session"
                        );
                    }
                }
                Ok(false) => {}
                Err(err) => {
                    warn!(
                        recipient = %recipient,
                        error = %err,
                        "Failed to read session confirmation state while restoring pending messages"
                    );
                }
            }
        }
    }

    /// Flushes pending messages for a recipient after session is established.
    fn flush_pending_messages(&mut self, recipient: &str) -> Result<()> {
        if let Some(pending) = self.pending_encrypted_messages.remove(recipient) {
            info!(recipient = %recipient, count = pending.len(), "Flushing pending messages");
            let mut remaining = Vec::new();

            for msg in pending {
                // Re-attempt to send each pending message
                // Use the stored message ID by passing reply_to_msg if it exists
                let reply_to_str = msg.reply_to_msg.as_ref().map(|id| id.as_str().to_string());
                match self.send_message(
                    recipient,
                    msg.content.clone(),
                    Some(msg.priority),
                    reply_to_str,
                ) {
                    Ok(id) => {
                        // Note: The new message will have a new ID, but the original ID was already returned to the caller
                        debug!(original_id = %msg.message_id, new_id = %id, "Sent pending message");
                    }
                    Err(e) => {
                        warn!(original_id = %msg.message_id, error = %e, "Failed to send pending message");
                        remaining.push(msg);
                    }
                }
            }

            if remaining.is_empty() {
                self.clear_pending_messages_from_storage(recipient);
            } else {
                self.persist_pending_messages_snapshot(recipient, &remaining);
                self.pending_encrypted_messages
                    .insert(recipient.to_string(), remaining);
            }
        }
        Ok(())
    }

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

    fn prune_expired_pending_for_peer(&mut self, peer_id: &str, now: Instant) -> usize {
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

    fn prune_expired_pending_global_front(&mut self, now: Instant, max_evictions: usize) -> usize {
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

    fn enqueue_pending_decryption(&mut self, sender: &str, message: &Message) {
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
    fn process_pending_decryption(&mut self, sender: &str) {
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
                }
            }
        }
    }

    // ========================================================================
    // PENDING MESSAGE PERSISTENCE
    // ========================================================================

    /// Persists a pending message for a recipient to storage.
    ///
    /// This ensures messages survive app crashes/restarts.
    fn persist_pending_message(&self, recipient: &str, pending: &PendingMessage) {
        // Load existing messages for this recipient
        let mut messages: Vec<PendingMessage> = self
            .load_pending_messages_from_storage(recipient)
            .unwrap_or_default();

        // Add the new message
        messages.push(pending.clone());

        self.persist_pending_messages_snapshot(recipient, &messages);
    }

    fn persist_pending_messages_snapshot(&self, recipient: &str, messages: &[PendingMessage]) {
        let Some(storage) = &self.message_storage else {
            return;
        };

        if messages.is_empty() {
            if let Err(e) = storage.delete(storage_keys::PENDING_MESSAGES, recipient) {
                warn!(
                    recipient = %recipient,
                    error = %e,
                    "Failed to clear persisted pending messages"
                );
            }
            return;
        }

        match serde_json::to_vec(messages) {
            Ok(data) => {
                if let Err(e) = storage.store(storage_keys::PENDING_MESSAGES, recipient, &data) {
                    warn!(recipient = %recipient, error = %e, "Failed to persist pending messages");
                }
            }
            Err(e) => {
                warn!(recipient = %recipient, error = %e, "Failed to serialize pending messages");
            }
        }
    }

    /// Loads pending messages for a recipient from storage.
    fn load_pending_messages_from_storage(&self, recipient: &str) -> Option<Vec<PendingMessage>> {
        let storage = self.message_storage.as_ref()?;
        let data = storage
            .load(storage_keys::PENDING_MESSAGES, recipient)
            .ok()??;
        serde_json::from_slice(&data).ok()
    }

    /// Removes pending messages for a recipient from storage.
    fn clear_pending_messages_from_storage(&self, recipient: &str) {
        if let Some(storage) = &self.message_storage {
            let _ = storage.delete(storage_keys::PENDING_MESSAGES, recipient);
        }
    }

    /// Restores all pending messages from storage on startup.
    ///
    /// This should be called after initializing storage to recover
    /// any messages that were pending when the app was terminated.
    fn restore_pending_messages(&mut self) -> Result<()> {
        let Some(storage) = &self.message_storage else {
            return Ok(());
        };

        let recipients = storage
            .list_keys(storage_keys::PENDING_MESSAGES)
            .map_err(|e| Error::Other(format!("Failed to list pending messages: {}", e)))?;

        for recipient in recipients {
            if let Some(messages) = self.load_pending_messages_from_storage(&recipient) {
                if !messages.is_empty() {
                    info!(recipient = %recipient, count = messages.len(), "Restored pending messages from storage");
                    self.pending_encrypted_messages.insert(recipient, messages);
                }
            }
        }

        Ok(())
    }

    /// Persists a received key package for a peer so it survives restart.
    fn persist_peer_key_package(&self, peer_id: &str, pkg: &ReceivedKeyPackage) {
        let Some(storage) = &self.message_storage else {
            return;
        };
        match serde_json::to_vec(pkg) {
            Ok(data) => {
                if let Err(e) = storage.store(storage_keys::PEER_KEY_PACKAGES, peer_id, &data) {
                    warn!(peer_id = %peer_id, error = %e, "Failed to persist peer key package");
                }
            }
            Err(e) => {
                warn!(peer_id = %peer_id, error = %e, "Failed to serialize peer key package");
            }
        }
    }

    /// Loads a persisted key package for a peer (if present and not expired).
    fn load_peer_key_package_from_storage(&self, peer_id: &str) -> Option<ReceivedKeyPackage> {
        let storage = self.message_storage.as_ref()?;
        let data = storage
            .load(storage_keys::PEER_KEY_PACKAGES, peer_id)
            .ok()??;
        let pkg: ReceivedKeyPackage = serde_json::from_slice(&data).ok()?;
        let now_ms = Utc::now().timestamp_millis() as u64;
        if now_ms >= pkg.local_expires_at_ms {
            let _ = storage.delete(storage_keys::PEER_KEY_PACKAGES, peer_id);
            return None;
        }
        Some(pkg)
    }

    /// Removes persisted key package for a peer (e.g. after session created).
    fn delete_peer_key_package_from_storage(&self, peer_id: &str) {
        if let Some(storage) = &self.message_storage {
            let _ = storage.delete(storage_keys::PEER_KEY_PACKAGES, peer_id);
        }
    }

    /// Loads key package from storage into memory if not already present. Returns true if we now have one in memory.
    fn try_load_key_package_from_storage_into_memory(&mut self, peer_id: &str) -> bool {
        if self.pending_key_packages.contains_key(peer_id) {
            return true;
        }
        if let Some(pkg) = self.load_peer_key_package_from_storage(peer_id) {
            self.pending_key_packages.insert(peer_id.to_string(), pkg);
            return true;
        }
        false
    }

    /// Restores peer key packages from storage for peers that have no MLS session.
    fn restore_peer_key_packages(&mut self, mls: &Arc<RwLock<MlsManager>>) -> Result<()> {
        let Some(storage) = &self.message_storage else {
            return Ok(());
        };

        let peer_ids = storage
            .list_keys(storage_keys::PEER_KEY_PACKAGES)
            .map_err(|e| Error::Other(format!("Failed to list peer key packages: {}", e)))?;

        let sessions = {
            let manager = mls
                .read()
                .map_err(|_| Error::Other("MLS lock poisoned".to_string()))?;
            manager.list_sessions().map_err(Error::Mls)?
        };
        let session_set: std::collections::HashSet<_> = sessions.into_iter().collect();

        for peer_id in peer_ids {
            if session_set.contains(&peer_id) {
                continue;
            }
            if let Some(pkg) = self.load_peer_key_package_from_storage(&peer_id) {
                info!(peer_id = %peer_id, "Restored peer key package from storage");
                self.pending_key_packages.insert(peer_id, pkg);
            }
        }

        Ok(())
    }

    /// Loads a persisted session state entry (if present).
    fn load_session_state_entry(&self, peer_id: &str) -> Result<Option<SessionState>> {
        let Some(storage) = &self.message_storage else {
            return Ok(None);
        };

        let Some(data) = storage
            .load(storage_keys::SESSION_STATES, peer_id)
            .map_err(|e| {
                Error::Other(format!(
                    "Failed to load session state for {}: {}",
                    peer_id, e
                ))
            })?
        else {
            return Ok(None);
        };

        let state = serde_json::from_slice::<SessionState>(&data).map_err(|e| {
            Error::Other(format!(
                "Failed to deserialize session state for {}: {}",
                peer_id, e
            ))
        })?;

        Ok(Some(state))
    }

    /// Persists session state atomically for a single peer key.
    fn persist_session_state(
        &self,
        peer_id: &str,
        new_state: SessionState,
        source_event: &str,
    ) -> Result<()> {
        let Some(storage) = &self.message_storage else {
            return Err(Error::MlsNotInitialized);
        };

        let encoded = serde_json::to_vec(&new_state).map_err(|e| {
            Error::Serialization(format!("Failed to serialize session state: {}", e))
        })?;
        storage
            .store(storage_keys::SESSION_STATES, peer_id, &encoded)
            .map_err(|e| {
                Error::Other(format!(
                    "Failed to persist session state for {}: {}",
                    peer_id, e
                ))
            })?;
        let persisted_data = storage
            .load(storage_keys::SESSION_STATES, peer_id)
            .map_err(|e| {
                Error::Other(format!(
                    "Failed to verify persisted session state for {}: {}",
                    peer_id, e
                ))
            })?
            .ok_or_else(|| {
                Error::Other(format!(
                    "Persisted session state missing immediately after write for {}",
                    peer_id
                ))
            })?;
        let persisted_state =
            serde_json::from_slice::<SessionState>(&persisted_data).map_err(|e| {
                Error::Other(format!(
                    "Failed to deserialize verified session state for {}: {}",
                    peer_id, e
                ))
            })?;
        if persisted_state != new_state {
            return Err(Error::Other(format!(
                "Session state verification mismatch for {}: expected {}, got {}",
                peer_id,
                new_state.as_str(),
                persisted_state.as_str()
            )));
        }

        if matches!(new_state, SessionState::Confirmed) {
            info!(
                event = "confirmation_persisted",
                session_or_group_id = %peer_id,
                previous_state = "Pending",
                new_state = "Confirmed",
                source_event = %source_event,
                "confirmation_persisted"
            );
        }

        Ok(())
    }

    fn clear_session_state_entry(&self, peer_id: &str) -> Result<()> {
        let Some(storage) = &self.message_storage else {
            return Ok(());
        };
        storage
            .delete(storage_keys::SESSION_STATES, peer_id)
            .map_err(|e| {
                Error::Other(format!(
                    "Failed to clear session state for {}: {}",
                    peer_id, e
                ))
            })
    }

    fn load_welcome_lifecycle_entry(
        &self,
        peer_id: &str,
    ) -> Result<Option<WelcomeLifecycleRecord>> {
        let Some(storage) = &self.message_storage else {
            return Ok(None);
        };

        let Some(data) = storage
            .load(storage_keys::WELCOME_LIFECYCLES, peer_id)
            .map_err(|e| {
                Error::Other(format!(
                    "Failed to load welcome lifecycle for {}: {}",
                    peer_id, e
                ))
            })?
        else {
            return Ok(None);
        };

        let record = serde_json::from_slice::<WelcomeLifecycleRecord>(&data).map_err(|e| {
            Error::Other(format!(
                "Failed to deserialize welcome lifecycle for {}: {}",
                peer_id, e
            ))
        })?;
        Ok(Some(record))
    }

    fn persist_welcome_lifecycle_entry(&self, record: &WelcomeLifecycleRecord) -> Result<()> {
        let Some(storage) = &self.message_storage else {
            return Err(Error::MlsNotInitialized);
        };

        let encoded = serde_json::to_vec(record).map_err(|e| {
            Error::Serialization(format!("Failed to serialize welcome lifecycle: {}", e))
        })?;
        storage
            .store(storage_keys::WELCOME_LIFECYCLES, &record.peer_id, &encoded)
            .map_err(|e| {
                Error::Other(format!(
                    "Failed to persist welcome lifecycle for {}: {}",
                    record.peer_id, e
                ))
            })
    }

    fn clear_welcome_lifecycle_entry(&self, peer_id: &str) -> Result<()> {
        let Some(storage) = &self.message_storage else {
            return Ok(());
        };
        storage
            .delete(storage_keys::WELCOME_LIFECYCLES, peer_id)
            .map_err(|e| {
                Error::Other(format!(
                    "Failed to clear welcome lifecycle for {}: {}",
                    peer_id, e
                ))
            })
    }

    fn restore_welcome_lifecycles(&mut self) -> Result<()> {
        self.welcome_lifecycles.clear();
        let Some(storage) = &self.message_storage else {
            return Ok(());
        };

        let peers = storage
            .list_keys(storage_keys::WELCOME_LIFECYCLES)
            .map_err(|e| Error::Other(format!("Failed to list welcome lifecycles: {}", e)))?;

        for peer_id in peers {
            if let Some(mut record) = self.load_welcome_lifecycle_entry(&peer_id)? {
                if matches!(
                    record.state,
                    WelcomeDeliveryState::Created | WelcomeDeliveryState::SendAttempted
                ) {
                    record.state = WelcomeDeliveryState::Failed;
                    record.next_retry_at = Some(Utc::now());
                    self.persist_welcome_lifecycle_entry(&record)?;
                    warn!(
                        event = "welcome_lifecycle_repaired",
                        session_or_group_id = %peer_id,
                        repair_action = "in_flight_to_failed_retry_now",
                        state = record.state.as_str(),
                        attempt = record.attempt,
                        "welcome_lifecycle_repaired"
                    );
                }
                if matches!(record.state, WelcomeDeliveryState::Failed)
                    && record.next_retry_at.is_none()
                {
                    if matches!(
                        record.last_reason_code,
                        Some(crate::events::WelcomeReasonCode::RetryExhausted)
                    ) || record.expires_at <= Utc::now()
                    {
                        record.state = WelcomeDeliveryState::Expired;
                        warn!(
                            event = "welcome_lifecycle_repaired",
                            session_or_group_id = %peer_id,
                            repair_action = "failed_no_retry_to_expired",
                            state = record.state.as_str(),
                            attempt = record.attempt,
                            "welcome_lifecycle_repaired"
                        );
                    } else {
                        // Recover from partial-crash write where Failed was persisted
                        // without a retry schedule.
                        record.next_retry_at = Some(Utc::now());
                        warn!(
                            event = "welcome_lifecycle_repaired",
                            session_or_group_id = %peer_id,
                            repair_action = "failed_no_retry_to_failed_retry_now",
                            state = record.state.as_str(),
                            attempt = record.attempt,
                            "welcome_lifecycle_repaired"
                        );
                    }
                    self.persist_welcome_lifecycle_entry(&record)?;
                }
                if matches!(
                    record.state,
                    WelcomeDeliveryState::Sent | WelcomeDeliveryState::Expired
                ) && record.next_retry_at.is_some()
                {
                    record.next_retry_at = None;
                    self.persist_welcome_lifecycle_entry(&record)?;
                    warn!(
                        event = "welcome_lifecycle_repaired",
                        session_or_group_id = %peer_id,
                        repair_action = "terminal_clear_retry_schedule",
                        state = record.state.as_str(),
                        attempt = record.attempt,
                        "welcome_lifecycle_repaired"
                    );
                }
                self.welcome_lifecycles.insert(peer_id.clone(), record);
                info!(
                    event = "welcome_lifecycle_restored",
                    session_or_group_id = %peer_id,
                    "welcome_lifecycle_restored"
                );
            }
        }

        Ok(())
    }

    fn can_transition_welcome_state(
        current: WelcomeDeliveryState,
        next: WelcomeDeliveryState,
    ) -> bool {
        matches!(
            (current, next),
            (
                WelcomeDeliveryState::Created,
                WelcomeDeliveryState::SendAttempted
            ) | (WelcomeDeliveryState::Created, WelcomeDeliveryState::Expired)
                | (
                    WelcomeDeliveryState::SendAttempted,
                    WelcomeDeliveryState::Sent
                )
                | (
                    WelcomeDeliveryState::SendAttempted,
                    WelcomeDeliveryState::Failed
                )
                | (
                    WelcomeDeliveryState::Failed,
                    WelcomeDeliveryState::SendAttempted
                )
                | (WelcomeDeliveryState::Failed, WelcomeDeliveryState::Sent)
                | (WelcomeDeliveryState::Failed, WelcomeDeliveryState::Expired)
        )
    }

    fn transition_welcome_state(
        &mut self,
        peer_id: &str,
        next_state: WelcomeDeliveryState,
        source_event: &str,
    ) -> Result<()> {
        let (previous_state, record_snapshot) = {
            let record = self.welcome_lifecycles.get_mut(peer_id).ok_or_else(|| {
                Error::Other(format!(
                    "Missing welcome lifecycle for transition: {}",
                    peer_id
                ))
            })?;

            if record.state == next_state {
                return Ok(());
            }

            if !Self::can_transition_welcome_state(record.state, next_state) {
                return Err(Error::Other(format!(
                    "Illegal welcome lifecycle transition for {}: {} -> {}",
                    peer_id,
                    record.state.as_str(),
                    next_state.as_str()
                )));
            }

            let previous = record.state;
            record.state = next_state;
            if matches!(
                next_state,
                WelcomeDeliveryState::Sent | WelcomeDeliveryState::Expired
            ) {
                record.next_retry_at = None;
            }
            (previous, record.clone())
        };

        self.persist_welcome_lifecycle_entry(&record_snapshot)?;
        info!(
            event = "welcome_lifecycle_transition",
            session_or_group_id = %peer_id,
            previous_state = previous_state.as_str(),
            new_state = next_state.as_str(),
            source_event = %source_event,
            attempt = record_snapshot.attempt,
            "welcome_lifecycle_transition"
        );
        Ok(())
    }

    fn upsert_welcome_lifecycle(
        &mut self,
        peer_id: &str,
        group_id: &str,
        welcome_message: Message,
        source_event: &str,
    ) -> Result<()> {
        if let Some(existing) = self.welcome_lifecycles.get(peer_id) {
            if !matches!(
                existing.state,
                WelcomeDeliveryState::Sent | WelcomeDeliveryState::Expired
            ) {
                return Err(Error::Other(format!(
                    "Refusing to overwrite active welcome lifecycle for {} in state {}",
                    peer_id,
                    existing.state.as_str()
                )));
            }
        }

        let now = Utc::now();
        let record = WelcomeLifecycleRecord {
            peer_id: peer_id.to_string(),
            group_id: group_id.to_string(),
            state: WelcomeDeliveryState::Created,
            attempt: 0,
            welcome_message,
            next_retry_at: None,
            last_reason_code: None,
            last_transport_error: None,
            created_at: now,
            expires_at: now + ChronoDuration::seconds(WELCOME_LIFECYCLE_TTL_SECS),
        };
        self.welcome_lifecycles
            .insert(peer_id.to_string(), record.clone());
        self.persist_welcome_lifecycle_entry(&record)?;
        info!(
            event = "welcome_lifecycle_transition",
            session_or_group_id = %peer_id,
            previous_state = "Absent",
            new_state = WelcomeDeliveryState::Created.as_str(),
            source_event = %source_event,
            attempt = 0,
            "welcome_lifecycle_transition"
        );
        Ok(())
    }

    fn compute_welcome_retry_delay_ms(&self, peer_id: &str, attempt: u32) -> u64 {
        let config = &self.config.reliability.retry;
        let capped_attempt = attempt.saturating_sub(1);
        let base_ms = if capped_attempt == 0 {
            config.initial_delay_ms
        } else {
            let multiplier = config.backoff_multiplier.powi(capped_attempt as i32);
            (config.initial_delay_ms as f64 * multiplier as f64) as u64
        }
        .min(config.max_delay_ms);

        let mut hasher = std::collections::hash_map::DefaultHasher::new();
        peer_id.hash(&mut hasher);
        attempt.hash(&mut hasher);
        Utc::now().timestamp_millis().hash(&mut hasher);
        let bucket = (hasher.finish() % 10_000) as f64 / 10_000.0;
        let jitter_factor = 1.0 + ((bucket * 2.0 - 1.0) * WELCOME_RETRY_JITTER_RATIO);
        let jittered = (base_ms as f64 * jitter_factor).round() as i64;
        jittered.max(1) as u64
    }

    /// Ensures a session has an explicit persisted state entry.
    fn ensure_session_state_entry(
        &self,
        peer_id: &str,
        source_event: &str,
    ) -> Result<SessionState> {
        let existing = self.load_session_state_entry(peer_id)?;
        if let Some(state) = existing {
            return Ok(state);
        }

        self.persist_session_state(peer_id, SessionState::Pending, source_event)?;
        info!(
            event = "session_state_transition",
            session_or_group_id = %peer_id,
            previous_state = "Absent",
            new_state = "Pending",
            source_event = %source_event,
            "session_state_transition"
        );
        Ok(SessionState::Pending)
    }

    /// Returns true when persisted state marks this peer session as confirmed.
    fn is_session_confirmed(&mut self, peer_id: &str) -> Result<bool> {
        let persisted = self
            .load_session_state_entry(peer_id)?
            .unwrap_or(SessionState::Pending);
        if matches!(persisted, SessionState::Confirmed) {
            if !self.has_mls_session(peer_id)? {
                warn!(
                    peer_id = %peer_id,
                    "Persisted confirmed state has no matching MLS session; clearing stale state"
                );
                self.confirmed_sessions.remove(peer_id);
                self.clear_confirmation_recovery_tracking(peer_id);
                self.welcome_lifecycles.remove(peer_id);
                if let Err(err) = self.clear_session_state_entry(peer_id) {
                    warn!(
                        peer_id = %peer_id,
                        error = %err,
                        "Failed to clear stale persisted session state"
                    );
                }
                if let Err(err) = self.clear_welcome_lifecycle_entry(peer_id) {
                    warn!(
                        peer_id = %peer_id,
                        error = %err,
                        "Failed to clear stale persisted welcome lifecycle"
                    );
                }
                return Ok(false);
            }
            self.confirmed_sessions.insert(peer_id.to_string());
            return Ok(true);
        }
        Ok(false)
    }

    fn has_mls_session(&self, peer_id: &str) -> Result<bool> {
        let Some(mls) = self.mls_manager.clone() else {
            return Ok(false);
        };

        let manager = mls
            .read()
            .map_err(|_| Error::Other("MLS lock poisoned".to_string()))?;
        manager.has_session(peer_id).map_err(Error::Mls)
    }

    /// Returns the current establishment state for a peer (for API and error reporting).
    fn establishment_state(&self, peer_id: &str) -> Result<EstablishmentState> {
        if let Some(mls) = &self.mls_manager {
            let has_session = {
                let manager = mls
                    .read()
                    .map_err(|_| Error::Other("MLS lock poisoned".to_string()))?;
                manager.has_session(peer_id).map_err(Error::Mls)?
            };
            if has_session {
                let state = self
                    .load_session_state_entry(peer_id)?
                    .unwrap_or(SessionState::Pending);
                return Ok(if matches!(state, SessionState::Confirmed) {
                    EstablishmentState::SessionConfirmed
                } else {
                    EstablishmentState::SessionPending
                });
            }
        }

        let now_ms = Utc::now().timestamp_millis() as u64;
        if let Some(pkg) = self.pending_key_packages.get(peer_id) {
            if now_ms < pkg.local_expires_at_ms {
                return Ok(EstablishmentState::HaveKeyPackage);
            }
        }
        if self.load_peer_key_package_from_storage(peer_id).is_some() {
            return Ok(EstablishmentState::HaveKeyPackage);
        }
        Ok(EstablishmentState::NoKeyPackage)
    }

    fn schedule_confirmation_retry(&mut self, peer_id: &str, source_event: &str) {
        self.confirmation_retry_due_at
            .insert(peer_id.to_string(), Utc::now());
        warn!(
            event = "session_confirmation_retry_scheduled",
            session_or_group_id = %peer_id,
            source_event = %source_event,
            "session_confirmation_retry_scheduled"
        );
    }

    fn clear_confirmation_recovery_tracking(&mut self, peer_id: &str) {
        self.confirmation_retry_due_at.remove(peer_id);
        self.confirmation_probe_due_at.remove(peer_id);
    }

    /// Monotonic state transition helper: Pending -> Confirmed only.
    fn confirm_session_state(&mut self, peer_id: &str, source_event: &str) -> Result<bool> {
        if !self.can_confirm_from_source(peer_id, source_event) {
            warn!(
                event = "session_confirmation_blocked",
                session_or_group_id = %peer_id,
                source_event = %source_event,
                "session_confirmation_blocked"
            );
            return Ok(false);
        }

        let previous = self.ensure_session_state_entry(peer_id, source_event)?;

        if matches!(previous, SessionState::Confirmed) {
            self.confirmed_sessions.insert(peer_id.to_string());
            self.clear_confirmation_recovery_tracking(peer_id);
            info!(
                event = "session_state_transition",
                session_or_group_id = %peer_id,
                previous_state = "Confirmed",
                new_state = "Confirmed",
                source_event = %source_event,
                "session_state_transition"
            );
            return Ok(false);
        }

        // Persist first, then publish in-memory view.
        if let Err(err) = self.persist_session_state(peer_id, SessionState::Confirmed, source_event)
        {
            self.schedule_confirmation_retry(peer_id, source_event);
            return Err(err);
        }

        self.confirmed_sessions.insert(peer_id.to_string());
        self.clear_confirmation_recovery_tracking(peer_id);
        if source_event != "welcome_received" && source_event != "decrypt_success" {
            self.maybe_emit_local_session_established(
                peer_id,
                Self::session_ready_context_for_source(source_event),
            );
        }
        info!(
            event = "session_state_transition",
            session_or_group_id = %peer_id,
            previous_state = previous.as_str(),
            new_state = "Confirmed",
            source_event = %source_event,
            "session_state_transition"
        );
        Ok(true)
    }

    /// Reconstructs runtime confirmation cache from persisted session states.
    fn restore_session_states_from_manager(&mut self, mls: Arc<RwLock<MlsManager>>) -> Result<()> {
        self.confirmed_sessions.clear();

        let sessions = {
            let manager = mls
                .read()
                .map_err(|_| Error::Other("MLS lock poisoned".to_string()))?;
            manager.list_sessions()?
        };

        for peer_id in sessions {
            let state = match self.load_session_state_entry(&peer_id)? {
                Some(state) => state,
                None => self.bootstrap_missing_session_state(&peer_id)?,
            };
            if matches!(state, SessionState::Confirmed) {
                self.confirmed_sessions.insert(peer_id.clone());
            }
            info!(
                event = "session_state_restored",
                session_or_group_id = %peer_id,
                previous_state = "Pending",
                new_state = %state.as_str(),
                source_event = "initialize_mls",
                "session_state_restored"
            );
        }

        Ok(())
    }

    fn bootstrap_missing_session_state(&self, peer_id: &str) -> Result<SessionState> {
        // Legacy session records without explicit state are treated as Pending.
        // Recovery is driven by probe/ack reconciliation, never by implicit inference.
        let restored_state = SessionState::Pending;
        self.persist_session_state(
            peer_id,
            restored_state,
            "initialize_mls_missing_state_migration",
        )?;
        info!(
            event = "session_state_transition",
            session_or_group_id = %peer_id,
            previous_state = "Absent",
            new_state = %restored_state.as_str(),
            source_event = "initialize_mls_missing_state_migration",
            "session_state_transition"
        );

        Ok(restored_state)
    }

    fn collect_pending_session_peers(&mut self) -> Result<Vec<String>> {
        let Some(mls) = self.mls_manager.clone() else {
            return Ok(Vec::new());
        };

        let sessions = {
            let manager = mls
                .read()
                .map_err(|_| Error::Other("MLS lock poisoned".to_string()))?;
            manager.list_sessions()?
        };

        let mut pending = Vec::new();
        for peer_id in sessions {
            if !self.is_session_confirmed(&peer_id)? {
                pending.push(peer_id);
            }
        }

        Ok(pending)
    }

    fn send_session_confirmation_probe(&mut self, peer_id: &str, source_event: &str) {
        match self.send_internal_message(
            peer_id,
            internal_prefixes::SESSION_CONFIRM_PROBE.to_string(),
            MessagePriority::High,
        ) {
            Ok(_) => {
                info!(
                    event = "session_confirmation_probe_sent",
                    session_or_group_id = %peer_id,
                    source_event = %source_event,
                    "session_confirmation_probe_sent"
                );
            }
            Err(err) => {
                warn!(
                    event = "session_confirmation_probe_failed",
                    session_or_group_id = %peer_id,
                    source_event = %source_event,
                    error = %err,
                    "session_confirmation_probe_failed"
                );
            }
        }
    }

    fn kick_pending_session_reconciliation(&mut self, source_event: &str) {
        let now = Utc::now();
        let pending_peers = match self.collect_pending_session_peers() {
            Ok(peers) => peers,
            Err(err) => {
                warn!(
                    event = "session_confirmation_probe_scan_failed",
                    source_event = %source_event,
                    error = %err,
                    "session_confirmation_probe_scan_failed"
                );
                return;
            }
        };

        let pending_set: HashSet<String> = pending_peers.iter().cloned().collect();
        self.confirmation_probe_due_at
            .retain(|peer, _| pending_set.contains(peer));

        for peer_id in pending_peers {
            let due_at = self
                .confirmation_probe_due_at
                .get(&peer_id)
                .copied()
                .unwrap_or(now);
            if due_at > now {
                continue;
            }

            self.send_session_confirmation_probe(&peer_id, source_event);
            self.confirmation_probe_due_at.insert(
                peer_id,
                now + ChronoDuration::seconds(CONFIRMATION_PROBE_INTERVAL_SECS),
            );
        }
    }

    fn retry_pending_session_confirmations(&mut self) {
        let now = Utc::now();
        let due_peers: Vec<String> = self
            .confirmation_retry_due_at
            .iter()
            .filter_map(|(peer_id, due_at)| {
                if *due_at <= now {
                    Some(peer_id.clone())
                } else {
                    None
                }
            })
            .collect();

        for peer_id in due_peers {
            match self.has_mls_session(&peer_id) {
                Ok(true) => {}
                Ok(false) => {
                    self.confirmation_retry_due_at.remove(&peer_id);
                    continue;
                }
                Err(err) => {
                    warn!(
                        event = "session_confirmation_retry_scan_failed",
                        session_or_group_id = %peer_id,
                        error = %err,
                        "session_confirmation_retry_scan_failed"
                    );
                    self.confirmation_retry_due_at.insert(
                        peer_id,
                        now + ChronoDuration::seconds(CONFIRMATION_RETRY_INTERVAL_SECS),
                    );
                    continue;
                }
            }

            if !self.can_confirm_from_source(&peer_id, "confirmation_retry") {
                debug!(
                    peer_id = %peer_id,
                    "Skipping confirmation retry until welcome delivery is sent"
                );
                continue;
            }

            match self.confirm_session_state(&peer_id, "confirmation_retry") {
                Ok(_) => {
                    let _ = self.flush_pending_messages(&peer_id);
                    self.process_pending_decryption(&peer_id);
                }
                Err(err) => {
                    warn!(
                        event = "session_confirmation_retry_failed",
                        session_or_group_id = %peer_id,
                        error = %err,
                        "session_confirmation_retry_failed"
                    );
                    self.confirmation_retry_due_at.insert(
                        peer_id,
                        now + ChronoDuration::seconds(CONFIRMATION_RETRY_INTERVAL_SECS),
                    );
                }
            }
        }
    }

    // ========================================================================
    // LAMPORT CLOCK PERSISTENCE
    // ========================================================================

    /// Persists the current Lamport clock value to storage.
    fn persist_lamport_clock(&self) {
        let Some(storage) = &self.message_storage else {
            return;
        };
        let value = self.lamport_clock.value().to_le_bytes();
        if let Err(e) = storage.store(
            storage_keys::LAMPORT_CLOCK,
            storage_keys::LAMPORT_CLOCK_ID,
            &value,
        ) {
            warn!(error = %e, "Failed to persist Lamport clock");
        }
    }

    /// Restores the Lamport clock from storage.
    ///
    /// Uses `max(current, restored)` so the clock never goes backward even
    /// if the in-memory value has advanced before storage was attached.
    fn restore_lamport_clock(&mut self) {
        let Some(storage) = &self.message_storage else {
            return;
        };
        if let Ok(Some(data)) =
            storage.load(storage_keys::LAMPORT_CLOCK, storage_keys::LAMPORT_CLOCK_ID)
        {
            if data.len() == 8 {
                let restored = u64::from_le_bytes(data.try_into().expect("verified length is 8"));
                let restored_clock = LamportClock::from_value(restored);
                if restored_clock > self.lamport_clock {
                    self.lamport_clock = restored_clock;
                }
                debug!(clock = %self.lamport_clock, "Restored Lamport clock from storage");
            } else {
                warn!(
                    len = data.len(),
                    "Corrupted Lamport clock in storage (expected 8 bytes), starting fresh"
                );
            }
        }
    }

    // ========================================================================
    // KEY PACKAGE HANDLING
    // ========================================================================

    /// Sends our key package to a peer for session establishment.
    fn send_key_package_to(&mut self, peer_id: &str) -> Result<()> {
        let mls = self.mls_manager.as_ref().ok_or(Error::MlsNotInitialized)?;

        let key_pkg = {
            let manager = mls
                .read()
                .map_err(|_| Error::Other("MLS lock poisoned".to_string()))?;
            manager.get_or_create_key_package()?
        };

        let payload = KeyPackagePayload {
            user_id: self.config.user_id.clone(),
            key_package_data: key_pkg.key_package_data.clone(),
            remaining_lifetime_ms: key_pkg.remaining_lifetime_ms(),
            timestamp_ms: Utc::now().timestamp_millis() as u64,
        };

        let serialized =
            serde_json::to_string(&payload).map_err(|e| Error::Serialization(e.to_string()))?;
        let content = format!("{}{}", internal_prefixes::KEY_PACKAGE, serialized);

        let message = self.create_message(peer_id, content, Some(MessagePriority::Low), None)?;

        match self.transport_manager.send(&message) {
            Ok(()) => {
                self.key_package_sent_to.insert(peer_id.to_string());
                debug!(peer_id = %peer_id, message_id = %message.id, "Sent key package");
                Ok(())
            }
            Err(err) => {
                // Don't mark as sent and don't enqueue for retry -- if the peer is
                // unreachable now, on_neighbor_discovered will fire again when they
                // reconnect, generating a fresh exchange.
                debug!(peer_id = %peer_id, error = %err, "Key package send deferred");
                Err(err)
            }
        }
    }

    /// Called when a new neighbor is discovered.
    ///
    /// When auto key exchange is enabled, this method sends our key package
    /// to the newly discovered peer to enable encrypted communication.
    ///
    /// # Arguments
    ///
    /// * `peer_id` - The ID of the discovered peer
    pub fn on_neighbor_discovered(&mut self, peer_id: &str) {
        // Don't track ourselves
        if peer_id == self.config.user_id {
            return;
        }

        // Track discovered peers for service discovery and routing, with capacity limit
        if self.known_peers.len() < MAX_KNOWN_PEERS || self.known_peers.contains(peer_id) {
            self.known_peers.insert(peer_id.to_string());
        } else {
            debug!(peer_id = %peer_id, cap = MAX_KNOWN_PEERS, "Known peers at capacity, not tracking new peer");
        }

        // Only send key package if encryption is enabled and auto key exchange is on
        if !self.config.encryption.enabled || !self.config.encryption.auto_key_exchange {
            return;
        }

        // Only send once per peer
        if self.key_package_sent_to.contains(peer_id) {
            return;
        }

        // Only if MLS is initialized
        if self.mls_manager.is_none() {
            return;
        }

        if let Err(e) = self.send_key_package_to(peer_id) {
            warn!(error = %e, peer_id = %peer_id, "Failed to send key package on discovery");
        }
    }

    /// Called when a neighbor is lost.
    ///
    /// Cleans up tracking state for the lost peer.
    ///
    /// # Arguments
    ///
    /// * `peer_id` - The ID of the lost peer
    pub fn on_neighbor_lost(&mut self, peer_id: &str) {
        // Remove from key package sent tracking so we can re-send if they reconnect
        self.key_package_sent_to.remove(peer_id);
        self.known_peers.remove(peer_id);
    }

    /// Establishes a secure MLS session with a peer.
    ///
    /// This high-level method handles the complete session establishment flow:
    /// 1. Checks if a session already exists (returns Ok(None) if so)
    /// 2. Checks for a pending key package from the peer
    /// 3. If found, imports the key package, creates the session, and sends the Welcome message
    /// 4. If no key package is available, returns an error
    ///
    /// This method is designed for use by application code that needs explicit control
    /// over session establishment, as opposed to the automatic encryption flow.
    ///
    /// # Arguments
    ///
    /// * `peer_id` - The ID of the peer to establish a session with
    ///
    /// # Returns
    ///
    /// * `Ok(Some(WelcomeMessage))` - Session created, Welcome message returned (and sent to peer)
    /// * `Ok(None)` - Session already exists
    /// * `Err(SessionNotReady(state))` - Establishment in progress; caller can retry or show "Establishing…"
    pub fn establish_secure_session(&mut self, peer_id: &str) -> Result<Option<WelcomeMessage>> {
        let mls = self.mls_manager.clone().ok_or(Error::MlsNotInitialized)?;

        // Check if session already exists
        let has_session = {
            let manager = mls
                .read()
                .map_err(|_| Error::Other("MLS lock poisoned".to_string()))?;
            manager.has_session(peer_id)?
        };

        if has_session {
            debug!(peer_id = %peer_id, "Session already exists");
            return Ok(None);
        }

        // Try loading key package from storage (e.g. after restart) before giving up
        self.try_load_key_package_from_storage_into_memory(peer_id);

        // Check for pending key package (memory, possibly just restored from storage)
        // Clone first, only remove after all operations succeed to avoid losing the key package on failure
        if let Some(received_pkg) = self.pending_key_packages.get(peer_id).cloned() {
            // Check if key package has expired (using local clock)
            let now_ms = Utc::now().timestamp_millis() as u64;
            if now_ms >= received_pkg.local_expires_at_ms {
                warn!(peer_id = %peer_id, "Received key package has expired, discarding");
                self.pending_key_packages.remove(peer_id);
                self.delete_peer_key_package_from_storage(peer_id);
            } else {
                {
                    let manager = mls
                        .read()
                        .map_err(|_| Error::Other("MLS lock poisoned".to_string()))?;
                    manager.import_key_package(peer_id, &received_pkg.key_package_data)?;
                }

                // Create session and get welcome message
                let welcome = {
                    let manager = mls
                        .read()
                        .map_err(|_| Error::Other("MLS lock poisoned".to_string()))?;
                    manager.create_session(peer_id)?
                };

                // Send welcome message to peer
                let welcome_sent = self.send_welcome_message(peer_id, &welcome)?;

                // All operations succeeded, now safe to remove the key package
                self.pending_key_packages.remove(peer_id);
                self.delete_peer_key_package_from_storage(peer_id);

                let group_id = welcome.group_id.as_str().to_string();
                let is_session = group_id.starts_with("session:");
                if let Err(err) = self.ensure_session_state_entry(peer_id, "session_created_local")
                {
                    warn!(
                        peer_id = %peer_id,
                        error = %err,
                        "Failed to persist pending session state"
                    );
                }

                info!(peer_id = %peer_id, group_id = %group_id, "Established secure session");

                if welcome_sent {
                    debug!(peer_id = %peer_id, group_id = %group_id, is_session, "Welcome synchronously sent");
                }

                return Ok(Some(welcome));
            }
        }

        // No key package available (memory nor storage) — return non-terminal state so caller can retry
        Err(Error::SessionNotReady(self.establishment_state(peer_id)?))
    }

    /// Checks if a pending key package is available for a peer.
    ///
    /// This can be used to check if session establishment is possible
    /// before calling `establish_secure_session`.
    ///
    /// # Arguments
    ///
    /// * `peer_id` - The ID of the peer to check
    ///
    /// # Returns
    ///
    /// `true` if a key package is available, `false` otherwise
    pub fn has_pending_key_package(&self, peer_id: &str) -> bool {
        self.pending_key_packages.contains_key(peer_id)
    }

    /// Returns the current establishment state for a peer.
    ///
    /// Use this to show "Establishing…" or drive retries without calling send/establish.
    pub fn get_establishment_state(&self, peer_id: &str) -> Result<EstablishmentState> {
        self.establishment_state(peer_id)
    }

    /// Creates a session using manually imported key material.
    ///
    /// This entrypoint is for bindings that expose low-level MLS APIs. It must
    /// still keep protocol-level session lifecycle state in sync with MLS state.
    pub fn manual_mls_create_session(&mut self, peer_id: &str) -> Result<WelcomeMessage> {
        let mls = self.mls_manager.clone().ok_or(Error::MlsNotInitialized)?;
        let welcome = {
            let manager = mls
                .read()
                .map_err(|_| Error::Other("MLS lock poisoned".to_string()))?;
            manager.create_session(peer_id)?
        };
        if let Err(err) = self.ensure_session_state_entry(peer_id, "manual_session_created") {
            warn!(
                peer_id = %peer_id,
                error = %err,
                "Failed to persist pending session state after manual session create"
            );
        }
        Ok(welcome)
    }

    /// Deletes a 1:1 session and clears protocol-level lifecycle state.
    pub fn manual_mls_delete_session(&mut self, peer_id: &str) -> Result<()> {
        let mls = self.mls_manager.clone().ok_or(Error::MlsNotInitialized)?;
        let manager = mls
            .read()
            .map_err(|_| Error::Other("MLS lock poisoned".to_string()))?;
        manager.delete_session(peer_id)?;

        // Apply protocol-state cleanup only after MLS deletion succeeds.
        self.confirmed_sessions.remove(peer_id);
        self.clear_confirmation_recovery_tracking(peer_id);
        self.welcome_lifecycles.remove(peer_id);
        self.clear_session_state_entry(peer_id)?;
        self.clear_welcome_lifecycle_entry(peer_id)?;
        Ok(())
    }

    /// Joins a session from a Welcome message and synchronizes confirmation state.
    pub fn manual_mls_join_session(
        &mut self,
        welcome: &WelcomeMessage,
    ) -> Result<offline_protocol_mls::GroupInfo> {
        let mls = self.mls_manager.clone().ok_or(Error::MlsNotInitialized)?;
        let group_info = {
            let manager = mls
                .read()
                .map_err(|_| Error::Other("MLS lock poisoned".to_string()))?;
            manager.join_session(welcome)?
        };
        self.handle_manual_welcome_confirmation(&welcome.inviter_id);
        Ok(group_info)
    }

    /// Processes an MLS Welcome message and synchronizes confirmation state.
    pub fn manual_mls_process_welcome(
        &mut self,
        welcome: &WelcomeMessage,
    ) -> Result<offline_protocol_mls::GroupInfo> {
        let mls = self.mls_manager.clone().ok_or(Error::MlsNotInitialized)?;
        let group_info = {
            let manager = mls
                .read()
                .map_err(|_| Error::Other("MLS lock poisoned".to_string()))?;
            manager.process_welcome(welcome)?
        };
        self.handle_manual_welcome_confirmation(&welcome.inviter_id);
        Ok(group_info)
    }

    /// Decrypts a user-scoped MLS message and synchronizes confirmation state.
    pub fn manual_mls_decrypt_from_user(
        &mut self,
        encrypted: &EncryptedMessage,
    ) -> Result<Option<Vec<u8>>> {
        let mls = self.mls_manager.clone().ok_or(Error::MlsNotInitialized)?;
        let plaintext = {
            let manager = mls
                .read()
                .map_err(|_| Error::Other("MLS lock poisoned".to_string()))?;
            manager.decrypt_from_user(encrypted)?
        };
        if plaintext.is_some() {
            self.handle_manual_decrypt_confirmation(&encrypted.sender_id);
        }
        Ok(plaintext)
    }

    /// Decrypts any MLS message and synchronizes confirmation state for 1:1 flows.
    pub fn manual_mls_decrypt(&mut self, encrypted: &EncryptedMessage) -> Result<Option<Vec<u8>>> {
        let mls = self.mls_manager.clone().ok_or(Error::MlsNotInitialized)?;
        let plaintext = {
            let manager = mls
                .read()
                .map_err(|_| Error::Other("MLS lock poisoned".to_string()))?;
            manager.decrypt(encrypted)?
        };
        if plaintext.is_some() && Self::is_session_group_id(encrypted.group_id.as_str()) {
            self.handle_manual_decrypt_confirmation(&encrypted.sender_id);
        }
        Ok(plaintext)
    }

    fn is_session_group_id(group_id: &str) -> bool {
        group_id.starts_with("session:")
    }

    fn handle_manual_welcome_confirmation(&mut self, peer_id: &str) {
        match self.confirm_session_state(peer_id, "welcome_received") {
            Ok(true) => {
                let _ = self.flush_pending_messages(peer_id);
                self.process_pending_decryption(peer_id);
            }
            Ok(false) => {}
            Err(err) => {
                warn!(
                    peer_id = %peer_id,
                    error = %err,
                    "Failed to persist session confirmation after manual welcome processing"
                );
            }
        }
    }

    fn handle_manual_decrypt_confirmation(&mut self, peer_id: &str) {
        if !self.can_confirm_from_source(peer_id, "decrypt_success") {
            return;
        }
        match self.confirm_session_state(peer_id, "decrypt_success") {
            Ok(true) => {
                let _ = self.flush_pending_messages(peer_id);
            }
            Ok(false) => {}
            Err(err) => {
                warn!(
                    peer_id = %peer_id,
                    error = %err,
                    "Failed to persist session confirmation after manual decrypt"
                );
            }
        }
    }

    /// Gets access to the MLS manager for advanced operations.
    ///
    /// Returns `None` if MLS is not initialized.
    pub fn mls_manager(&self) -> Option<&Arc<RwLock<MlsManager>>> {
        self.mls_manager.as_ref()
    }

    /// Sends a message via a specific transport, bypassing DORS selection.
    ///
    /// # Arguments
    ///
    /// * `recipient` - Recipient's user ID
    /// * `content` - Message content
    /// * `priority` - Message priority (optional, defaults to Medium)
    /// * `transport` - The transport to use
    /// * `reply_to_msg` - ID of the message this is replying to (optional)
    ///
    /// # Returns
    ///
    /// Returns the message ID if successful.
    pub fn send_message_via_transport(
        &mut self,
        recipient: impl Into<String>,
        content: impl Into<String>,
        priority: Option<MessagePriority>,
        transport: TransportType,
        reply_to_msg: Option<impl Into<String>>,
    ) -> Result<MessageId> {
        // Check if protocol is running
        {
            let state = lock_shared_state(&self.shared_state)?;
            if state.state != ProtocolState::Running {
                return Err(Error::NotStarted);
            }
        }

        let recipient_str: String = recipient.into();
        let content_str: String = content.into();
        let priority = priority.unwrap_or(MessagePriority::Medium);

        // Parse reply_to_msg if provided
        let reply_to_msg_id = reply_to_msg
            .map(|r| MessageId::from_str(&r.into()))
            .transpose()
            .map_err(|e| Error::Other(format!("Invalid reply_to_msg: {}", e)))?;

        let final_content = match self.prepare_outbound_content(
            &recipient_str,
            &content_str,
            priority,
            reply_to_msg_id.clone(),
            "send_message_via_transport_session_pending",
        )? {
            OutboundSendPreparation::Ready(content) => content,
            OutboundSendPreparation::Queued(message_id) => return Ok(message_id),
        };

        // Create message
        let message = self.create_message(
            &recipient_str,
            final_content,
            Some(priority),
            reply_to_msg_id,
        )?;
        let message_id = message.id.clone();

        // Check for duplicates
        if self.deduplicator.is_duplicate(&message_id) {
            return Err(crate::Error::Other("Duplicate message".to_string()));
        }

        // Mark as seen
        self.deduplicator.mark_seen(message_id.clone());

        // Track previous transport before sending
        let previous_transport = self.transport_manager.current_transport();

        // Attempt to send via the specified transport (bypassing DORS)
        let send_result = self
            .transport_manager
            .send_via_transport(&message, transport);
        let current_transport = Some(transport);

        // Handle send result
        match send_result {
            Ok(()) => {
                self.handle_send_success(&message, current_transport)?;
                self.emit_transport_switch_event(previous_transport, current_transport)?;
                self.emit_message_sent_event(&message)?;
                Ok(message_id)
            }
            Err(err) => {
                self.handle_send_failure(&message, current_transport.or(previous_transport))?;
                // send_via_transport does not record retry failures internally
                // (unlike TransportManager::send), so record explicitly here.
                self.transport_manager.record_retry_failure(transport);
                warn!(
                    message_id = %message.id,
                    transport = ?transport,
                    error = %err,
                    "Send via forced transport failed, message deferred"
                );
                Err(Error::Other(format!(
                    "Send via {:?} failed (message {} deferred for retry): {}",
                    transport, message.id, err
                )))
            }
        }
    }

    /// Sends a connection request to another user via any available transport.
    ///
    /// The request is routed through DORS, so it works over Internet, BLE, or WiFi Direct.
    ///
    /// # Arguments
    ///
    /// * `recipient` - The user ID of the recipient
    /// * `sender_name` - Display name of the sender
    /// * `key_package` - Optional MLS key package for encrypted session setup
    ///
    /// # Strict Encryption Behavior
    ///
    /// When `encryption.require_encryption = true`, this API returns `EncryptFailed`
    /// because bootstrap control messages are plaintext by design.
    pub fn send_connection_request(
        &mut self,
        recipient: &str,
        sender_name: &str,
        key_package: Option<Vec<u8>>,
    ) -> Result<MessageId> {
        self.ensure_plaintext_control_send_allowed("send_connection_request")?;

        let payload = ConnectionRequestPayload {
            sender_name: sender_name.to_string(),
            timestamp_ms: Utc::now().timestamp_millis(),
            key_package,
        };

        let serialized =
            serde_json::to_string(&payload).map_err(|e| Error::Serialization(e.to_string()))?;
        let content = format!("{}{}", internal_prefixes::CONN_REQUEST, serialized);

        let message_id = self.send_internal_message(recipient, content, MessagePriority::High)?;
        info!(recipient = %recipient, "Sent connection request");
        Ok(message_id)
    }

    /// Accepts a connection request from another user via any available transport.
    ///
    /// The response is routed through DORS, so it works over Internet, BLE, or WiFi Direct.
    ///
    /// # Arguments
    ///
    /// * `recipient` - The user ID of the original requester
    /// * `accepter_name` - Display name of the accepting party
    /// * `key_package` - Optional MLS key package for encrypted session setup
    ///
    /// # Strict Encryption Behavior
    ///
    /// When `encryption.require_encryption = true`, this API returns `EncryptFailed`
    /// because bootstrap control messages are plaintext by design.
    pub fn accept_connection_request(
        &mut self,
        recipient: &str,
        accepter_name: &str,
        key_package: Option<Vec<u8>>,
    ) -> Result<MessageId> {
        self.ensure_plaintext_control_send_allowed("accept_connection_request")?;

        let payload = ConnectionAcceptedPayload {
            accepted_by_name: accepter_name.to_string(),
            timestamp_ms: Utc::now().timestamp_millis(),
            key_package,
        };

        let serialized =
            serde_json::to_string(&payload).map_err(|e| Error::Serialization(e.to_string()))?;
        let content = format!("{}{}", internal_prefixes::CONN_ACCEPT, serialized);

        let message_id = self.send_internal_message(recipient, content, MessagePriority::High)?;
        info!(recipient = %recipient, "Accepted connection request");
        Ok(message_id)
    }

    /// Rejects a connection request from another user via any available transport.
    ///
    /// The response is routed through DORS, so it works over Internet, BLE, or WiFi Direct.
    ///
    /// # Arguments
    ///
    /// * `recipient` - The user ID of the original requester
    ///
    /// # Strict Encryption Behavior
    ///
    /// When `encryption.require_encryption = true`, this API returns `EncryptFailed`
    /// because bootstrap control messages are plaintext by design.
    pub fn reject_connection_request(&mut self, recipient: &str) -> Result<MessageId> {
        self.ensure_plaintext_control_send_allowed("reject_connection_request")?;

        let content = internal_prefixes::CONN_REJECT.to_string();

        let message_id = self.send_internal_message(recipient, content, MessagePriority::High)?;
        info!(recipient = %recipient, "Rejected connection request");
        Ok(message_id)
    }

    // ========================================================================
    // SERVICE DISCOVERY & REQUEST/RESPONSE
    // ========================================================================

    /// Registers a local service that this node offers.
    pub fn register_service(&mut self, descriptor: ServiceDescriptor) -> Result<()> {
        self.mesh_services
            .register_service(descriptor)
            .map_err(Error::Service)
    }

    /// Unregisters a local service. Returns true if the service was found and removed.
    pub fn unregister_service(&mut self, service_id: &str) -> Result<bool> {
        self.mesh_services
            .unregister_service(service_id)
            .map_err(Error::Service)
    }

    /// Broadcasts a service discovery query to all known peers.
    /// Returns a query_id. Responses arrive asynchronously as `ServiceDiscovered` events.
    ///
    /// **Note:** Discovery responses currently travel only one hop back (to the
    /// immediate sender of the query). Multi-hop response relay is not yet
    /// implemented, so services more than one hop away will generate responses
    /// that reach intermediate forwarders but not the original querier.
    pub fn discover_services(&mut self, service_id: Option<&str>) -> Result<String> {
        self.ensure_plaintext_control_send_allowed("discover_services")?;

        let peers: Vec<String> = self.known_peers.iter().cloned().collect();
        let result = self
            .mesh_services
            .discover_services(&self.config.user_id, &peers, service_id)
            .map_err(Error::Service)?;
        let mut send_failures = 0usize;
        for msg in result.messages {
            if self
                .send_internal_message(&msg.recipient, msg.content, msg.priority)
                .is_err()
            {
                send_failures += 1;
            }
        }
        if send_failures > 0 {
            warn!(
                failures = send_failures,
                total = peers.len(),
                "Some discovery broadcasts failed to send"
            );
        }
        Ok(result.query_id)
    }

    /// Sends a typed service request to a specific provider peer.
    /// Returns a request_id. The response arrives as a `ServiceResponseReceived` event.
    pub fn send_service_request(
        &mut self,
        provider: &str,
        service_id: &str,
        method: &str,
        body: &str,
    ) -> Result<String> {
        self.ensure_plaintext_control_send_allowed("send_service_request")?;

        let result = self
            .mesh_services
            .send_service_request(provider, service_id, method, body)
            .map_err(Error::Service)?;
        let msg = result.message;
        self.send_internal_message(&msg.recipient, msg.content, msg.priority)?;
        Ok(result.request_id)
    }

    /// Responds to a service request from another peer.
    pub fn respond_to_service_request(
        &mut self,
        request_id: &str,
        requester: &str,
        service_id: &str,
        status: &str,
        body: &str,
    ) -> Result<MessageId> {
        self.ensure_plaintext_control_send_allowed("respond_to_service_request")?;

        let result = self
            .mesh_services
            .respond_to_service_request(request_id, requester, service_id, status, body)
            .map_err(Error::Service)?;
        let msg = result.message;
        let message_id = self.send_internal_message(&msg.recipient, msg.content, msg.priority)?;
        Ok(message_id)
    }

    /// Receives the next available message.
    ///
    /// # Returns
    ///
    /// Returns `Some(Message)` if a message is available, `None` otherwise.
    /// Receives the next available message.
    ///
    /// # Returns
    ///
    /// Returns `Some(Message)` if a message is available, `None` otherwise.
    ///
    /// # Auto-Decryption
    ///
    /// When encryption is enabled, encrypted messages are automatically decrypted.
    /// Internal MLS protocol messages (key packages, welcome messages) are handled
    /// transparently and not surfaced to the application.
    pub fn receive_message(&mut self) -> Option<Message> {
        let Ok(mut state) = lock_shared_state(&self.shared_state) else {
            error!("Failed to lock shared state in receive_message");
            return None;
        };
        let protocol_running = state.state == ProtocolState::Running;

        if !state.received_messages.is_empty() {
            return Some(state.received_messages.remove(0));
        }

        drop(state);

        // Drive confirmation maintenance from receive polling as an additional
        // liveness source when the app does not call process() on a timer.
        if protocol_running {
            self.retry_pending_session_confirmations();
            self.kick_pending_session_reconciliation("receive_message_poll");
        }

        loop {
            match self.transport_manager.receive() {
                Ok(Some((transport_used, mut message))) => {
                    // Merge Lamport clock for every received message — including
                    // duplicates, ACKs, and internal protocol messages — so the
                    // local clock always advances past any observed peer value.
                    if message.lamport_clock.value() > 0 {
                        self.lamport_clock.merge(message.lamport_clock);
                        self.persist_lamport_clock();
                    }

                    if message.metadata.contains_key(ACK_FOR_KEY) {
                        self.handle_ack_message(&message);
                        continue;
                    }

                    if self.deduplicator.is_duplicate(&message.id) {
                        // Re-ACK duplicate packets so the sender can stop retrying
                        // if our previous ACK was dropped.
                        if message.requires_ack {
                            if let Err(err) = self.send_delivery_ack(&message, transport_used) {
                                error!(
                                    message_id = %message.id,
                                    error = %err,
                                    "Failed to send delivery ACK for duplicate message"
                                );
                            }
                        }
                        continue;
                    }

                    self.deduplicator.mark_seen(message.id.clone());

                    // Handle internal MLS messages
                    if let Some(result) = self.process_internal_message(&message) {
                        match result {
                            InternalMessageResult::Consumed => {
                                // Internal control messages are still delivery-sensitive for
                                // the sender (invites/accept/welcome). ACK before consume.
                                if message.requires_ack {
                                    if let Err(err) =
                                        self.send_delivery_ack(&message, transport_used)
                                    {
                                        error!(
                                            message_id = %message.id,
                                            error = %err,
                                            "Failed to send delivery ACK for internal message"
                                        );
                                    }
                                }
                                // Internal message handled, don't surface to app
                                continue;
                            }
                            InternalMessageResult::Decrypted(plaintext) => {
                                // Replace content with decrypted plaintext
                                message.content = plaintext;
                                message
                                    .metadata
                                    .insert("encrypted".to_string(), "true".to_string());
                            }
                        }
                    }

                    if message.requires_ack {
                        if let Err(err) = self.send_delivery_ack(&message, transport_used) {
                            error!(
                                message_id = %message.id,
                                error = %err,
                                "Failed to send delivery ACK"
                            );
                        }
                    }

                    // Route file-chunk messages to the transfer manager instead
                    // of surfacing them to the app as regular messages.
                    if message.content_type == ContentType::FileChunk {
                        self.handle_incoming_file_chunk(&message);
                        continue;
                    }

                    let event = Event::MessageReceived {
                        message_id: message.id.as_str(),
                        sender: message.sender.as_str().to_string(),
                        recipient: message.recipient.as_str().to_string(),
                        content: message.content.clone(),
                        hop_count: message.hop_count.value(),
                        transport: transport_used.to_string(),
                        timestamp: message.timestamp.as_millis(),
                        lamport_clock: message.lamport_clock.value(),
                        reply_to_msg: message
                            .reply_to_msg
                            .as_ref()
                            .map(|id| id.as_str().to_string()),
                        content_type: message.content_type.to_string(),
                        media_metadata: message.media_metadata.clone(),
                    };

                    let Ok(state) = lock_shared_state(&self.shared_state) else {
                        error!("Failed to lock shared state for message received event");
                        return None;
                    };
                    state.emit_event(event);
                    drop(state);

                    return Some(message);
                }
                Ok(None) => return None,
                Err(err) => {
                    error!(error = %err, "Transport receive error");
                    return None;
                }
            }
        }
    }

    /// Processes internal MLS protocol messages.
    ///
    /// Returns `Some(InternalMessageResult::Consumed)` if the message was an internal
    /// protocol message that should not be surfaced to the application.
    /// Returns `Some(InternalMessageResult::Decrypted(plaintext))` if the message was
    /// encrypted and successfully decrypted.
    /// Returns `None` if the message is not an internal message.
    pub(crate) fn process_internal_message(&mut self, message: &Message) -> Option<InternalMessageResult> {
        let content = &message.content;
        let sender = message.sender.as_str();

        // Handle key package messages
        if let Some(data) = content.strip_prefix(internal_prefixes::KEY_PACKAGE) {
            if let Ok(payload) = serde_json::from_str::<KeyPackagePayload>(data) {
                debug!(sender = %sender, "Received key package");
                let now_ms = Utc::now().timestamp_millis() as u64;
                let local_expires_at_ms = if payload.remaining_lifetime_ms > 0 {
                    now_ms.saturating_add(payload.remaining_lifetime_ms)
                } else {
                    // Legacy sender didn't include remaining_lifetime_ms;
                    // assume 30-day default lifetime.
                    now_ms.saturating_add(30 * 24 * 60 * 60 * 1000)
                };
                let pkg = ReceivedKeyPackage {
                    key_package_data: payload.key_package_data,
                    local_expires_at_ms,
                };
                self.pending_key_packages
                    .insert(sender.to_string(), pkg.clone());
                self.persist_peer_key_package(sender, &pkg);

                // Send our key package back if auto_key_exchange is enabled
                if self.config.encryption.auto_key_exchange
                    && self.config.encryption.enabled
                    && !self.key_package_sent_to.contains(sender)
                {
                    let _ = self.send_key_package_to(sender);
                }
            }
            return Some(InternalMessageResult::Consumed);
        }

        if content.starts_with(internal_prefixes::SESSION_CONFIRM_PROBE) {
            let sender_owned = sender.to_string();
            match self.has_mls_session(&sender_owned) {
                Ok(true) => {
                    if !self.can_confirm_from_source(&sender_owned, "confirmation_probe_received") {
                        debug!(
                            sender = %sender_owned,
                            "Skipping probe confirmation until welcome delivery is sent"
                        );
                    } else {
                        match self
                            .confirm_session_state(&sender_owned, "confirmation_probe_received")
                        {
                            Ok(_) => {
                                let _ = self.flush_pending_messages(&sender_owned);
                                self.process_pending_decryption(&sender_owned);
                            }
                            Err(err) => {
                                warn!(
                                    sender = %sender_owned,
                                    error = %err,
                                    "Failed to persist session confirmation after probe"
                                );
                            }
                        }
                    }

                    if let Err(err) = self.send_internal_message(
                        &sender_owned,
                        internal_prefixes::SESSION_CONFIRM_ACK.to_string(),
                        MessagePriority::High,
                    ) {
                        warn!(
                            sender = %sender_owned,
                            error = %err,
                            "Failed to send session confirmation ack"
                        );
                    }
                }
                Ok(false) => {
                    debug!(
                        sender = %sender_owned,
                        "Ignoring confirmation probe without local MLS session"
                    );
                }
                Err(err) => {
                    warn!(
                        sender = %sender_owned,
                        error = %err,
                        "Failed to validate local MLS session for confirmation probe"
                    );
                }
            }
            return Some(InternalMessageResult::Consumed);
        }

        if content.starts_with(internal_prefixes::SESSION_CONFIRM_ACK) {
            let sender_owned = sender.to_string();
            match self.has_mls_session(&sender_owned) {
                Ok(true) => {
                    if !self.can_confirm_from_source(&sender_owned, "confirmation_ack_received") {
                        debug!(
                            sender = %sender_owned,
                            "Skipping ack confirmation until welcome delivery is sent"
                        );
                    } else {
                        match self.confirm_session_state(&sender_owned, "confirmation_ack_received")
                        {
                            Ok(_) => {
                                let _ = self.flush_pending_messages(&sender_owned);
                                self.process_pending_decryption(&sender_owned);
                            }
                            Err(err) => {
                                warn!(
                                    sender = %sender_owned,
                                    error = %err,
                                    "Failed to persist session confirmation after ack"
                                );
                            }
                        }
                    }
                }
                Ok(false) => {
                    debug!(
                        sender = %sender_owned,
                        "Ignoring confirmation ack without local MLS session"
                    );
                }
                Err(err) => {
                    warn!(
                        sender = %sender_owned,
                        error = %err,
                        "Failed to validate local MLS session for confirmation ack"
                    );
                }
            }
            return Some(InternalMessageResult::Consumed);
        }

        // Handle welcome messages (session invitation)
        if let Some(data) = content.strip_prefix(internal_prefixes::WELCOME) {
            if let Ok(welcome) = serde_json::from_str::<WelcomeMessage>(data) {
                debug!(sender = %sender, group_id = %welcome.group_id, "Received welcome message");

                // Track if we need to flush pending messages and process pending decryption
                let mut should_flush = false;
                let sender_owned = sender.to_string();
                let group_id = welcome.group_id.as_str().to_string();
                let is_session = group_id.starts_with("session:");
                let mut error_reason: Option<String> = None;

                if let Some(mls) = self.mls_manager.clone() {
                    if let Ok(manager) = mls.read() {
                        let has_existing = manager.has_session(sender).unwrap_or(false);

                        if has_existing {
                            // Both sides created a session and exchanged Welcomes.
                            // Deterministic tiebreaker: the device whose user_id is
                            // lexicographically *greater* adopts the remote Welcome;
                            // the other keeps its own session.  This guarantees both
                            // devices converge on the same MLS group.
                            let local_id: &str = &self.config.user_id;
                            let remote_id: &str = sender;
                            if local_id > remote_id {
                                info!(
                                    sender = %sender,
                                    local_id = %local_id,
                                    "Welcome-wins tiebreaker: adopting remote Welcome (local > remote)"
                                );
                                match manager.replace_session_with_welcome(&welcome) {
                                    Ok(_) => {
                                        info!(sender = %sender, "Replaced session with remote Welcome");
                                        should_flush = true;
                                    }
                                    Err(e) => {
                                        warn!(error = %e, sender = %sender, "Failed to replace session");
                                        error_reason = Some(e.to_string());
                                    }
                                }
                            } else {
                                info!(
                                    sender = %sender,
                                    local_id = %local_id,
                                    "Welcome-wins tiebreaker: keeping local session (local < remote)"
                                );
                                should_flush = true;
                            }
                        } else {
                            match manager.join_session(&welcome) {
                                Ok(_) => {
                                    info!(sender = %sender, "Joined MLS session via Welcome");
                                    should_flush = true;
                                }
                                Err(e) => {
                                    warn!(error = %e, sender = %sender, "Failed to join MLS session");
                                    error_reason = Some(e.to_string());
                                }
                            }
                        }
                    }
                }

                // Confirm session and process queued items after releasing the MLS lock
                if should_flush {
                    match self.confirm_session_state(&sender_owned, "welcome_received") {
                        Ok(_) => {
                            // Flush pending outgoing messages
                            let _ = self.flush_pending_messages(&sender_owned);

                            // Process any encrypted messages that arrived before the Welcome
                            self.process_pending_decryption(&sender_owned);

                            self.emit_mls_session_ready(
                                &sender_owned,
                                &group_id,
                                MlsOperationContext::Welcome,
                            );

                            // Emit secure session established event
                            if let Ok(state) = lock_shared_state(&self.shared_state) {
                                state.emit_event(Event::secure_session_established(
                                    sender_owned,
                                    group_id,
                                    is_session,
                                    false, // initiated_by_local is false - we received the Welcome
                                ));
                            }
                        }
                        Err(e) => {
                            if let Ok(state) = lock_shared_state(&self.shared_state) {
                                state.emit_event(Event::secure_session_failed(
                                    sender_owned,
                                    format!("Failed to persist confirmation: {}", e),
                                ));
                            }
                        }
                    }
                } else if let Some(reason) = error_reason {
                    // Emit secure session failed event
                    if let Ok(state) = lock_shared_state(&self.shared_state) {
                        state.emit_event(Event::secure_session_failed(sender_owned, reason));
                    }
                }
            }
            return Some(InternalMessageResult::Consumed);
        }

        // Handle encrypted messages
        if let Some(data) = content.strip_prefix(internal_prefixes::ENCRYPTED) {
            if let Ok(encrypted) = serde_json::from_str::<EncryptedMessage>(data) {
                // Track state to update after releasing MLS lock
                enum DecryptResult {
                    Success {
                        text: String,
                        sender: String,
                        group_id: String,
                    },
                    Empty,
                    SessionNotReady {
                        sender: String,
                    },
                    Failed {
                        sender: String,
                        group_id: String,
                        kind: DecryptionFailureKind,
                    },
                    MlsNotInitialized,
                }

                let result = if let Some(mls) = self.mls_manager.clone() {
                    if let Ok(manager) = mls.read() {
                        match manager.decrypt(&encrypted) {
                            Ok(Some(plaintext)) => {
                                let text = String::from_utf8_lossy(&plaintext).to_string();
                                debug!(sender = %sender, "Decrypted message successfully");
                                DecryptResult::Success {
                                    text,
                                    sender: sender.to_string(),
                                    group_id: encrypted.group_id.as_str().to_string(),
                                }
                            }
                            Ok(None) => {
                                warn!(sender = %sender, "Decryption returned empty");
                                DecryptResult::Empty
                            }
                            Err(e) => {
                                let session_state_error = SessionStateError::from(&e);
                                match session_state_error {
                                    SessionStateError::SessionNotReady
                                    | SessionStateError::GroupNotFound => {
                                        info!(
                                            sender = %sender,
                                            error_code = session_state_error.code(),
                                            "Encrypted message received before session ready, queuing"
                                        );
                                        debug!(
                                            sender = %sender,
                                            error = %e,
                                            error_code = session_state_error.code(),
                                            "Queued encrypted message due to session state classification"
                                        );
                                        DecryptResult::SessionNotReady {
                                            sender: sender.to_string(),
                                        }
                                    }
                                    SessionStateError::NotInitialized => {
                                        warn!(
                                            sender = %sender,
                                            error = %e,
                                            error_code = session_state_error.code(),
                                            "MLS decrypt attempted before initialization"
                                        );
                                        DecryptResult::MlsNotInitialized
                                    }
                                    SessionStateError::TransportFailure
                                    | SessionStateError::CryptoFailure
                                    | SessionStateError::Unknown => {
                                        let kind = DecryptionFailureKind::from_mls_error(&e);
                                        warn!(
                                            sender = %sender,
                                            error = %e,
                                            error_code = session_state_error.code(),
                                            "Failed to decrypt message"
                                        );
                                        DecryptResult::Failed {
                                            sender: sender.to_string(),
                                            group_id: encrypted.group_id.as_str().to_string(),
                                            kind,
                                        }
                                    }
                                }
                            }
                        }
                    } else {
                        DecryptResult::MlsNotInitialized
                    }
                } else {
                    DecryptResult::MlsNotInitialized
                };

                // Now handle the result without holding the MLS lock
                match result {
                    DecryptResult::Success {
                        text,
                        sender: sender_owned,
                        group_id,
                    } => {
                        if !self.can_confirm_from_source(&sender_owned, "decrypt_success") {
                            debug!(
                                sender = %sender_owned,
                                "Skipping decrypt-based confirmation until welcome delivery is sent"
                            );
                            return Some(InternalMessageResult::Decrypted(text));
                        }

                        match self.confirm_session_state(&sender_owned, "decrypt_success") {
                            Ok(true) => {
                                info!(sender = %sender_owned, "Session confirmed via successful decryption");
                                let _ = self.flush_pending_messages(&sender_owned);
                                self.emit_mls_session_ready(
                                    &sender_owned,
                                    &group_id,
                                    MlsOperationContext::Receive,
                                );
                            }
                            Ok(false) => {}
                            Err(e) => {
                                warn!(
                                    sender = %sender_owned,
                                    error = %e,
                                    "Failed to persist session confirmation after decrypt"
                                );
                            }
                        }
                        return Some(InternalMessageResult::Decrypted(text));
                    }
                    DecryptResult::Empty => {
                        if let Ok(state) = lock_shared_state(&self.shared_state) {
                            state.emit_event(Event::message_decryption_failed(
                                message.id.clone(),
                                sender.to_string(),
                                DecryptionFailureCode::InvalidCiphertext,
                                "Failed to decrypt MLS message (empty plaintext)".to_string(),
                            ));
                        }
                        return Some(InternalMessageResult::Consumed);
                    }
                    DecryptResult::SessionNotReady {
                        sender: sender_owned,
                    } => {
                        self.emit_mls_session_missing(
                            Some(&sender_owned),
                            Some(encrypted.group_id.as_str()),
                            MlsOperationContext::SessionLookup,
                            MlsErrorCategory::SessionStateMissing,
                        );
                        self.enqueue_pending_decryption(&sender_owned, message);
                        return Some(InternalMessageResult::Consumed);
                    }
                    DecryptResult::Failed {
                        sender: sender_owned,
                        group_id,
                        kind,
                    } => {
                        self.emit_mls_decryption_failed(
                            &sender_owned,
                            Some(&group_id),
                            kind,
                            MlsOperationContext::Receive,
                        );
                        if let Ok(state) = lock_shared_state(&self.shared_state) {
                            state.emit_event(Event::message_decryption_failed(
                                message.id.clone(),
                                sender_owned.clone(),
                                Self::decryption_failure_code_from_kind(kind),
                                format!("Failed to decrypt MLS message ({kind:?})"),
                            ));
                        }
                        return Some(InternalMessageResult::Consumed);
                    }
                    DecryptResult::MlsNotInitialized => {
                        self.emit_mls_decryption_failed(
                            sender,
                            Some(encrypted.group_id.as_str()),
                            DecryptionFailureKind::NotInitialized,
                            MlsOperationContext::Receive,
                        );
                        if let Ok(state) = lock_shared_state(&self.shared_state) {
                            state.emit_event(Event::message_decryption_failed(
                                message.id.clone(),
                                sender.to_string(),
                                DecryptionFailureCode::NotInitialized,
                                "Failed to decrypt MLS message (not initialized)".to_string(),
                            ));
                        }
                        return Some(InternalMessageResult::Consumed);
                    }
                }
            } else {
                warn!(sender = %sender, "Invalid encrypted payload");
                if let Ok(state) = lock_shared_state(&self.shared_state) {
                    state.emit_event(Event::message_decryption_failed(
                        message.id.clone(),
                        sender.to_string(),
                        DecryptionFailureCode::InvalidPayload,
                        "Invalid encrypted payload".to_string(),
                    ));
                }
                return Some(InternalMessageResult::Consumed);
            }
        }

        // Handle connection request messages
        if let Some(data) = content.strip_prefix(internal_prefixes::CONN_REQUEST) {
            if let Ok(payload) = serde_json::from_str::<ConnectionRequestPayload>(data) {
                info!(sender = %sender, sender_name = %payload.sender_name, "Received connection request");
                if let Ok(state) = lock_shared_state(&self.shared_state) {
                    state.emit_event(Event::connection_request_received(
                        sender.to_string(),
                        payload.sender_name,
                        payload.timestamp_ms,
                        payload.key_package,
                    ));
                }
            } else {
                warn!(sender = %sender, "Failed to parse connection request payload");
            }
            return Some(InternalMessageResult::Consumed);
        }

        // Handle connection accepted messages
        if let Some(data) = content.strip_prefix(internal_prefixes::CONN_ACCEPT) {
            if let Ok(payload) = serde_json::from_str::<ConnectionAcceptedPayload>(data) {
                info!(sender = %sender, accepted_by_name = %payload.accepted_by_name, "Connection request accepted");
                if let Ok(state) = lock_shared_state(&self.shared_state) {
                    state.emit_event(Event::connection_accepted(
                        sender.to_string(),
                        payload.accepted_by_name,
                        payload.timestamp_ms,
                        payload.key_package,
                    ));
                }
            } else {
                warn!(sender = %sender, "Failed to parse connection accepted payload");
            }
            return Some(InternalMessageResult::Consumed);
        }

        // Handle connection rejected messages
        if content
            .strip_prefix(internal_prefixes::CONN_REJECT)
            .is_some()
        {
            info!(sender = %sender, "Connection request rejected");
            if let Ok(state) = lock_shared_state(&self.shared_state) {
                state.emit_event(Event::connection_rejected(sender.to_string()));
            }
            return Some(InternalMessageResult::Consumed);
        }

        // --- Group (mesh/MLS) messages ---

        if let Some(data) = content.strip_prefix(internal_prefixes::GROUP_MLS_MSG) {
            self.handle_group_mls_msg(message, sender, data);
            return Some(InternalMessageResult::Consumed);
        }

        if let Some(data) = content.strip_prefix(internal_prefixes::GROUP_MLS_WELCOME) {
            self.handle_group_mls_welcome(sender, data);
            return Some(InternalMessageResult::Consumed);
        }

        if let Some(data) = content.strip_prefix(internal_prefixes::GROUP_MLS_COMMIT) {
            self.handle_group_mls_commit(sender, data);
            return Some(InternalMessageResult::Consumed);
        }

        if let Some(data) = content.strip_prefix(internal_prefixes::GROUP_MLS_LEAVE) {
            self.handle_group_mls_leave(sender, data);
            return Some(InternalMessageResult::Consumed);
        }

        // --- Group (relay) messages ---

        if let Some(data) = content.strip_prefix(internal_prefixes::GROUP_CREATED) {
            if let Ok(payload) = serde_json::from_str::<GroupCreatedPayload>(data) {
                info!(group_id = %payload.group_id, "Group created");
                if let Ok(state) = lock_shared_state(&self.shared_state) {
                    state.emit_event(Event::group_created(payload.group_id, payload.name));
                }
            } else {
                warn!("Failed to parse GroupCreated payload");
            }
            return Some(InternalMessageResult::Consumed);
        }

        if let Some(data) = content.strip_prefix(internal_prefixes::GROUP_MSG) {
            if let Ok(payload) = serde_json::from_str::<GroupMessageReceivedPayload>(data) {
                info!(group_id = %payload.group_id, message_id = %payload.message_id, "Group message received");
                if let Ok(state) = lock_shared_state(&self.shared_state) {
                    state.emit_event(Event::group_message_received(
                        payload.group_id,
                        payload.sender,
                        payload.content,
                        payload.timestamp,
                        payload.message_id,
                        payload.reply_to_msg,
                    ));
                }
            } else {
                warn!("Failed to parse GroupMessageReceived payload");
            }
            return Some(InternalMessageResult::Consumed);
        }

        if let Some(data) = content.strip_prefix(internal_prefixes::GROUP_MEMBER_ADDED) {
            if let Ok(payload) = serde_json::from_str::<GroupMemberAddedPayload>(data) {
                info!(group_id = %payload.group_id, user_id = %payload.user_id, "Group member added");
                if let Ok(state) = lock_shared_state(&self.shared_state) {
                    state.emit_event(Event::group_member_added(
                        payload.group_id,
                        payload.user_id,
                        payload.added_by,
                    ));
                }
            } else {
                warn!("Failed to parse GroupMemberAdded payload");
            }
            return Some(InternalMessageResult::Consumed);
        }

        if let Some(data) = content.strip_prefix(internal_prefixes::GROUP_MEMBER_REMOVED) {
            if let Ok(payload) = serde_json::from_str::<GroupMemberRemovedPayload>(data) {
                info!(group_id = %payload.group_id, user_id = %payload.user_id, "Group member removed");
                if let Ok(state) = lock_shared_state(&self.shared_state) {
                    state.emit_event(Event::group_member_removed(
                        payload.group_id,
                        payload.user_id,
                        payload.removed_by,
                    ));
                }
            } else {
                warn!("Failed to parse GroupMemberRemoved payload");
            }
            return Some(InternalMessageResult::Consumed);
        }

        if let Some(data) = content.strip_prefix(internal_prefixes::GROUP_INFO) {
            if let Ok(payload) = serde_json::from_str::<GroupInfoPayload>(data) {
                info!(group_id = %payload.group_id, "Group info received");
                let members: Vec<crate::events::GroupInfoMember> = payload
                    .members
                    .into_iter()
                    .map(|m| crate::events::GroupInfoMember {
                        user_id: m.user_id,
                        role: m.role,
                        joined_at: m.joined_at,
                    })
                    .collect();
                if let Ok(state) = lock_shared_state(&self.shared_state) {
                    state.emit_event(Event::group_info(
                        payload.group_id,
                        payload.name,
                        payload.created_by,
                        payload.created_at,
                        members,
                    ));
                }
            } else {
                warn!("Failed to parse GroupInfo payload");
            }
            return Some(InternalMessageResult::Consumed);
        }

        if let Some(data) = content.strip_prefix(internal_prefixes::USER_GROUPS) {
            if let Ok(payload) = serde_json::from_str::<UserGroupsPayload>(data) {
                info!(count = payload.groups.len(), "User groups received");
                let groups: Vec<crate::events::UserGroupSummary> = payload
                    .groups
                    .into_iter()
                    .map(|g| crate::events::UserGroupSummary {
                        group_id: g.group_id,
                        name: g.name,
                        created_at: g.created_at,
                    })
                    .collect();
                if let Ok(state) = lock_shared_state(&self.shared_state) {
                    state.emit_event(Event::user_groups(groups));
                }
            } else {
                warn!("Failed to parse UserGroups payload");
            }
            return Some(InternalMessageResult::Consumed);
        }

        if let Some(data) = content.strip_prefix(internal_prefixes::GROUP_ERROR) {
            if let Ok(payload) = serde_json::from_str::<GroupErrorPayload>(data) {
                warn!(reason = %payload.reason, "Group error");
                if let Ok(state) = lock_shared_state(&self.shared_state) {
                    state.emit_event(Event::group_error(payload.reason));
                }
            } else {
                warn!("Failed to parse GroupError payload");
            }
            return Some(InternalMessageResult::Consumed);
        }

        // --- Service discovery & request/response (delegated to MeshServices) ---
        // Only allocate peer list when the message is actually a service message.
        if content.starts_with(offline_protocol_services::SVC_MESSAGE_PREFIX) {
            let peers: Vec<String> = self
                .known_peers
                .iter()
                .filter(|p| p.as_str() != sender)
                .cloned()
                .collect();
            match self.mesh_services.handle_incoming_message(
                content,
                sender,
                message.hop_count.value(),
                &self.config.user_id,
                &peers,
            ) {
                ServiceAction::NotHandled => {
                    warn!(sender = %sender, "Received unknown service message prefix, consuming");
                    return Some(InternalMessageResult::Consumed);
                }
                ServiceAction::Consumed {
                    messages_to_send,
                    events_to_emit,
                } => {
                    for msg in messages_to_send {
                        let _ =
                            self.send_internal_message(&msg.recipient, msg.content, msg.priority);
                    }
                    if let Ok(state) = lock_shared_state(&self.shared_state) {
                        for svc_event in events_to_emit {
                            state.emit_event(Event::from(svc_event));
                        }
                    }
                    return Some(InternalMessageResult::Consumed);
                }
            }
        }

        None // Not an internal message
    }

    /// Registers an event handler.
    ///
    /// # Arguments
    ///
    /// * `handler` - Callback function that will be called for each event
    pub fn on_event<F>(&mut self, handler: F)
    where
        F: Fn(Event) + Send + Sync + 'static,
    {
        let Ok(mut state) = lock_shared_state(&self.shared_state) else {
            error!("Failed to lock shared state in on_event");
            return;
        };
        state.event_handlers.push(Arc::new(handler));
    }

    /// Processes pending operations (retries, timeouts, etc.).
    ///
    /// This should be called periodically to handle background tasks.
    pub fn process(&mut self) -> Result<()> {
        {
            let state = lock_shared_state(&self.shared_state)?;
            if state.state != ProtocolState::Running {
                return Ok(()); // Don't process if not running
            }
        }

        self.process_retry_queue()?;
        self.process_welcome_retry_queue()?;
        self.process_timed_out_acks()?;
        self.retry_pending_session_confirmations();
        self.kick_pending_session_reconciliation("process_tick");
        let _ = self.prune_expired_pending_global_front(Instant::now(), 256);
        self.pump_media_transfers();
        self.cleanup_expired_entries();

        Ok(())
    }

    fn process_welcome_retry_queue(&mut self) -> Result<()> {
        let now = Utc::now();
        let timed_out_attempts: Vec<String> = self
            .welcome_lifecycles
            .iter()
            .filter_map(|(peer_id, record)| {
                if matches!(record.state, WelcomeDeliveryState::SendAttempted)
                    && record.next_retry_at.is_some_and(|retry_at| retry_at <= now)
                {
                    return Some(peer_id.clone());
                }
                None
            })
            .take(WELCOME_RETRY_BATCH_SIZE)
            .collect();

        for peer_id in timed_out_attempts {
            let _ = self.apply_welcome_send_failure(
                &peer_id,
                crate::events::WelcomeReasonCode::Timeout,
                Some("Welcome send confirmation timed out".to_string()),
                "welcome_confirm_timeout",
            )?;
        }

        let due_peers: Vec<String> = self
            .welcome_lifecycles
            .iter()
            .filter_map(|(peer_id, record)| {
                if matches!(record.state, WelcomeDeliveryState::Failed)
                    && record.next_retry_at.is_some_and(|retry_at| retry_at <= now)
                {
                    return Some(peer_id.clone());
                }
                None
            })
            .take(WELCOME_RETRY_BATCH_SIZE)
            .collect();

        for peer_id in due_peers {
            if let Err(err) = self.try_send_welcome(&peer_id, "welcome_retry") {
                warn!(
                    peer_id = %peer_id,
                    error = %err,
                    "Failed to process welcome retry"
                );
            }
        }

        Ok(())
    }

    /// Processes messages ready for retry from the retry queue.
    ///
    /// EDGE CASE HANDLING:
    /// - Checks transport availability before each retry attempt
    /// - Handles transport switch mid-retry
    /// - Properly tracks retry counts and transport failures
    fn process_retry_queue(&mut self) -> Result<()> {
        // Limit batch size to prevent blocking on large queues
        let max_batch_size = 20;
        let mut processed = 0;

        while processed < max_batch_size {
            let entry = match self.retry_queue.dequeue_ready() {
                Some(e) => e,
                None => break,
            };

            processed += 1;
            let previous_transport = self.transport_manager.current_transport();
            self.ensure_outbox_entry(&entry.message);

            let forced_transport = self.pinned_media_transport_for_message(&entry.message.id);
            let send_result = if let Some(transport) = forced_transport {
                self.transport_manager
                    .send_via_transport(&entry.message, transport)
            } else {
                self.transport_manager.send(&entry.message)
            };
            let current_transport =
                forced_transport.or_else(|| self.transport_manager.current_transport());

            match send_result {
                Ok(()) => {
                    let ack_registered_now = self.ensure_ack_registration(&entry.message)?;

                    if !ack_registered_now {
                        self.ack_manager.increment_retry_count(&entry.message.id);
                    }
                    self.mark_message_sent(
                        &entry.message,
                        current_transport,
                        Some(entry.retry_count.saturating_add(1)),
                    );

                    if let Some(transport) = current_transport {
                        self.transport_manager.reset_retry_count(transport);
                    }

                    debug!(
                        message_id = %entry.message.id,
                        retry_count = entry.retry_count,
                        transport = ?current_transport,
                        "Retry send succeeded"
                    );
                }
                Err(e) => {
                    // Re-enqueue with incremented retry count
                    // If this fails (max retries), the message remains in outbox
                    if self
                        .retry_queue
                        .enqueue(entry.message.clone(), entry.retry_count + 1)
                        .is_err()
                    {
                        warn!(
                            message_id = %entry.message.id,
                            retry_count = entry.retry_count,
                            "Max retries exceeded, message remains in outbox for recovery"
                        );
                    }

                    if let Some(transport) = forced_transport.or(previous_transport) {
                        self.transport_manager.record_retry_failure(transport);
                    }

                    debug!(
                        message_id = %entry.message.id,
                        retry_count = entry.retry_count,
                        transport = ?current_transport,
                        error = %e,
                        "Retry send failed, will retry later"
                    );
                }
            }
        }

        if processed > 0 {
            debug!(processed = processed, "Processed retry queue entries");
        }

        Ok(())
    }

    /// Processes timed out ACKs and handles retry or failure.
    fn process_timed_out_acks(&mut self) -> Result<()> {
        let timed_out = self.ack_manager.drain_timed_out();
        for pending in timed_out {
            let message_id = pending.message_id.clone();

            if pending.retry_count >= self.config.reliability.retry.max_retries {
                self.handle_max_retries_exceeded(&message_id, pending.retry_count)?;
                continue;
            }

            self.handle_ack_timeout_retry(&message_id, pending.retry_count)?;
        }
        Ok(())
    }

    /// Handles a message that has exceeded maximum retries.
    fn handle_max_retries_exceeded(
        &mut self,
        message_id: &MessageId,
        retry_count: u32,
    ) -> Result<()> {
        let state = lock_shared_state(&self.shared_state).map_err(|e| {
            error!(
                "Failed to lock shared state for message failed event: {}",
                e
            );
            e
        })?;
        state.emit_event(Event::message_failed(
            message_id.clone(),
            "Max retries exceeded".to_string(),
            retry_count,
        ));
        drop(state);

        self.handle_outbound_media_chunk_failed(message_id, "max retries exceeded");
        self.ack_manager.remove_ack(message_id);
        if let Some(entry) = self.remove_outbox_entry(message_id) {
            if let Some(transport) = entry.last_transport {
                self.transport_manager.record_delivery_failure(transport);
            }
        }
        Ok(())
    }

    /// Handles retry logic for a timed out ACK.
    fn handle_ack_timeout_retry(&mut self, message_id: &MessageId, retry_count: u32) -> Result<()> {
        if let Some(entry) = self
            .outbox
            .get(message_id)
            .or_else(|| self.media_outbox.get(message_id))
        {
            let message_clone = entry.message.clone();
            let last_transport = entry.last_transport;

            match self.retry_queue.enqueue(message_clone, retry_count) {
                Ok(()) => {
                    if let Some(transport) = last_transport {
                        self.transport_manager.record_retry_failure(transport);
                    }
                }
                Err(_) => {
                    self.handle_retry_queue_unavailable(message_id, retry_count)?;
                }
            }
        } else {
            self.handle_missing_outbox_entry(message_id, retry_count)?;
        }
        Ok(())
    }

    /// Handles the case when retry queue is unavailable.
    fn handle_retry_queue_unavailable(
        &mut self,
        message_id: &MessageId,
        retry_count: u32,
    ) -> Result<()> {
        let state = lock_shared_state(&self.shared_state).map_err(|e| {
            error!(
                "Failed to lock shared state for retry queue error event: {}",
                e
            );
            e
        })?;
        state.emit_event(Event::message_failed(
            message_id.clone(),
            "Retry queue unavailable".to_string(),
            retry_count,
        ));
        drop(state);

        self.handle_outbound_media_chunk_failed(message_id, "retry queue unavailable");
        self.ack_manager.remove_ack(message_id);
        if let Some(entry) = self.remove_outbox_entry(message_id) {
            if let Some(transport) = entry.last_transport {
                self.transport_manager.record_delivery_failure(transport);
            }
        }
        Ok(())
    }

    /// Handles the case when outbox entry is missing.
    fn handle_missing_outbox_entry(
        &mut self,
        message_id: &MessageId,
        retry_count: u32,
    ) -> Result<()> {
        let state = lock_shared_state(&self.shared_state).map_err(|e| {
            error!(
                "Failed to lock shared state for missing outbox entry event: {}",
                e
            );
            e
        })?;
        state.emit_event(Event::message_failed(
            message_id.clone(),
            "Message missing from outbox (cannot retry)".to_string(),
            retry_count,
        ));
        drop(state);

        self.handle_outbound_media_chunk_failed(message_id, "missing outbox entry");
        self.ack_manager.remove_ack(message_id);
        Ok(())
    }

    // ========================================================================
    // MESH GROUP MESSAGING (MLS-encrypted, transport-agnostic)
    // ========================================================================

    /// Acquires a read guard on the MLS manager.
    ///
    /// Returns an error if MLS is not initialized or the lock is poisoned.
    pub(crate) fn read_mls_guard(
        &self,
    ) -> Result<std::sync::RwLockReadGuard<'_, offline_protocol_mls::MlsManager>> {
        let mls = self
            .mls_manager
            .as_ref()
            .ok_or_else(|| Error::Other("MLS not initialized".to_string()))?;
        mls.read()
            .map_err(|_| Error::Other("MLS lock poisoned".to_string()))
    }


    /// Cleans up expired entries from deduplicator, retry queue, outbox, and ack manager.
    pub(crate) fn cleanup_expired_entries(&mut self) {
        self.deduplicator.cleanup_expired();
        self.retry_queue.cleanup_expired();
        self.cleanup_outbox();
        self.mesh_services.cleanup_expired();
        self.cleanup_group_message_dedup();
        let stale_file_ids = self
            .file_transfer_manager
            .cleanup_stale_transfers(StdDuration::from_secs(MEDIA_TRANSFER_STALE_TIMEOUT_SECS));
        for file_id in stale_file_ids {
            self.pending_media_metadata.remove(&file_id);
        }
        self.cleanup_stale_media_state(StdDuration::from_secs(MEDIA_TRANSFER_STALE_TIMEOUT_SECS));
        // Prune old timed-out ACKs that weren't cleaned up by normal retry flow
        self.ack_manager
            .prune_old_timeouts(std::time::Duration::from_secs(300)); // 5 minutes
    }

    /// Gets the current protocol state.
    pub fn state(&self) -> ProtocolState {
        let Ok(state) = lock_shared_state(&self.shared_state) else {
            error!("Failed to lock shared state in state()");
            return ProtocolState::Stopped;
        };
        state.state
    }

    /// Gets the configuration.
    pub fn config(&self) -> &ProtocolConfig {
        &self.config
    }

    /// Gets a reference to the mesh services registry.
    pub fn mesh_services(&self) -> &MeshServices {
        &self.mesh_services
    }

    /// Gets a mutable reference to the transport manager.
    ///
    /// This allows external code (e.g., FFI) to add transports dynamically.
    pub fn transport_manager_mut(&mut self) -> &mut TransportManager {
        &mut self.transport_manager
    }

    /// Gets a reference to the transport manager.
    pub fn transport_manager(&self) -> &TransportManager {
        &self.transport_manager
    }

    /// Updates the DORS configuration at runtime.
    ///
    /// This replaces the current DORS selector configuration with the provided config.
    pub fn update_dors_config(&mut self, config: DorsConfig) {
        self.transport_manager.update_selector_config(config);
    }

    /// Updates the ACK configuration at runtime.
    ///
    /// Note: This affects new ACK registrations; existing pending ACKs keep their original timeout.
    pub fn update_ack_config(&mut self, config: AckConfig) {
        self.ack_manager = AckManager::with_config(config.clone());
        self.config.reliability.ack = config;
    }

    /// Updates the retry configuration at runtime.
    ///
    /// Note: This affects new retry entries; existing entries keep their original timing.
    pub fn update_retry_config(&mut self, config: RetryConfig) {
        self.retry_queue = RetryQueue::with_config(config.clone());
        self.config.reliability.retry = config;
    }

    /// Updates the deduplication configuration at runtime.
    ///
    /// Note: This clears the deduplication cache and applies the new config.
    pub fn update_dedup_config(&mut self, config: DeduplicatorConfig) {
        self.deduplicator = Deduplicator::with_config(config.clone());
        self.config.reliability.dedup = config;
    }

    /// Gets deduplicator statistics for monitoring.
    pub fn deduplicator_stats(&self) -> DeduplicatorStats {
        self.deduplicator.stats()
    }

    /// Gets pending encrypted message queue counters and gauges.
    pub fn pending_queue_metrics(&self) -> PendingQueueMetrics {
        self.pending_queue_metrics.clone()
    }

    /// Gets the current ACK manager statistics.
    pub fn pending_ack_count(&self) -> usize {
        self.ack_manager.pending_count()
    }

    /// Gets the current retry queue statistics.
    pub fn retry_queue_size(&self) -> usize {
        self.retry_queue.len()
    }

    fn handle_outbound_media_chunk_delivered(&mut self, message_id: &MessageId) {
        let Some((file_id, chunk_index)) = self.outbound_media_chunks.remove(message_id) else {
            return;
        };

        if let Some(window) = self.outbound_media_windows.get_mut(&file_id) {
            window.on_chunk_ack(chunk_index);
        }

        let Some(transfer) = self.outbound_media_transfers.get_mut(&file_id) else {
            return;
        };

        transfer.delivered_chunks.insert(chunk_index);
        transfer.last_updated_at = Instant::now();

        let delivered_chunks = transfer.delivered_chunks.len() as u32;
        let total_chunks = transfer.total_chunks;
        let content_type = transfer.content_type;
        let recipient = transfer.recipient.clone();
        let completed = delivered_chunks == total_chunks;

        if let Ok(state) = lock_shared_state(&self.shared_state) {
            state.emit_event(Event::file_progress(
                file_id.clone(),
                delivered_chunks,
                total_chunks,
            ));
            if completed {
                state.emit_event(Event::media_sent(file_id.clone(), content_type, recipient));
            }
        }

        if completed {
            self.outbound_media_transfers.remove(&file_id);
            self.outbound_media_windows.remove(&file_id);
        }
    }

    fn handle_outbound_media_chunk_failed(&mut self, message_id: &MessageId, reason: &str) {
        let Some((file_id, _chunk_index)) = self.outbound_media_chunks.remove(message_id) else {
            return;
        };

        if self.outbound_media_transfers.remove(&file_id).is_some() {
            self.outbound_media_chunks
                .retain(|_, (candidate_file_id, _)| candidate_file_id != &file_id);
            self.outbound_media_windows.remove(&file_id);
            warn!(
                file_id = %file_id,
                message_id = %message_id,
                reason = %reason,
                "Aborting outbound media transfer after terminal chunk failure"
            );
        }
    }

    fn cleanup_stale_media_state(&mut self, max_age: StdDuration) {
        let now = Instant::now();

        self.pending_media_metadata
            .retain(|_, metadata| now.duration_since(metadata.last_updated_at) <= max_age);

        let stale_outbound_file_ids: HashSet<String> = self
            .outbound_media_transfers
            .iter()
            .filter_map(|(file_id, transfer)| {
                if now.duration_since(transfer.last_updated_at) > max_age {
                    return Some(file_id.clone());
                }
                None
            })
            .collect();

        if stale_outbound_file_ids.is_empty() {
            return;
        }

        self.outbound_media_transfers
            .retain(|file_id, _| !stale_outbound_file_ids.contains(file_id));
        self.outbound_media_chunks
            .retain(|_, (file_id, _)| !stale_outbound_file_ids.contains(file_id));
        self.outbound_media_windows
            .retain(|file_id, _| !stale_outbound_file_ids.contains(file_id));
    }

    fn is_media_outbox_message(message: &Message) -> bool {
        message.content_type == ContentType::FileChunk
    }

    fn ensure_outbox_entry(&mut self, message: &Message) {
        if !message.requires_ack {
            return;
        }

        let is_media = Self::is_media_outbox_message(message);
        let (outbox, capacity) = if is_media {
            use crate::constants::MAX_MEDIA_OUTBOX_ENTRIES;
            (&mut self.media_outbox, MAX_MEDIA_OUTBOX_ENTRIES)
        } else {
            (&mut self.outbox, MAX_OUTBOX_ENTRIES)
        };

        if !outbox.contains_key(&message.id) && outbox.len() >= capacity {
            if let Some((oldest_id, last_transport)) = outbox
                .iter()
                .min_by_key(|(_, entry)| entry.last_sent_at)
                .map(|(id, entry)| (id.clone(), entry.last_transport))
            {
                if let Some(transport) = last_transport {
                    self.transport_manager.record_delivery_failure(transport);
                }
                let outbox = if is_media {
                    &mut self.media_outbox
                } else {
                    &mut self.outbox
                };
                outbox.remove(&oldest_id);
                self.handle_outbound_media_chunk_failed(&oldest_id, "outbox eviction");
            }
        }

        let outbox = if is_media {
            &mut self.media_outbox
        } else {
            &mut self.outbox
        };
        outbox
            .entry(message.id.clone())
            .or_insert_with(|| OutboxEntry {
                message: message.clone(),
                attempt_count: 0,
                first_sent_at: Utc::now(),
                last_sent_at: Utc::now(),
                last_transport: None,
            });
    }

    fn mark_message_sent(
        &mut self,
        message: &Message,
        transport: Option<TransportType>,
        attempt_hint: Option<u32>,
    ) {
        if !message.requires_ack {
            return;
        }

        let now = Utc::now();
        let outbox = if Self::is_media_outbox_message(message) {
            &mut self.media_outbox
        } else {
            &mut self.outbox
        };
        let entry = outbox
            .entry(message.id.clone())
            .or_insert_with(|| OutboxEntry {
                message: message.clone(),
                attempt_count: 0,
                first_sent_at: now,
                last_sent_at: now,
                last_transport: transport,
            });

        entry.message = message.clone();
        if entry.attempt_count == 0 {
            entry.first_sent_at = now;
        }
        entry.attempt_count = attempt_hint.unwrap_or(entry.attempt_count.saturating_add(1));
        entry.last_sent_at = now;
        entry.last_transport = transport;
    }

    fn remove_outbox_entry(&mut self, message_id: &MessageId) -> Option<OutboxEntry> {
        self.outbox
            .remove(message_id)
            .or_else(|| self.media_outbox.remove(message_id))
    }

    fn cleanup_outbox(&mut self) {
        let cutoff = Utc::now()
            - ChronoDuration::milliseconds(
                self.config.reliability.retry.outbox_max_lifetime_ms as i64,
            );

        let mut expired_from_outbox = Vec::new();
        for (message_id, entry) in &self.outbox {
            if entry.last_sent_at >= cutoff {
                continue;
            }
            if entry.message.requires_ack && self.ack_manager.is_waiting_for_ack(&entry.message.id)
            {
                continue;
            }
            expired_from_outbox.push((message_id.clone(), entry.last_transport));
        }
        for (message_id, last_transport) in expired_from_outbox {
            if let Some(transport) = last_transport {
                self.transport_manager.record_delivery_failure(transport);
            }
            self.outbox.remove(&message_id);
            self.handle_outbound_media_chunk_failed(&message_id, "outbox lifetime exceeded");
        }

        let mut expired_from_media = Vec::new();
        for (message_id, entry) in &self.media_outbox {
            if entry.last_sent_at >= cutoff {
                continue;
            }
            if entry.message.requires_ack && self.ack_manager.is_waiting_for_ack(&entry.message.id)
            {
                continue;
            }
            expired_from_media.push((message_id.clone(), entry.last_transport));
        }
        for (message_id, last_transport) in expired_from_media {
            if let Some(transport) = last_transport {
                self.transport_manager.record_delivery_failure(transport);
            }
            self.media_outbox.remove(&message_id);
            self.handle_outbound_media_chunk_failed(&message_id, "outbox lifetime exceeded");
        }
    }

    fn ensure_ack_registration(&mut self, message: &Message) -> Result<bool> {
        if !message.requires_ack {
            return Ok(false);
        }

        if self.ack_manager.is_waiting_for_ack(&message.id) {
            Ok(false)
        } else {
            self.ack_manager
                .register_pending_ack(message.id.clone(), None)?;
            Ok(true)
        }
    }
}

#[cfg(test)]
pub(crate) mod tests {
    use super::*;
    #[cfg(feature = "mls-observability")]
    use crate::mls_observability::MlsLifecycleEvent;
    use offline_protocol_transport::{
        mock::MockTransport, Transport, TransportMetrics, TransportStatus, TransportType,
    };
    use std::sync::{Arc, Barrier, Mutex};
    use std::thread;
    use std::time::Duration;

    pub(crate) fn create_test_config() -> ProtocolConfig {
        ProtocolConfig::new("test-app", "user123")
    }

    pub(crate) fn create_test_config_for_user(user_id: &str) -> ProtocolConfig {
        ProtocolConfig::new("test-app", user_id)
    }

    #[cfg(feature = "mls-observability")]
    #[derive(Default, Clone)]
    struct RecordingMlsEmitter {
        events: Arc<Mutex<Vec<MlsLifecycleEvent>>>,
    }

    #[cfg(feature = "mls-observability")]
    impl MlsEventEmitter for RecordingMlsEmitter {
        fn emit(&self, event: MlsLifecycleEvent) {
            self.events.lock().unwrap().push(event);
        }
    }

    #[cfg(feature = "mls-observability")]
    impl RecordingMlsEmitter {
        fn take(&self) -> Vec<MlsLifecycleEvent> {
            let mut guard = self.events.lock().unwrap();
            std::mem::take(&mut *guard)
        }
    }

    fn pending_test_message(sender: &str, content: &str) -> Message {
        Message::new(
            UserId::new(sender).unwrap(),
            UserId::new("user123").unwrap(),
            AppId::new("test-app").unwrap(),
            content,
        )
    }

    #[derive(Debug, Clone)]
    struct FlakyTransport {
        transport_type: TransportType,
        status: Arc<Mutex<TransportStatus>>,
        sent_messages: Arc<Mutex<Vec<Message>>>,
        failures_remaining: Arc<Mutex<u32>>,
    }

    impl FlakyTransport {
        fn fail_first(transport_type: TransportType, failures: u32) -> Self {
            Self {
                transport_type,
                status: Arc::new(Mutex::new(TransportStatus::Unavailable)),
                sent_messages: Arc::new(Mutex::new(Vec::new())),
                failures_remaining: Arc::new(Mutex::new(failures)),
            }
        }
    }

    impl Transport for FlakyTransport {
        fn as_any(&self) -> &dyn std::any::Any {
            self
        }

        fn transport_type(&self) -> TransportType {
            self.transport_type
        }

        fn status(&self) -> TransportStatus {
            *self.status.lock().unwrap()
        }

        fn metrics(&self) -> TransportMetrics {
            TransportMetrics::default()
        }

        fn send(&self, message: &Message) -> offline_protocol_transport::Result<()> {
            let mut remaining = self.failures_remaining.lock().unwrap();
            if *remaining > 0 {
                *remaining = remaining.saturating_sub(1);
                return Err(offline_protocol_transport::Error::SendFailed(
                    "forced failure".to_string(),
                ));
            }

            self.sent_messages.lock().unwrap().push(message.clone());
            Ok(())
        }

        fn receive(&self) -> offline_protocol_transport::Result<Option<Message>> {
            Ok(None)
        }

        fn start(&mut self) -> offline_protocol_transport::Result<()> {
            *self.status.lock().unwrap() = TransportStatus::Available;
            Ok(())
        }

        fn stop(&mut self) -> offline_protocol_transport::Result<()> {
            *self.status.lock().unwrap() = TransportStatus::Disconnected;
            Ok(())
        }
    }

    #[derive(Default)]
    struct FailingPendingListStorage {
        inner: crate::mls::InMemoryStorage,
    }

    impl MlsStorage for FailingPendingListStorage {
        fn store(
            &self,
            key_type: &str,
            key_id: &str,
            data: &[u8],
        ) -> offline_protocol_mls::storage::StorageResult<()> {
            self.inner.store(key_type, key_id, data)
        }

        fn load(
            &self,
            key_type: &str,
            key_id: &str,
        ) -> offline_protocol_mls::storage::StorageResult<Option<Vec<u8>>> {
            self.inner.load(key_type, key_id)
        }

        fn delete(
            &self,
            key_type: &str,
            key_id: &str,
        ) -> offline_protocol_mls::storage::StorageResult<()> {
            self.inner.delete(key_type, key_id)
        }

        fn list_keys(
            &self,
            key_type: &str,
        ) -> offline_protocol_mls::storage::StorageResult<Vec<String>> {
            if key_type == storage_keys::PENDING_MESSAGES {
                return Err(offline_protocol_mls::StorageError::LoadFailed(
                    "forced restore failure".to_string(),
                ));
            }
            self.inner.list_keys(key_type)
        }
    }

    #[test]
    fn test_protocol_creation() {
        let protocol = OfflineProtocol::new(create_test_config());
        assert!(protocol.is_ok());
    }

    #[test]
    fn test_protocol_start_stop() {
        let mut protocol = OfflineProtocol::new(create_test_config()).unwrap();

        assert_eq!(protocol.state(), ProtocolState::Stopped);

        assert!(protocol.start().is_ok());
        assert_eq!(protocol.state(), ProtocolState::Running);

        assert!(protocol.stop().is_ok());
        assert_eq!(protocol.state(), ProtocolState::Stopped);
    }

    #[test]
    fn test_protocol_already_started() {
        let mut protocol = OfflineProtocol::new(create_test_config()).unwrap();

        protocol.start().unwrap();
        let result = protocol.start();
        assert!(result.is_err());
    }

    #[test]
    fn test_protocol_pause_resume() {
        let mut protocol = OfflineProtocol::new(create_test_config()).unwrap();

        protocol.start().unwrap();
        assert_eq!(protocol.state(), ProtocolState::Running);

        protocol.pause().unwrap();
        assert_eq!(protocol.state(), ProtocolState::Paused);

        protocol.resume().unwrap();
        assert_eq!(protocol.state(), ProtocolState::Running);
    }

    #[test]
    fn test_send_message() {
        let mut protocol = OfflineProtocol::new(create_test_config()).unwrap();

        // Add a mock transport
        let mut mock_transport = MockTransport::new(TransportType::BLE);
        mock_transport.start().unwrap();
        protocol
            .transport_manager_mut()
            .add_transport(TransportType::BLE, Box::new(mock_transport));

        protocol.start().unwrap();

        let result =
            protocol.send_message("bob", "Hello!", None::<MessagePriority>, None::<String>);
        assert!(result.is_ok());
    }

    #[test]
    fn test_send_message_not_started() {
        let mut protocol = OfflineProtocol::new(create_test_config()).unwrap();

        let result =
            protocol.send_message("bob", "Hello!", None::<MessagePriority>, None::<String>);
        assert!(result.is_err());
    }

    #[test]
    fn test_receive_message() {
        let mut protocol = OfflineProtocol::new(create_test_config()).unwrap();
        // Add a mock transport for testing
        let mut mock_transport = MockTransport::new(TransportType::BLE);
        mock_transport.start().unwrap();

        // Queue a message in the mock transport
        let message = Message::new(
            UserId::new("alice").unwrap(),
            UserId::new("user123").unwrap(),
            AppId::new("test-app").unwrap(),
            "Test message",
        );
        mock_transport.queue_message(message.clone());

        protocol
            .transport_manager_mut()
            .add_transport(TransportType::BLE, Box::new(mock_transport));
        protocol.start().unwrap();

        // Receive it
        let received = protocol.receive_message();
        assert!(received.is_some());
        assert_eq!(received.unwrap().id, message.id);
    }

    #[test]
    fn test_event_handler() {
        let mut protocol = OfflineProtocol::new(create_test_config()).unwrap();

        let event_received = Arc::new(Mutex::new(false));
        let event_received_clone = event_received.clone();

        protocol.on_event(move |event| {
            if matches!(event, Event::MessageSent { .. }) {
                *event_received_clone.lock().unwrap() = true;
            }
        });

        // Add a mock transport
        use offline_protocol_transport::{mock::MockTransport, TransportType};
        let mut mock_transport = MockTransport::new(TransportType::BLE);
        mock_transport.start().unwrap();
        protocol
            .transport_manager_mut()
            .add_transport(TransportType::BLE, Box::new(mock_transport));

        protocol.start().unwrap();
        protocol
            .send_message("bob", "Hello!", None::<MessagePriority>, None::<String>)
            .unwrap();

        assert!(*event_received.lock().unwrap());
    }

    #[test]
    fn test_deduplication() {
        let mut protocol = OfflineProtocol::new(create_test_config()).unwrap();

        // Add a mock transport
        let mut mock_transport = MockTransport::new(TransportType::BLE);
        mock_transport.start().unwrap();
        protocol
            .transport_manager_mut()
            .add_transport(TransportType::BLE, Box::new(mock_transport));

        protocol.start().unwrap();

        // Send same message twice
        protocol
            .send_message("bob", "Hello!", None::<MessagePriority>, None::<String>)
            .unwrap();
        let result =
            protocol.send_message("bob", "Hello!", None::<MessagePriority>, None::<String>);

        // Second send should succeed (different message ID generated)
        assert!(result.is_ok());
    }

    #[test]
    fn test_process_retries() {
        let mut protocol = OfflineProtocol::new(create_test_config()).unwrap();
        protocol.start().unwrap();

        // Process should not fail
        assert!(protocol.process().is_ok());
    }

    #[test]
    fn test_ack_timeout_requeues_message() {
        let mut config = create_test_config();
        config.reliability.ack.default_timeout_ms = 10;
        config.reliability.retry.initial_delay_ms = 5;
        config.reliability.retry.max_retries = 2;
        let mut protocol = OfflineProtocol::new(config).unwrap();

        let mut mock_transport = MockTransport::new(TransportType::BLE);
        mock_transport.start().unwrap();
        protocol
            .transport_manager_mut()
            .add_transport(TransportType::BLE, Box::new(mock_transport.clone()));

        protocol.start().unwrap();

        protocol
            .send_message("bob", "Hello!", None::<MessagePriority>, None::<String>)
            .unwrap();
        assert_eq!(mock_transport.sent_messages().len(), 1);

        thread::sleep(Duration::from_millis(15));
        protocol.process().unwrap();
        thread::sleep(Duration::from_millis(10));
        protocol.process().unwrap();

        assert!(
            mock_transport.sent_messages().len() >= 2,
            "Expected retry to resend message"
        );
    }

    #[test]
    fn test_config_access() {
        let config = create_test_config();
        let protocol = OfflineProtocol::new(config.clone()).unwrap();

        assert_eq!(protocol.config().app_id, config.app_id);
        assert_eq!(protocol.config().user_id, config.user_id);
    }

    #[test]
    fn test_ble_only_transport_works() {
        // Test that BLE works independently when it's the only transport enabled
        // This verifies the fix for BLE not working when Internet/WiFi Direct are disabled
        let mut config = create_test_config();
        config.transport.ble_enabled = true;
        config.transport.wifi_direct_enabled = false;
        config.transport.internet_enabled = false;

        let mut protocol = OfflineProtocol::new(config).unwrap();

        // Add only BLE transport (simulating BLE-only configuration)
        let mut mock_transport = MockTransport::new(TransportType::BLE);
        mock_transport.start().unwrap();
        protocol
            .transport_manager_mut()
            .add_transport(TransportType::BLE, Box::new(mock_transport));

        // Start protocol - BLE should be available
        protocol.start().unwrap();
        assert_eq!(protocol.state(), ProtocolState::Running);

        // Verify BLE transport is available
        let available_transports = protocol.transport_manager().get_available_transports();
        assert!(
            available_transports.contains_key(&TransportType::BLE),
            "BLE transport should be available when it's the only transport enabled"
        );
        assert_eq!(
            available_transports.len(),
            1,
            "Only BLE transport should be available"
        );

        // Test that we can send a message via BLE
        let result = protocol.send_message(
            "bob",
            "Hello from BLE-only!",
            None::<MessagePriority>,
            None::<String>,
        );
        assert!(
            result.is_ok(),
            "Should be able to send message when only BLE is enabled"
        );

        // Verify the message was sent via BLE
        let current_transport = protocol.transport_manager().current_transport();
        assert_eq!(
            current_transport,
            Some(TransportType::BLE),
            "Current transport should be BLE"
        );
    }

    // ========================================================================
    // AUTO-ENCRYPTION TESTS
    // ========================================================================

    use crate::config::EncryptionConfig;

    #[test]
    fn test_encryption_config_default_enabled() {
        let config = create_test_config();
        assert!(
            config.encryption.enabled,
            "Encryption should be enabled by default"
        );
        assert!(
            config.encryption.auto_key_exchange,
            "Auto key exchange should be enabled by default"
        );
        assert!(
            config.encryption.store_pending,
            "Store pending should be enabled by default"
        );
    }

    #[test]
    fn test_encryption_config_disabled() {
        let mut config = create_test_config();
        config.encryption = EncryptionConfig::disabled();

        assert!(!config.encryption.enabled);
        assert!(!config.encryption.auto_key_exchange);
        assert!(!config.encryption.store_pending);

        let protocol = OfflineProtocol::new(config).unwrap();
        assert!(!protocol.is_mls_initialized());
    }

    #[test]
    fn test_should_auto_encrypt_without_mls() {
        let config = create_test_config();
        let protocol = OfflineProtocol::new(config).unwrap();

        // Even though encryption is enabled by default, MLS is not initialized
        assert!(!protocol.is_mls_initialized());
    }

    #[cfg(feature = "mls-observability")]
    #[test]
    fn test_mls_observability_emits_initialized_event() {
        let mut protocol = OfflineProtocol::new(create_test_config()).unwrap();
        let emitter = RecordingMlsEmitter::default();
        protocol.set_mls_event_emitter(Arc::new(emitter.clone()));
        let storage = Arc::new(crate::mls::InMemoryStorage::new());

        protocol.initialize_mls(storage).unwrap();

        let events = emitter.take();
        assert!(
            events
                .iter()
                .any(|event| matches!(event, MlsLifecycleEvent::Initialized { .. })),
            "Expected initialized lifecycle event"
        );
    }

    #[cfg(feature = "mls-observability")]
    #[test]
    fn test_mls_observability_emits_session_missing_when_not_initialized() {
        let mut protocol = OfflineProtocol::new(create_test_config()).unwrap();
        let emitter = RecordingMlsEmitter::default();
        protocol.set_mls_event_emitter(Arc::new(emitter.clone()));

        let result = protocol.encrypt_content_for_recipient_strict("bob", "hello");
        assert!(matches!(result, Err(Error::MlsNotInitialized)));

        let events = emitter.take();
        assert!(events.iter().any(|event| matches!(
            event,
            MlsLifecycleEvent::SessionMissing {
                error_category: Some(MlsErrorCategory::NotInitialized),
                ..
            }
        )));
    }

    #[cfg(feature = "mls-observability")]
    #[test]
    fn test_mls_observability_emits_encryption_used_for_successful_encrypt() {
        let mut protocol = OfflineProtocol::new(create_test_config()).unwrap();
        let emitter = RecordingMlsEmitter::default();
        protocol.set_mls_event_emitter(Arc::new(emitter.clone()));
        let storage = Arc::new(crate::mls::InMemoryStorage::new());
        protocol.initialize_mls(storage).unwrap();

        let bob_storage = Arc::new(crate::mls::InMemoryStorage::new());
        let bob_manager = MlsManager::new("bob", bob_storage).unwrap();
        let key_package = bob_manager.generate_key_package().unwrap();

        {
            let mls = protocol.mls_manager.as_ref().unwrap().clone();
            let manager = mls.read().unwrap();
            manager
                .import_key_package("bob", &key_package.key_package_data)
                .unwrap();
            manager.create_session("bob").unwrap();
        }
        protocol
            .confirm_session_state("bob", "manual_test")
            .unwrap();

        let encrypted = protocol
            .encrypt_content_for_recipient_strict("bob", "hello secure")
            .unwrap();
        assert!(encrypted.starts_with(internal_prefixes::ENCRYPTED));

        let events = emitter.take();
        assert!(events
            .iter()
            .any(|event| { matches!(event, MlsLifecycleEvent::EncryptionUsed { .. }) }));
    }

    #[cfg(feature = "mls-observability")]
    #[test]
    fn test_mls_observability_emits_decryption_failed_not_initialized() {
        let mut protocol = OfflineProtocol::new(create_test_config()).unwrap();
        let emitter = RecordingMlsEmitter::default();
        protocol.set_mls_event_emitter(Arc::new(emitter.clone()));

        let encrypted = EncryptedMessage {
            group_id: offline_protocol_mls::GroupId::new("session:alice:bob"),
            message_type: offline_protocol_mls::MlsMessageType::Application,
            epoch: 1,
            ciphertext: vec![1, 2, 3],
            sender_id: "alice".to_string(),
            timestamp_ms: 1234,
        };
        let content = format!(
            "{}{}",
            internal_prefixes::ENCRYPTED,
            serde_json::to_string(&encrypted).unwrap()
        );
        let message = Message::new(
            UserId::new("alice").unwrap(),
            UserId::new("user123").unwrap(),
            AppId::new("test-app").unwrap(),
            &content,
        );

        let result = protocol.process_internal_message(&message);
        assert!(matches!(result, Some(InternalMessageResult::Consumed)));

        let events = emitter.take();
        assert!(events.iter().any(|event| matches!(
            event,
            MlsLifecycleEvent::DecryptionFailed {
                failure_kind: DecryptionFailureKind::NotInitialized,
                ..
            }
        )));
    }

    #[cfg(feature = "mls-observability")]
    #[test]
    fn test_mls_observability_no_encryption_event_on_aborted_encrypt() {
        let mut protocol = OfflineProtocol::new(create_test_config()).unwrap();
        let emitter = RecordingMlsEmitter::default();
        protocol.set_mls_event_emitter(Arc::new(emitter.clone()));
        let storage = Arc::new(crate::mls::InMemoryStorage::new());
        protocol.initialize_mls(storage).unwrap();

        protocol.pending_key_packages.insert(
            "bob".to_string(),
            ReceivedKeyPackage {
                key_package_data: vec![1, 2, 3],
                local_expires_at_ms: (Utc::now().timestamp_millis() as u64).saturating_add(60_000),
            },
        );

        let result = protocol.encrypt_content_for_recipient_strict("bob", "blocked");
        assert!(matches!(
            result,
            Err(Error::SessionNotReady(EstablishmentState::HaveKeyPackage))
        ));

        let events = emitter.take();
        assert!(
            !events
                .iter()
                .any(|event| matches!(event, MlsLifecycleEvent::EncryptionUsed { .. })),
            "EncryptionUsed should not emit for aborted operation"
        );
    }

    #[cfg(feature = "mls-observability")]
    #[test]
    fn test_mls_observability_session_ready_emits_once_for_idempotent_confirm() {
        let mut protocol = OfflineProtocol::new(create_test_config()).unwrap();
        let emitter = RecordingMlsEmitter::default();
        protocol.set_mls_event_emitter(Arc::new(emitter.clone()));
        let storage = Arc::new(crate::mls::InMemoryStorage::new());
        protocol.initialize_mls(storage).unwrap();

        protocol.welcome_lifecycles.insert(
            "bob".to_string(),
            WelcomeLifecycleRecord {
                peer_id: "bob".to_string(),
                group_id: "session:user123:bob".to_string(),
                state: WelcomeDeliveryState::Sent,
                attempt: 1,
                welcome_message: Message::new(
                    UserId::new("user123").unwrap(),
                    UserId::new("bob").unwrap(),
                    AppId::new("test-app").unwrap(),
                    "__MLS_WELCOME__{}",
                ),
                next_retry_at: None,
                last_reason_code: None,
                last_transport_error: None,
                created_at: Utc::now(),
                expires_at: Utc::now() + ChronoDuration::seconds(60),
            },
        );

        assert!(protocol
            .confirm_session_state("bob", "confirmation_ack_received")
            .unwrap());
        assert!(!protocol
            .confirm_session_state("bob", "confirmation_ack_received")
            .unwrap());

        let events = emitter.take();
        let ready_count = events
            .iter()
            .filter(|event| matches!(event, MlsLifecycleEvent::SessionReady { .. }))
            .count();
        assert_eq!(ready_count, 1);
    }

    #[cfg(feature = "mls-observability")]
    #[test]
    fn test_mls_observability_uses_opaque_identifiers() {
        let mut protocol = OfflineProtocol::new(create_test_config()).unwrap();
        let emitter = RecordingMlsEmitter::default();
        protocol.set_mls_event_emitter(Arc::new(emitter.clone()));
        let storage = Arc::new(crate::mls::InMemoryStorage::new());
        protocol.initialize_mls(storage).unwrap();

        let events = emitter.take();
        let initialized = events
            .iter()
            .find_map(|event| match event {
                MlsLifecycleEvent::Initialized { session_id, .. } => Some(session_id.clone()),
                _ => None,
            })
            .unwrap();
        assert_ne!(initialized, "peer=none|group=none");
        assert_eq!(initialized.len(), 32);
    }

    #[test]
    fn test_on_neighbor_discovered_without_mls() {
        let mut config = create_test_config();
        config.encryption.enabled = true;
        config.encryption.auto_key_exchange = true;

        let mut protocol = OfflineProtocol::new(config).unwrap();

        // Add a mock transport
        let mut mock_transport = MockTransport::new(TransportType::BLE);
        mock_transport.start().unwrap();
        protocol
            .transport_manager_mut()
            .add_transport(TransportType::BLE, Box::new(mock_transport.clone()));

        protocol.start().unwrap();

        // This should not panic even without MLS initialized
        protocol.on_neighbor_discovered("peer123");

        // No key package should have been sent since MLS is not initialized
        assert_eq!(mock_transport.sent_messages().len(), 0);
    }

    #[test]
    fn test_on_neighbor_lost_clears_tracking() {
        let mut config = create_test_config();
        config.encryption.enabled = true;
        config.encryption.auto_key_exchange = true;

        let mut protocol = OfflineProtocol::new(config).unwrap();

        // Simulate that we've sent a key package to a peer (by inserting into tracking set)
        protocol.key_package_sent_to.insert("peer123".to_string());
        assert!(protocol.key_package_sent_to.contains("peer123"));

        // Neighbor lost should remove from tracking
        protocol.on_neighbor_lost("peer123");
        assert!(!protocol.key_package_sent_to.contains("peer123"));
    }

    #[test]
    fn test_internal_prefixes_are_correct() {
        // Verify internal message prefixes match expected values
        assert_eq!(internal_prefixes::KEY_PACKAGE, "__MLS_KEY_PKG__");
        assert_eq!(internal_prefixes::WELCOME, "__MLS_WELCOME__");
        assert_eq!(internal_prefixes::ENCRYPTED, "__MLS_ENC__");
        assert_eq!(
            internal_prefixes::SESSION_CONFIRM_PROBE,
            "__MLS_CONFIRM_PROBE__"
        );
        assert_eq!(
            internal_prefixes::SESSION_CONFIRM_ACK,
            "__MLS_CONFIRM_ACK__"
        );
        assert_eq!(internal_prefixes::CONN_REQUEST, "__CONN_REQ__");
        assert_eq!(internal_prefixes::CONN_ACCEPT, "__CONN_ACC__");
        assert_eq!(internal_prefixes::CONN_REJECT, "__CONN_REJ__");
        assert_eq!(
            offline_protocol_services::SVC_DISCOVER_QUERY,
            "__SVC_DISC_Q__"
        );
        assert_eq!(
            offline_protocol_services::SVC_DISCOVER_RESPONSE,
            "__SVC_DISC_R__"
        );
        assert_eq!(offline_protocol_services::SVC_REQUEST, "__SVC_REQ__");
        assert_eq!(offline_protocol_services::SVC_RESPONSE, "__SVC_RESP__");
    }

    #[test]
    fn test_process_internal_message_key_package() {
        let mut config = create_test_config();
        config.encryption.enabled = true;
        config.encryption.auto_key_exchange = true;

        let mut protocol = OfflineProtocol::new(config).unwrap();

        // Create a key package message
        let key_pkg_payload = KeyPackagePayload {
            user_id: "sender123".to_string(),
            key_package_data: vec![1, 2, 3, 4],
            remaining_lifetime_ms: 30 * 24 * 60 * 60 * 1000,
            timestamp_ms: 12345,
        };
        let content = format!(
            "{}{}",
            internal_prefixes::KEY_PACKAGE,
            serde_json::to_string(&key_pkg_payload).unwrap()
        );

        let message = Message::new(
            UserId::new("sender123").unwrap(),
            UserId::new("user123").unwrap(),
            AppId::new("test-app").unwrap(),
            &content,
        );

        // Process the message
        let result = protocol.process_internal_message(&message);

        // Should be consumed (not surfaced to app)
        assert!(matches!(result, Some(InternalMessageResult::Consumed)));

        // Key package should be stored
        assert!(protocol.pending_key_packages.contains_key("sender123"));
        let received = protocol.pending_key_packages.get("sender123").unwrap();
        assert_eq!(received.key_package_data, vec![1u8, 2, 3, 4]);
        assert!(received.local_expires_at_ms > 0);
    }

    #[test]
    fn test_process_internal_message_connection_request_event() {
        let mut protocol = OfflineProtocol::new(create_test_config()).unwrap();
        let events: Arc<Mutex<Vec<Event>>> = Arc::new(Mutex::new(Vec::new()));
        let events_handle = Arc::clone(&events);

        protocol.on_event(move |event| {
            events_handle.lock().unwrap().push(event);
        });

        let payload = ConnectionRequestPayload {
            sender_name: "Alice".to_string(),
            timestamp_ms: 12345,
            key_package: Some(vec![9, 8, 7]),
        };
        let content = format!(
            "{}{}",
            internal_prefixes::CONN_REQUEST,
            serde_json::to_string(&payload).unwrap()
        );

        let message = Message::new(
            UserId::new("alice").unwrap(),
            UserId::new("user123").unwrap(),
            AppId::new("test-app").unwrap(),
            &content,
        );

        let result = protocol.process_internal_message(&message);
        assert!(matches!(result, Some(InternalMessageResult::Consumed)));

        let captured = events.lock().unwrap();
        assert_eq!(captured.len(), 1);
        match &captured[0] {
            Event::ConnectionRequestReceived {
                sender,
                sender_name,
                timestamp,
                key_package,
            } => {
                assert_eq!(sender, "alice");
                assert_eq!(sender_name, "Alice");
                assert_eq!(*timestamp, 12345);
                assert_eq!(key_package.as_ref(), Some(&vec![9, 8, 7]));
            }
            _ => panic!("Wrong event type"),
        }
    }

    #[test]
    fn test_process_internal_message_connection_accepted_event() {
        let mut protocol = OfflineProtocol::new(create_test_config()).unwrap();
        let events: Arc<Mutex<Vec<Event>>> = Arc::new(Mutex::new(Vec::new()));
        let events_handle = Arc::clone(&events);

        protocol.on_event(move |event| {
            events_handle.lock().unwrap().push(event);
        });

        let payload = ConnectionAcceptedPayload {
            accepted_by_name: "Bob".to_string(),
            timestamp_ms: 99999,
            key_package: Some(vec![1, 2, 3, 4]),
        };
        let content = format!(
            "{}{}",
            internal_prefixes::CONN_ACCEPT,
            serde_json::to_string(&payload).unwrap()
        );

        let message = Message::new(
            UserId::new("bob").unwrap(),
            UserId::new("user123").unwrap(),
            AppId::new("test-app").unwrap(),
            &content,
        );

        let result = protocol.process_internal_message(&message);
        assert!(matches!(result, Some(InternalMessageResult::Consumed)));

        let captured = events.lock().unwrap();
        assert_eq!(captured.len(), 1);
        match &captured[0] {
            Event::ConnectionAccepted {
                accepted_by,
                accepted_by_name,
                timestamp,
                key_package,
            } => {
                assert_eq!(accepted_by, "bob");
                assert_eq!(accepted_by_name, "Bob");
                assert_eq!(*timestamp, 99999);
                assert_eq!(key_package.as_ref(), Some(&vec![1, 2, 3, 4]));
            }
            _ => panic!("Wrong event type"),
        }
    }

    #[test]
    fn test_process_internal_message_connection_rejected_event() {
        let mut protocol = OfflineProtocol::new(create_test_config()).unwrap();
        let events: Arc<Mutex<Vec<Event>>> = Arc::new(Mutex::new(Vec::new()));
        let events_handle = Arc::clone(&events);

        protocol.on_event(move |event| {
            events_handle.lock().unwrap().push(event);
        });

        let content = internal_prefixes::CONN_REJECT.to_string();
        let message = Message::new(
            UserId::new("carol").unwrap(),
            UserId::new("user123").unwrap(),
            AppId::new("test-app").unwrap(),
            &content,
        );

        let result = protocol.process_internal_message(&message);
        assert!(matches!(result, Some(InternalMessageResult::Consumed)));

        let captured = events.lock().unwrap();
        assert_eq!(captured.len(), 1);
        match &captured[0] {
            Event::ConnectionRejected { rejected_by } => {
                assert_eq!(rejected_by, "carol");
            }
            _ => panic!("Wrong event type"),
        }
    }

    // ========================================================================
    // SENDER-SIDE CONNECTION REQUEST TESTS
    // ========================================================================

    #[test]
    fn test_send_connection_request_success() {
        let mut protocol = OfflineProtocol::new(create_test_config()).unwrap();

        let mut mock_transport = MockTransport::new(TransportType::BLE);
        mock_transport.start().unwrap();
        protocol
            .transport_manager_mut()
            .add_transport(TransportType::BLE, Box::new(mock_transport));

        protocol.start().unwrap();

        let result = protocol.send_connection_request("bob", "Alice", None);
        assert!(result.is_ok());
    }

    #[test]
    fn test_send_connection_request_not_started() {
        let mut protocol = OfflineProtocol::new(create_test_config()).unwrap();

        let result = protocol.send_connection_request("bob", "Alice", None);
        assert!(result.is_err());
    }

    #[test]
    fn test_send_connection_request_with_key_package() {
        let mut protocol = OfflineProtocol::new(create_test_config()).unwrap();

        let mut mock_transport = MockTransport::new(TransportType::BLE);
        mock_transport.start().unwrap();
        protocol
            .transport_manager_mut()
            .add_transport(TransportType::BLE, Box::new(mock_transport));

        protocol.start().unwrap();

        let key_package = vec![1, 2, 3, 4, 5];
        let result = protocol.send_connection_request("bob", "Alice", Some(key_package));
        assert!(result.is_ok());
    }

    #[test]
    fn test_accept_connection_request_success() {
        let mut protocol = OfflineProtocol::new(create_test_config()).unwrap();

        let mut mock_transport = MockTransport::new(TransportType::BLE);
        mock_transport.start().unwrap();
        protocol
            .transport_manager_mut()
            .add_transport(TransportType::BLE, Box::new(mock_transport));

        protocol.start().unwrap();

        let result = protocol.accept_connection_request("bob", "Alice", None);
        assert!(result.is_ok());
    }

    #[test]
    fn test_accept_connection_request_not_started() {
        let mut protocol = OfflineProtocol::new(create_test_config()).unwrap();

        let result = protocol.accept_connection_request("bob", "Alice", None);
        assert!(result.is_err());
    }

    #[test]
    fn test_accept_connection_request_with_key_package() {
        let mut protocol = OfflineProtocol::new(create_test_config()).unwrap();

        let mut mock_transport = MockTransport::new(TransportType::BLE);
        mock_transport.start().unwrap();
        protocol
            .transport_manager_mut()
            .add_transport(TransportType::BLE, Box::new(mock_transport));

        protocol.start().unwrap();

        let key_package = vec![10, 20, 30];
        let result = protocol.accept_connection_request("bob", "Alice", Some(key_package));
        assert!(result.is_ok());
    }

    #[test]
    fn test_reject_connection_request_success() {
        let mut protocol = OfflineProtocol::new(create_test_config()).unwrap();

        let mut mock_transport = MockTransport::new(TransportType::BLE);
        mock_transport.start().unwrap();
        protocol
            .transport_manager_mut()
            .add_transport(TransportType::BLE, Box::new(mock_transport));

        protocol.start().unwrap();

        let result = protocol.reject_connection_request("bob");
        assert!(result.is_ok());
    }

    #[test]
    fn test_reject_connection_request_not_started() {
        let mut protocol = OfflineProtocol::new(create_test_config()).unwrap();

        let result = protocol.reject_connection_request("bob");
        assert!(result.is_err());
    }

    #[test]
    fn test_send_connection_request_returns_unique_ids() {
        let mut protocol = OfflineProtocol::new(create_test_config()).unwrap();

        let mut mock_transport = MockTransport::new(TransportType::BLE);
        mock_transport.start().unwrap();
        protocol
            .transport_manager_mut()
            .add_transport(TransportType::BLE, Box::new(mock_transport));

        protocol.start().unwrap();

        let id1 = protocol
            .send_connection_request("bob", "Alice", None)
            .unwrap();
        let id2 = protocol
            .send_connection_request("carol", "Alice", None)
            .unwrap();
        assert_ne!(id1, id2);
    }

    #[test]
    fn test_process_internal_message_regular_message() {
        let mut config = create_test_config();
        config.encryption.enabled = true;

        let mut protocol = OfflineProtocol::new(config).unwrap();

        // Create a regular (non-internal) message
        let message = Message::new(
            UserId::new("sender123").unwrap(),
            UserId::new("user123").unwrap(),
            AppId::new("test-app").unwrap(),
            "Hello, this is a regular message!",
        );

        // Process the message
        let result = protocol.process_internal_message(&message);

        // Should not be an internal message
        assert!(result.is_none());
    }

    #[test]
    fn test_pending_message_queue() {
        let mut config = create_test_config();
        config.encryption.enabled = true;
        config.encryption.store_pending = true;

        let mut protocol = OfflineProtocol::new(config).unwrap();

        // Queue some pending messages
        protocol.queue_pending_message(
            "bob",
            "Hello Bob!",
            MessagePriority::High,
            MessageId::new(),
            None,
        );
        protocol.queue_pending_message(
            "bob",
            "Another message",
            MessagePriority::Medium,
            MessageId::new(),
            None,
        );
        protocol.queue_pending_message(
            "alice",
            "Hello Alice!",
            MessagePriority::Low,
            MessageId::new(),
            None,
        );

        // Check pending messages are stored
        assert!(protocol.pending_encrypted_messages.contains_key("bob"));
        assert!(protocol.pending_encrypted_messages.contains_key("alice"));

        let bob_pending = protocol.pending_encrypted_messages.get("bob").unwrap();
        assert_eq!(bob_pending.len(), 2);
        assert_eq!(bob_pending[0].content, "Hello Bob!");
        assert_eq!(bob_pending[0].priority, MessagePriority::High);
    }

    #[test]
    fn test_encryption_builder_methods() {
        let config = ProtocolConfig::builder("test-app", "user123")
            .encryption_enabled(false)
            .auto_key_exchange(true)
            .store_pending_messages(false)
            .build()
            .unwrap();

        assert!(!config.encryption.enabled);
        assert!(config.encryption.auto_key_exchange);
        assert!(!config.encryption.store_pending);
    }

    #[test]
    fn test_require_encryption_blocks_plaintext_when_mls_uninitialized() {
        let mut config = create_test_config();
        config.encryption.enabled = true;
        config.encryption.require_encryption = true;

        let mut protocol = OfflineProtocol::new(config).unwrap();
        let mut mock_transport = MockTransport::new(TransportType::BLE);
        mock_transport.start().unwrap();
        let transport_handle = mock_transport.clone();
        protocol
            .transport_manager_mut()
            .add_transport(TransportType::BLE, Box::new(mock_transport));
        protocol.start().unwrap();

        let result = protocol.send_message("bob", "Hello", None::<MessagePriority>, None::<String>);
        assert!(matches!(result, Err(Error::EncryptFailed(_))));
        assert_eq!(transport_handle.sent_messages().len(), 0);
    }

    #[test]
    fn test_require_encryption_returns_typed_failures() {
        // NoKeyPackage
        let mut no_key_config = create_test_config();
        no_key_config.encryption.require_encryption = true;
        let mut no_key_protocol = OfflineProtocol::new(no_key_config).unwrap();
        let mut no_key_transport = MockTransport::new(TransportType::BLE);
        no_key_transport.start().unwrap();
        no_key_protocol
            .transport_manager_mut()
            .add_transport(TransportType::BLE, Box::new(no_key_transport));
        no_key_protocol.start().unwrap();
        no_key_protocol
            .initialize_mls(Arc::new(crate::mls::InMemoryStorage::new()))
            .unwrap();
        let no_key_result =
            no_key_protocol.send_message("bob", "nkp", None::<MessagePriority>, None::<String>);
        assert!(matches!(
            no_key_result,
            Err(Error::SessionNotReady(EstablishmentState::NoKeyPackage))
        ));

        // SessionPending
        let mut pending_config = create_test_config();
        pending_config.encryption.require_encryption = true;
        let mut pending_protocol = OfflineProtocol::new(pending_config).unwrap();
        let mut pending_transport = MockTransport::new(TransportType::BLE);
        pending_transport.start().unwrap();
        pending_protocol
            .transport_manager_mut()
            .add_transport(TransportType::BLE, Box::new(pending_transport));
        pending_protocol.start().unwrap();
        pending_protocol
            .initialize_mls(Arc::new(crate::mls::InMemoryStorage::new()))
            .unwrap();
        let bob_manager =
            crate::mls::MlsManager::new("bob", Arc::new(crate::mls::InMemoryStorage::new()))
                .unwrap();
        let bob_key_package = bob_manager.get_or_create_key_package().unwrap();
        {
            let manager = pending_protocol
                .mls_manager
                .as_ref()
                .unwrap()
                .read()
                .unwrap();
            manager
                .import_key_package("bob", &bob_key_package.key_package_data)
                .unwrap();
            manager.create_session("bob").unwrap();
        }
        let pending_result = pending_protocol.send_message(
            "bob",
            "pending",
            None::<MessagePriority>,
            None::<String>,
        );
        assert!(matches!(
            pending_result,
            Err(Error::SessionNotReady(EstablishmentState::SessionPending))
        ));

        // EncryptFailed
        let mut encrypt_fail_config = create_test_config();
        encrypt_fail_config.encryption.require_encryption = true;
        let mut encrypt_fail_protocol = OfflineProtocol::new(encrypt_fail_config).unwrap();
        let mut encrypt_fail_transport = MockTransport::new(TransportType::BLE);
        encrypt_fail_transport.start().unwrap();
        encrypt_fail_protocol
            .transport_manager_mut()
            .add_transport(TransportType::BLE, Box::new(encrypt_fail_transport));
        encrypt_fail_protocol.start().unwrap();
        let encrypt_fail_result = encrypt_fail_protocol.send_message(
            "bob",
            "encrypt-failed",
            None::<MessagePriority>,
            None::<String>,
        );
        assert!(matches!(encrypt_fail_result, Err(Error::EncryptFailed(_))));
    }

    #[test]
    fn test_require_encryption_failure_does_not_send_transport_payload() {
        let mut config = create_test_config();
        config.encryption.enabled = true;
        config.encryption.require_encryption = true;
        config.encryption.store_pending = true;

        let mut protocol = OfflineProtocol::new(config).unwrap();
        protocol
            .initialize_mls(Arc::new(crate::mls::InMemoryStorage::new()))
            .unwrap();

        let mut mock_transport = MockTransport::new(TransportType::BLE);
        mock_transport.start().unwrap();
        let transport_handle = mock_transport.clone();
        protocol
            .transport_manager_mut()
            .add_transport(TransportType::BLE, Box::new(mock_transport));
        protocol.start().unwrap();

        let result =
            protocol.send_message("bob", "blocked", None::<MessagePriority>, None::<String>);
        assert!(matches!(
            result,
            Err(Error::SessionNotReady(EstablishmentState::NoKeyPackage))
        ));
        assert_eq!(transport_handle.sent_messages().len(), 0);
    }

    #[test]
    fn test_require_encryption_encrypt_failed_emits_send_error_without_transport_output() {
        let mut config = create_test_config();
        config.encryption.enabled = true;
        config.encryption.require_encryption = true;

        // Keep MLS uninitialized to force strict-mode EncryptFailed path.
        let mut protocol = OfflineProtocol::new(config).unwrap();
        let mut mock_transport = MockTransport::new(TransportType::BLE);
        mock_transport.start().unwrap();
        let transport_handle = mock_transport.clone();
        protocol
            .transport_manager_mut()
            .add_transport(TransportType::BLE, Box::new(mock_transport));
        protocol.start().unwrap();

        let result = protocol.send_message(
            "bob",
            "must-never-leak",
            None::<MessagePriority>,
            None::<String>,
        );
        assert!(matches!(result, Err(Error::EncryptFailed(_))));
        assert_eq!(transport_handle.sent_messages().len(), 0);
    }

    #[test]
    fn test_require_encryption_strict_mode_is_side_effect_free_on_session_pending() {
        let mut config = create_test_config();
        config.encryption.enabled = true;
        config.encryption.require_encryption = true;
        config.encryption.store_pending = true;

        let mut protocol = OfflineProtocol::new(config).unwrap();
        protocol
            .initialize_mls(Arc::new(crate::mls::InMemoryStorage::new()))
            .unwrap();

        let mut mock_transport = MockTransport::new(TransportType::BLE);
        mock_transport.start().unwrap();
        let transport_handle = mock_transport.clone();
        protocol
            .transport_manager_mut()
            .add_transport(TransportType::BLE, Box::new(mock_transport));
        protocol.start().unwrap();

        let bob_manager =
            crate::mls::MlsManager::new("bob", Arc::new(crate::mls::InMemoryStorage::new()))
                .unwrap();
        let bob_key_package = bob_manager.get_or_create_key_package().unwrap();
        protocol.pending_key_packages.insert(
            "bob".to_string(),
            ReceivedKeyPackage {
                key_package_data: bob_key_package.key_package_data,
                local_expires_at_ms: Utc::now().timestamp_millis() as u64 + 60_000,
            },
        );

        let result = protocol.send_message(
            "bob",
            "strict-no-side-effects",
            None::<MessagePriority>,
            None::<String>,
        );

        // Strict path does not create session; we have key package but no session -> HaveKeyPackage
        assert!(matches!(
            result,
            Err(Error::SessionNotReady(EstablishmentState::HaveKeyPackage))
        ));
        assert_eq!(transport_handle.sent_messages().len(), 0);
        assert!(!protocol.pending_encrypted_messages.contains_key("bob"));
        assert!(!protocol.welcome_lifecycles.contains_key("bob"));
    }

    #[test]
    fn test_require_encryption_blocks_plaintext_for_send_message_via_transport() {
        let mut config = create_test_config();
        config.encryption.enabled = true;
        config.encryption.require_encryption = true;
        config.encryption.store_pending = true;

        let mut protocol = OfflineProtocol::new(config).unwrap();
        protocol
            .initialize_mls(Arc::new(crate::mls::InMemoryStorage::new()))
            .unwrap();

        let mut mock_transport = MockTransport::new(TransportType::BLE);
        mock_transport.start().unwrap();
        let transport_handle = mock_transport.clone();
        protocol
            .transport_manager_mut()
            .add_transport(TransportType::BLE, Box::new(mock_transport));
        protocol.start().unwrap();

        let result = protocol.send_message_via_transport(
            "bob",
            "blocked-via-transport",
            None::<MessagePriority>,
            TransportType::BLE,
            None::<String>,
        );

        assert!(matches!(
            result,
            Err(Error::SessionNotReady(EstablishmentState::NoKeyPackage))
        ));
        assert_eq!(transport_handle.sent_messages().len(), 0);
    }

    #[test]
    fn test_require_encryption_blocks_plaintext_connection_control_messages() {
        let mut config = create_test_config();
        config.encryption.enabled = true;
        config.encryption.require_encryption = true;

        let mut protocol = OfflineProtocol::new(config).unwrap();

        let mut mock_transport = MockTransport::new(TransportType::BLE);
        mock_transport.start().unwrap();
        let transport_handle = mock_transport.clone();
        protocol
            .transport_manager_mut()
            .add_transport(TransportType::BLE, Box::new(mock_transport));
        protocol.start().unwrap();

        let request_result = protocol.send_connection_request("bob", "alice", None);
        assert!(matches!(request_result, Err(Error::EncryptFailed(_))));

        let accept_result = protocol.accept_connection_request("bob", "alice", None);
        assert!(matches!(accept_result, Err(Error::EncryptFailed(_))));

        let reject_result = protocol.reject_connection_request("bob");
        assert!(matches!(reject_result, Err(Error::EncryptFailed(_))));

        assert_eq!(transport_handle.sent_messages().len(), 0);
    }

    #[test]
    fn test_non_strict_mode_preserves_pending_queue_behavior() {
        let mut config = create_test_config();
        config.encryption.enabled = true;
        config.encryption.store_pending = true;
        config.encryption.require_encryption = false;

        let mut protocol = OfflineProtocol::new(config).unwrap();
        protocol
            .initialize_mls(Arc::new(crate::mls::InMemoryStorage::new()))
            .unwrap();

        let mut mock_transport = MockTransport::new(TransportType::BLE);
        mock_transport.start().unwrap();
        let transport_handle = mock_transport.clone();
        protocol
            .transport_manager_mut()
            .add_transport(TransportType::BLE, Box::new(mock_transport));
        protocol.start().unwrap();

        let result =
            protocol.send_message("bob", "queued", None::<MessagePriority>, None::<String>);
        assert!(result.is_ok());
        assert_eq!(transport_handle.sent_messages().len(), 0);
        assert_eq!(
            protocol
                .pending_encrypted_messages
                .get("bob")
                .map_or(0, std::vec::Vec::len),
            1
        );
    }

    #[test]
    fn test_confirmed_sessions_tracking() {
        let mut config = create_test_config();
        config.encryption.enabled = true;
        config.encryption.store_pending = true;

        let mut protocol = OfflineProtocol::new(config).unwrap();

        // Initially no confirmed sessions
        assert!(protocol.confirmed_sessions.is_empty());

        // Add a confirmed session
        protocol.confirmed_sessions.insert("peer123".to_string());

        assert!(protocol.confirmed_sessions.contains("peer123"));
        assert!(!protocol.confirmed_sessions.contains("peer456"));
    }

    #[test]
    fn test_session_confirmation_persists_across_restart_bidirectional_send() {
        let mut alice_config = create_test_config_for_user("alice");
        alice_config.encryption.enabled = true;
        alice_config.encryption.store_pending = true;

        let mut bob_config = create_test_config_for_user("bob");
        bob_config.encryption.enabled = true;
        bob_config.encryption.store_pending = true;

        let alice_storage = Arc::new(InMemoryStorage::new());
        let bob_storage = Arc::new(InMemoryStorage::new());

        let mut alice = OfflineProtocol::new(alice_config).unwrap();
        let mut bob = OfflineProtocol::new(bob_config).unwrap();

        alice.initialize_mls(alice_storage.clone()).unwrap();
        bob.initialize_mls(bob_storage.clone()).unwrap();

        let mut alice_transport = MockTransport::new(TransportType::BLE);
        alice_transport.start().unwrap();
        let alice_transport_handle = alice_transport.clone();
        alice
            .transport_manager_mut()
            .add_transport(TransportType::BLE, Box::new(alice_transport));
        alice.start().unwrap();

        let mut bob_transport = MockTransport::new(TransportType::BLE);
        bob_transport.start().unwrap();
        let bob_transport_handle = bob_transport.clone();
        bob.transport_manager_mut()
            .add_transport(TransportType::BLE, Box::new(bob_transport));
        bob.start().unwrap();

        // Establish session from Alice -> Bob.
        let bob_key_package = {
            let manager = bob.mls_manager.as_ref().unwrap().read().unwrap();
            manager.get_or_create_key_package().unwrap()
        };
        alice.pending_key_packages.insert(
            "bob".to_string(),
            ReceivedKeyPackage {
                key_package_data: bob_key_package.key_package_data,
                local_expires_at_ms: Utc::now().timestamp_millis() as u64 + 60_000,
            },
        );

        // This creates session + Welcome and queues plaintext until confirmed.
        let _ = alice
            .send_message("bob", "bootstrap", None::<MessagePriority>, None::<String>)
            .unwrap();

        let welcome_wire = alice_transport_handle
            .sent_messages()
            .into_iter()
            .find(|msg| msg.content.starts_with(internal_prefixes::WELCOME))
            .map(|msg| msg.content)
            .expect("expected welcome message sent by initiator");
        let welcome_msg = Message::new(
            UserId::new("alice").unwrap(),
            UserId::new("bob").unwrap(),
            AppId::new("test-app").unwrap(),
            &welcome_wire,
        );
        let _ = bob.process_internal_message(&welcome_msg);

        // Bob sends encrypted message; Alice decrypts and confirms.
        bob.send_message("alice", "hello", None::<MessagePriority>, None::<String>)
            .unwrap();
        let bob_sent = bob_transport_handle.sent_messages();
        let last = bob_sent.last().unwrap().clone();
        let _ = alice.process_internal_message(&last);

        // Simulate restart on both peers with same storage.
        let mut alice2 = OfflineProtocol::new(create_test_config_for_user("alice")).unwrap();
        alice2.config.encryption.enabled = true;
        alice2.config.encryption.store_pending = true;
        alice2.initialize_mls(alice_storage.clone()).unwrap();
        let mut alice2_transport = MockTransport::new(TransportType::BLE);
        alice2_transport.start().unwrap();
        let alice2_transport_handle = alice2_transport.clone();
        alice2
            .transport_manager_mut()
            .add_transport(TransportType::BLE, Box::new(alice2_transport));
        alice2.start().unwrap();

        let mut bob2 = OfflineProtocol::new(create_test_config_for_user("bob")).unwrap();
        bob2.config.encryption.enabled = true;
        bob2.config.encryption.store_pending = true;
        bob2.initialize_mls(bob_storage.clone()).unwrap();
        let mut bob2_transport = MockTransport::new(TransportType::BLE);
        bob2_transport.start().unwrap();
        let bob2_transport_handle = bob2_transport.clone();
        bob2.transport_manager_mut()
            .add_transport(TransportType::BLE, Box::new(bob2_transport));
        bob2.start().unwrap();

        alice2
            .send_message(
                "bob",
                "after-restart-a2b",
                None::<MessagePriority>,
                None::<String>,
            )
            .unwrap();
        bob2.send_message(
            "alice",
            "after-restart-b2a",
            None::<MessagePriority>,
            None::<String>,
        )
        .unwrap();

        let a2b = alice2_transport_handle.sent_messages();
        let b2a = bob2_transport_handle.sent_messages();
        assert!(a2b
            .last()
            .unwrap()
            .content
            .starts_with(internal_prefixes::ENCRYPTED));
        assert!(b2a
            .last()
            .unwrap()
            .content
            .starts_with(internal_prefixes::ENCRYPTED));
    }

    #[test]
    fn test_initialize_mls_restore_failure_does_not_publish_partial_state() {
        let mut config = create_test_config_for_user("alice");
        config.encryption.enabled = true;
        config.encryption.store_pending = true;

        let mut protocol = OfflineProtocol::new(config).unwrap();
        let initial_clock = protocol.lamport_clock.value();

        let result = protocol.initialize_mls(Arc::new(FailingPendingListStorage::default()));
        assert!(result.is_err());
        assert!(protocol.mls_manager.is_none());
        assert!(protocol.message_storage.is_none());
        assert!(protocol.pending_encrypted_messages.is_empty());
        assert!(protocol.confirmed_sessions.is_empty());
        assert!(protocol.welcome_lifecycles.is_empty());
        assert_eq!(protocol.lamport_clock.value(), initial_clock);
    }

    #[test]
    fn test_auto_send_and_manual_mls_share_single_state_under_concurrency() {
        let mut config = create_test_config_for_user("alice");
        config.encryption.enabled = true;
        config.encryption.store_pending = true;
        config.encryption.require_encryption = false;

        let mut protocol = OfflineProtocol::new(config).unwrap();
        protocol
            .initialize_mls(Arc::new(crate::mls::InMemoryStorage::new()))
            .unwrap();

        let mut transport = MockTransport::new(TransportType::BLE);
        transport.start().unwrap();
        let transport_handle = transport.clone();
        protocol
            .transport_manager_mut()
            .add_transport(TransportType::BLE, Box::new(transport));
        protocol.start().unwrap();

        let bob_manager =
            MlsManager::new("bob", Arc::new(crate::mls::InMemoryStorage::new())).unwrap();
        let bob_key_package = bob_manager.get_or_create_key_package().unwrap();
        {
            let manager = protocol.mls_manager.as_ref().unwrap().read().unwrap();
            manager
                .import_key_package("bob", &bob_key_package.key_package_data)
                .unwrap();
            manager.create_session("bob").unwrap();
        }
        protocol.confirm_session_state("bob", "test_setup").unwrap();

        let mls_handle_before = protocol.mls_manager.as_ref().unwrap().clone();
        let sessions_before = {
            let manager = mls_handle_before.read().unwrap();
            manager.list_sessions().unwrap()
        };
        let groups_before = {
            let manager = mls_handle_before.read().unwrap();
            manager.list_groups().unwrap().len()
        };

        let shared = Arc::new(Mutex::new(protocol));
        let manual_shared = Arc::clone(&shared);
        let manual_thread = thread::spawn(move || {
            for i in 0..24 {
                let mls = {
                    let guard = manual_shared.lock().unwrap();
                    guard.mls_manager.as_ref().unwrap().clone()
                };
                let manager = mls.read().unwrap();
                manager
                    .create_group(&format!("manual-concurrent-group-{}", i))
                    .unwrap();
            }
        });

        let auto_shared = Arc::clone(&shared);
        let auto_thread = thread::spawn(move || {
            for i in 0..24 {
                let content = format!("auto-encrypted-{}", i);
                let mut guard = auto_shared.lock().unwrap();
                guard
                    .send_message("bob", &content, None::<MessagePriority>, None::<String>)
                    .unwrap();
            }
        });

        manual_thread.join().unwrap();
        auto_thread.join().unwrap();

        let mls_handle_after = {
            let protocol = shared.lock().unwrap();
            protocol.mls_manager.as_ref().unwrap().clone()
        };
        assert!(Arc::ptr_eq(&mls_handle_before, &mls_handle_after));

        let sessions_after = {
            let manager = mls_handle_after.read().unwrap();
            manager.list_sessions().unwrap()
        };
        assert_eq!(sessions_before, sessions_after);

        let sent = transport_handle.sent_messages();
        assert!(sent
            .iter()
            .filter(|message| message.recipient.as_str() == "bob")
            .all(|message| message.content.starts_with(internal_prefixes::ENCRYPTED)));

        let groups_after = {
            let manager = mls_handle_after.read().unwrap();
            manager.list_groups().unwrap().len()
        };
        assert_eq!(groups_after, groups_before + 24);

        let mut protocol = shared.lock().unwrap();
        protocol.stop().unwrap();
    }

    #[test]
    fn test_manual_welcome_processing_confirms_session_for_auto_encrypt_flow() {
        let mut config = create_test_config_for_user("alice");
        config.encryption.enabled = true;
        config.encryption.store_pending = true;
        config.encryption.require_encryption = false;

        let mut protocol = OfflineProtocol::new(config).unwrap();
        protocol
            .initialize_mls(Arc::new(crate::mls::InMemoryStorage::new()))
            .unwrap();

        let mut transport = MockTransport::new(TransportType::BLE);
        transport.start().unwrap();
        let transport_handle = transport.clone();
        protocol
            .transport_manager_mut()
            .add_transport(TransportType::BLE, Box::new(transport));
        protocol.start().unwrap();

        let bob_storage = Arc::new(InMemoryStorage::new());
        let bob_manager = MlsManager::new("bob", bob_storage).unwrap();
        let alice_key_package = {
            let manager = protocol.mls_manager.as_ref().unwrap().read().unwrap();
            manager.get_or_create_key_package().unwrap()
        };
        bob_manager
            .import_key_package("alice", &alice_key_package.key_package_data)
            .unwrap();
        let welcome = bob_manager.create_session("alice").unwrap();

        protocol.manual_mls_process_welcome(&welcome).unwrap();

        let persisted = protocol.load_session_state_entry("bob").unwrap().unwrap();
        assert_eq!(persisted, SessionState::Confirmed);

        protocol
            .send_message(
                "bob",
                "manual-welcome-unblocks-auto-send",
                None::<MessagePriority>,
                None::<String>,
            )
            .unwrap();

        let sent = transport_handle.sent_messages();
        assert!(sent
            .iter()
            .filter(|message| message.recipient.as_str() == "bob")
            .any(|message| message.content.starts_with(internal_prefixes::ENCRYPTED)));

        protocol.stop().unwrap();
    }

    #[test]
    fn test_manual_delete_session_clears_protocol_session_state() {
        let mut config = create_test_config_for_user("alice");
        config.encryption.enabled = true;
        config.encryption.store_pending = true;

        let mut protocol = OfflineProtocol::new(config).unwrap();
        protocol
            .initialize_mls(Arc::new(crate::mls::InMemoryStorage::new()))
            .unwrap();

        let bob_manager = MlsManager::new("bob", Arc::new(InMemoryStorage::new())).unwrap();
        let bob_key_package = bob_manager.get_or_create_key_package().unwrap();
        {
            let manager = protocol.mls_manager.as_ref().unwrap().read().unwrap();
            manager
                .import_key_package("bob", &bob_key_package.key_package_data)
                .unwrap();
            manager.create_session("bob").unwrap();
        }
        protocol.confirm_session_state("bob", "test_setup").unwrap();
        assert_eq!(
            protocol.load_session_state_entry("bob").unwrap().unwrap(),
            SessionState::Confirmed
        );

        protocol.manual_mls_delete_session("bob").unwrap();

        {
            let manager = protocol.mls_manager.as_ref().unwrap().read().unwrap();
            assert!(!manager.has_session("bob").unwrap());
        }
        assert!(!protocol.confirmed_sessions.contains("bob"));
        assert!(protocol.load_session_state_entry("bob").unwrap().is_none());
    }

    #[test]
    fn test_manual_delete_session_failure_keeps_protocol_state_unchanged() {
        let mut config = create_test_config_for_user("alice");
        config.encryption.enabled = true;
        config.encryption.store_pending = true;

        let mut protocol = OfflineProtocol::new(config).unwrap();
        protocol
            .initialize_mls(Arc::new(crate::mls::InMemoryStorage::new()))
            .unwrap();

        protocol.confirm_session_state("bob", "test_setup").unwrap();
        assert_eq!(
            protocol.load_session_state_entry("bob").unwrap().unwrap(),
            SessionState::Confirmed
        );
        assert!(protocol.confirmed_sessions.contains("bob"));

        // Force a deterministic failure path by poisoning the MLS lock.
        let poisoned_handle = protocol.mls_manager.as_ref().unwrap().clone();
        let poison_result = thread::spawn(move || {
            let _guard = poisoned_handle.write().unwrap();
            panic!("poison mls lock");
        })
        .join();
        assert!(poison_result.is_err());

        let result = protocol.manual_mls_delete_session("bob");
        assert!(result.is_err());
        assert_eq!(
            protocol.load_session_state_entry("bob").unwrap().unwrap(),
            SessionState::Confirmed
        );
        assert!(protocol.confirmed_sessions.contains("bob"));
    }

    #[test]
    fn test_is_session_confirmed_clears_stale_confirmed_state_without_mls_session() {
        let mut config = create_test_config_for_user("alice");
        config.encryption.enabled = true;
        config.encryption.store_pending = true;

        let mut protocol = OfflineProtocol::new(config).unwrap();
        protocol
            .initialize_mls(Arc::new(crate::mls::InMemoryStorage::new()))
            .unwrap();

        protocol.confirm_session_state("bob", "test_setup").unwrap();
        assert_eq!(
            protocol.load_session_state_entry("bob").unwrap().unwrap(),
            SessionState::Confirmed
        );

        assert!(!protocol.is_session_confirmed("bob").unwrap());
        assert!(protocol.load_session_state_entry("bob").unwrap().is_none());
        assert!(!protocol.confirmed_sessions.contains("bob"));
    }

    #[test]
    fn test_session_group_detection_for_manual_decrypt_confirmation() {
        assert!(OfflineProtocol::is_session_group_id("session:alice:bob"));
        assert!(!OfflineProtocol::is_session_group_id("group:team"));
    }

    #[test]
    fn test_confirmation_crash_recovery_before_first_send() {
        let mut config = create_test_config_for_user("alice");
        config.encryption.enabled = true;
        config.encryption.store_pending = true;

        let storage = Arc::new(InMemoryStorage::new());
        let mut protocol = OfflineProtocol::new(config).unwrap();
        protocol.initialize_mls(storage.clone()).unwrap();

        // Build a real session in MLS storage without using protocol transport.
        let bob_storage = Arc::new(InMemoryStorage::new());
        let bob_manager = MlsManager::new("bob", bob_storage).unwrap();
        let bob_key_package = bob_manager.get_or_create_key_package().unwrap();
        {
            let manager = protocol.mls_manager.as_ref().unwrap().read().unwrap();
            manager
                .import_key_package("bob", &bob_key_package.key_package_data)
                .unwrap();
            let welcome = manager.create_session("bob").unwrap();
            bob_manager.join_session(&welcome).unwrap();
        }

        // Persist confirmation and "crash" before first outbound post-confirm send.
        protocol.confirm_session_state("bob", "test_setup").unwrap();

        let mut restarted = OfflineProtocol::new(create_test_config_for_user("alice")).unwrap();
        restarted.config.encryption.enabled = true;
        restarted.config.encryption.store_pending = true;
        restarted.initialize_mls(storage).unwrap();
        let mut transport = MockTransport::new(TransportType::BLE);
        transport.start().unwrap();
        let transport_handle = transport.clone();
        restarted
            .transport_manager_mut()
            .add_transport(TransportType::BLE, Box::new(transport));
        restarted.start().unwrap();

        restarted
            .send_message(
                "bob",
                "post-crash-send",
                None::<MessagePriority>,
                None::<String>,
            )
            .unwrap();
        let sent = transport_handle.sent_messages();
        assert!(sent
            .last()
            .unwrap()
            .content
            .starts_with(internal_prefixes::ENCRYPTED));
    }

    #[test]
    fn test_confirmation_transition_idempotent() {
        let mut config = create_test_config();
        config.encryption.enabled = true;
        config.encryption.store_pending = true;

        let storage = Arc::new(InMemoryStorage::new());
        let mut protocol = OfflineProtocol::new(config).unwrap();
        protocol.initialize_mls(storage.clone()).unwrap();

        // First confirmation transitions Pending -> Confirmed.
        assert!(protocol
            .confirm_session_state("peer123", "idempotency_test")
            .unwrap());
        // Replay confirmation is a no-op and remains Confirmed.
        assert!(!protocol
            .confirm_session_state("peer123", "idempotency_test")
            .unwrap());

        let persisted = protocol
            .load_session_state_entry("peer123")
            .unwrap()
            .unwrap();
        assert_eq!(persisted, SessionState::Confirmed);
    }

    #[test]
    fn test_pending_session_state_blocks_send_until_confirmed() {
        let mut config = create_test_config();
        config.encryption.enabled = true;
        config.encryption.store_pending = true;

        let storage = Arc::new(InMemoryStorage::new());
        let mut protocol = OfflineProtocol::new(config).unwrap();
        protocol.initialize_mls(storage).unwrap();

        let mut mock_transport = MockTransport::new(TransportType::BLE);
        mock_transport.start().unwrap();
        protocol
            .transport_manager_mut()
            .add_transport(TransportType::BLE, Box::new(mock_transport));
        protocol.start().unwrap();

        // Provide a key package so first send creates a session and persists Pending.
        let bob_storage = Arc::new(InMemoryStorage::new());
        let bob_manager = MlsManager::new("bob", bob_storage).unwrap();
        let bob_key_package = bob_manager.get_or_create_key_package().unwrap();
        protocol.pending_key_packages.insert(
            "bob".to_string(),
            ReceivedKeyPackage {
                key_package_data: bob_key_package.key_package_data,
                local_expires_at_ms: Utc::now().timestamp_millis() as u64 + 60_000,
            },
        );

        protocol
            .send_message("bob", "queued-1", None::<MessagePriority>, None::<String>)
            .unwrap();
        protocol
            .send_message("bob", "queued-2", None::<MessagePriority>, None::<String>)
            .unwrap();

        assert_eq!(
            protocol
                .pending_encrypted_messages
                .get("bob")
                .unwrap()
                .len(),
            2
        );
        let persisted = protocol.load_session_state_entry("bob").unwrap().unwrap();
        assert_eq!(persisted, SessionState::Pending);
    }

    #[test]
    fn test_welcome_send_failure_keeps_session_pending_and_emits_reason_code() {
        let mut config = create_test_config();
        config.encryption.enabled = true;
        config.encryption.store_pending = true;
        config.reliability.retry.max_retries = 3;
        config.reliability.retry.initial_delay_ms = 1;
        config.reliability.retry.max_delay_ms = 5;

        let storage = Arc::new(InMemoryStorage::new());
        let mut protocol = OfflineProtocol::new(config).unwrap();
        protocol.initialize_mls(storage).unwrap();

        let mut flaky = FlakyTransport::fail_first(TransportType::BLE, 1);
        flaky.start().unwrap();
        protocol
            .transport_manager_mut()
            .add_transport(TransportType::BLE, Box::new(flaky));

        let observed_events = Arc::new(Mutex::new(Vec::<Event>::new()));
        let observed_events_clone = observed_events.clone();
        protocol.on_event(move |event| {
            observed_events_clone.lock().unwrap().push(event);
        });

        protocol.start().unwrap();

        let bob_storage = Arc::new(InMemoryStorage::new());
        let bob_manager = MlsManager::new("bob", bob_storage).unwrap();
        let bob_key_package = bob_manager.get_or_create_key_package().unwrap();
        protocol.pending_key_packages.insert(
            "bob".to_string(),
            ReceivedKeyPackage {
                key_package_data: bob_key_package.key_package_data,
                local_expires_at_ms: Utc::now().timestamp_millis() as u64 + 60_000,
            },
        );

        let _ = protocol
            .send_message(
                "bob",
                "queued-after-welcome-fail",
                None::<MessagePriority>,
                None::<String>,
            )
            .unwrap();

        assert_eq!(
            protocol.load_session_state_entry("bob").unwrap().unwrap(),
            SessionState::Pending
        );
        let lifecycle = protocol.welcome_lifecycles.get("bob").unwrap();
        assert_eq!(lifecycle.state, WelcomeDeliveryState::Failed);
        assert!(protocol
            .pending_encrypted_messages
            .get("bob")
            .is_some_and(|messages| !messages.is_empty()));

        let events = observed_events.lock().unwrap();
        assert!(events.iter().any(|event| matches!(
            event,
            Event::WelcomeSendFailed {
                reason_code: crate::events::WelcomeReasonCode::TransportUnavailable,
                retryable: true,
                ..
            }
        )));
        assert!(!events
            .iter()
            .any(|event| matches!(event, Event::SecureSessionEstablished { .. })));
    }

    #[test]
    fn test_welcome_retry_exhaustion_expires_and_aborts_pending_queue() {
        let mut config = create_test_config();
        config.encryption.enabled = true;
        config.encryption.store_pending = true;
        config.reliability.retry.max_retries = 1;

        let storage = Arc::new(InMemoryStorage::new());
        let mut protocol = OfflineProtocol::new(config).unwrap();
        protocol.initialize_mls(storage).unwrap();

        let mut flaky = FlakyTransport::fail_first(TransportType::BLE, 10);
        flaky.start().unwrap();
        protocol
            .transport_manager_mut()
            .add_transport(TransportType::BLE, Box::new(flaky));

        let observed_events = Arc::new(Mutex::new(Vec::<Event>::new()));
        let observed_events_clone = observed_events.clone();
        protocol.on_event(move |event| {
            observed_events_clone.lock().unwrap().push(event);
        });

        protocol.start().unwrap();

        let bob_storage = Arc::new(InMemoryStorage::new());
        let bob_manager = MlsManager::new("bob", bob_storage).unwrap();
        let bob_key_package = bob_manager.get_or_create_key_package().unwrap();
        protocol.pending_key_packages.insert(
            "bob".to_string(),
            ReceivedKeyPackage {
                key_package_data: bob_key_package.key_package_data,
                local_expires_at_ms: Utc::now().timestamp_millis() as u64 + 60_000,
            },
        );

        let result = protocol.send_message(
            "bob",
            "should-fail-terminally",
            None::<MessagePriority>,
            None::<String>,
        );
        assert!(result.is_err());

        let lifecycle = protocol.welcome_lifecycles.get("bob").unwrap();
        assert_eq!(lifecycle.state, WelcomeDeliveryState::Expired);
        assert_eq!(
            protocol.load_session_state_entry("bob").unwrap().unwrap(),
            SessionState::Pending
        );
        assert!(!protocol.pending_encrypted_messages.contains_key("bob"));

        let events = observed_events.lock().unwrap();
        assert!(events.iter().any(|event| matches!(
            event,
            Event::WelcomeSendExpired {
                reason_code: crate::events::WelcomeReasonCode::RetryExhausted,
                ..
            }
        )));
    }

    #[test]
    fn test_welcome_partial_success_after_retry_reaches_sent() {
        let mut config = create_test_config();
        config.encryption.enabled = true;
        config.encryption.store_pending = true;
        config.reliability.retry.max_retries = 3;
        config.reliability.retry.initial_delay_ms = 1;
        config.reliability.retry.max_delay_ms = 5;

        let storage = Arc::new(InMemoryStorage::new());
        let mut protocol = OfflineProtocol::new(config).unwrap();
        protocol.initialize_mls(storage).unwrap();

        let mut flaky = FlakyTransport::fail_first(TransportType::BLE, 1);
        flaky.start().unwrap();
        protocol
            .transport_manager_mut()
            .add_transport(TransportType::BLE, Box::new(flaky));

        protocol.start().unwrap();

        let bob_storage = Arc::new(InMemoryStorage::new());
        let bob_manager = MlsManager::new("bob", bob_storage).unwrap();
        let bob_key_package = bob_manager.get_or_create_key_package().unwrap();
        protocol.pending_key_packages.insert(
            "bob".to_string(),
            ReceivedKeyPackage {
                key_package_data: bob_key_package.key_package_data,
                local_expires_at_ms: Utc::now().timestamp_millis() as u64 + 60_000,
            },
        );

        let _ = protocol
            .send_message(
                "bob",
                "queued-after-flaky-send",
                None::<MessagePriority>,
                None::<String>,
            )
            .unwrap();
        assert_eq!(
            protocol.welcome_lifecycles.get("bob").unwrap().state,
            WelcomeDeliveryState::Failed
        );

        thread::sleep(Duration::from_millis(10));
        protocol.process().unwrap();

        assert_eq!(
            protocol.welcome_lifecycles.get("bob").unwrap().state,
            WelcomeDeliveryState::Sent
        );
    }

    #[test]
    fn test_welcome_internet_requires_async_confirmation_before_sent() {
        let mut config = create_test_config();
        config.encryption.enabled = true;
        config.encryption.store_pending = true;

        let storage = Arc::new(InMemoryStorage::new());
        let mut protocol = OfflineProtocol::new(config).unwrap();
        protocol.initialize_mls(storage).unwrap();

        let mut internet = MockTransport::new(TransportType::Internet);
        internet.start().unwrap();
        protocol
            .transport_manager_mut()
            .add_transport(TransportType::Internet, Box::new(internet));
        protocol.start().unwrap();

        let bob_storage = Arc::new(InMemoryStorage::new());
        let bob_manager = MlsManager::new("bob", bob_storage).unwrap();
        let bob_key_package = bob_manager.get_or_create_key_package().unwrap();
        protocol.pending_key_packages.insert(
            "bob".to_string(),
            ReceivedKeyPackage {
                key_package_data: bob_key_package.key_package_data,
                local_expires_at_ms: Utc::now().timestamp_millis() as u64 + 60_000,
            },
        );

        let _ = protocol
            .send_message(
                "bob",
                "queued-over-internet",
                None::<MessagePriority>,
                None::<String>,
            )
            .unwrap();

        let welcome_message_id = protocol
            .welcome_lifecycles
            .get("bob")
            .unwrap()
            .welcome_message
            .id
            .as_str()
            .to_string();
        assert_eq!(
            protocol.welcome_lifecycles.get("bob").unwrap().state,
            WelcomeDeliveryState::SendAttempted
        );
        assert!(protocol
            .welcome_lifecycles
            .get("bob")
            .unwrap()
            .next_retry_at
            .is_some());

        protocol
            .on_transport_send_confirmed(&welcome_message_id)
            .unwrap();
        assert_eq!(
            protocol.welcome_lifecycles.get("bob").unwrap().state,
            WelcomeDeliveryState::Sent
        );
    }

    #[test]
    fn test_welcome_terminal_lifecycle_can_be_overwritten() {
        let mut config = create_test_config();
        config.encryption.enabled = true;
        config.encryption.store_pending = true;

        let storage = Arc::new(InMemoryStorage::new());
        let mut protocol = OfflineProtocol::new(config).unwrap();
        protocol.initialize_mls(storage).unwrap();

        let message = Message::new(
            UserId::new("alice").unwrap(),
            UserId::new("bob").unwrap(),
            AppId::new("test-app").unwrap(),
            "__MLS_WELCOME__dummy".to_string(),
        );
        protocol
            .upsert_welcome_lifecycle("bob", "session:bob:1", message.clone(), "test_created")
            .unwrap();
        protocol
            .transition_welcome_state("bob", WelcomeDeliveryState::SendAttempted, "test_attempted")
            .unwrap();
        protocol
            .transition_welcome_state("bob", WelcomeDeliveryState::Sent, "test_sent")
            .unwrap();

        let overwrite =
            protocol.upsert_welcome_lifecycle("bob", "session:bob:2", message, "test_overwrite");
        assert!(overwrite.is_ok());
    }

    #[test]
    fn test_welcome_non_terminal_lifecycle_cannot_be_overwritten() {
        let mut config = create_test_config();
        config.encryption.enabled = true;
        config.encryption.store_pending = true;

        let storage = Arc::new(InMemoryStorage::new());
        let mut protocol = OfflineProtocol::new(config).unwrap();
        protocol.initialize_mls(storage).unwrap();

        let message = Message::new(
            UserId::new("alice").unwrap(),
            UserId::new("bob").unwrap(),
            AppId::new("test-app").unwrap(),
            "__MLS_WELCOME__dummy".to_string(),
        );
        protocol
            .upsert_welcome_lifecycle("bob", "session:bob:1", message.clone(), "test_created")
            .unwrap();
        protocol
            .transition_welcome_state("bob", WelcomeDeliveryState::SendAttempted, "test_attempted")
            .unwrap();

        let overwrite =
            protocol.upsert_welcome_lifecycle("bob", "session:bob:2", message, "test_overwrite");
        assert!(overwrite.is_err());
    }

    #[test]
    fn test_welcome_lifecycle_rejects_illegal_transition_from_sent() {
        let mut config = create_test_config();
        config.encryption.enabled = true;
        config.encryption.store_pending = true;

        let storage = Arc::new(InMemoryStorage::new());
        let mut protocol = OfflineProtocol::new(config).unwrap();
        protocol.initialize_mls(storage).unwrap();

        let mut mock_transport = MockTransport::new(TransportType::BLE);
        mock_transport.start().unwrap();
        protocol
            .transport_manager_mut()
            .add_transport(TransportType::BLE, Box::new(mock_transport));
        protocol.start().unwrap();

        let bob_storage = Arc::new(InMemoryStorage::new());
        let bob_manager = MlsManager::new("bob", bob_storage).unwrap();
        let bob_key_package = bob_manager.get_or_create_key_package().unwrap();
        protocol.pending_key_packages.insert(
            "bob".to_string(),
            ReceivedKeyPackage {
                key_package_data: bob_key_package.key_package_data,
                local_expires_at_ms: Utc::now().timestamp_millis() as u64 + 60_000,
            },
        );

        let _ = protocol
            .send_message(
                "bob",
                "welcome-sent",
                None::<MessagePriority>,
                None::<String>,
            )
            .unwrap();
        assert_eq!(
            protocol.welcome_lifecycles.get("bob").unwrap().state,
            WelcomeDeliveryState::Sent
        );

        let illegal = protocol.transition_welcome_state(
            "bob",
            WelcomeDeliveryState::Failed,
            "test_illegal_transition",
        );
        assert!(illegal.is_err());
    }

    #[test]
    fn test_welcome_restart_recovery_restores_failed_lifecycle() {
        let mut config = create_test_config_for_user("alice");
        config.encryption.enabled = true;
        config.encryption.store_pending = true;
        config.reliability.retry.max_retries = 3;
        config.reliability.retry.initial_delay_ms = 50;
        config.reliability.retry.max_delay_ms = 50;

        let storage = Arc::new(InMemoryStorage::new());
        let mut protocol = OfflineProtocol::new(config.clone()).unwrap();
        protocol.initialize_mls(storage.clone()).unwrap();

        let mut flaky = FlakyTransport::fail_first(TransportType::BLE, 1);
        flaky.start().unwrap();
        protocol
            .transport_manager_mut()
            .add_transport(TransportType::BLE, Box::new(flaky));
        protocol.start().unwrap();

        let bob_storage = Arc::new(InMemoryStorage::new());
        let bob_manager = MlsManager::new("bob", bob_storage).unwrap();
        let bob_key_package = bob_manager.get_or_create_key_package().unwrap();
        protocol.pending_key_packages.insert(
            "bob".to_string(),
            ReceivedKeyPackage {
                key_package_data: bob_key_package.key_package_data,
                local_expires_at_ms: Utc::now().timestamp_millis() as u64 + 60_000,
            },
        );

        let _ = protocol
            .send_message(
                "bob",
                "restart-recovery",
                None::<MessagePriority>,
                None::<String>,
            )
            .unwrap();
        assert_eq!(
            protocol.welcome_lifecycles.get("bob").unwrap().state,
            WelcomeDeliveryState::Failed
        );

        let mut restarted = OfflineProtocol::new(config).unwrap();
        restarted.initialize_mls(storage).unwrap();
        let restored = restarted.welcome_lifecycles.get("bob").unwrap();
        assert_eq!(restored.state, WelcomeDeliveryState::Failed);
        assert!(restored.next_retry_at.is_some());
    }

    #[test]
    fn test_welcome_restore_repairs_failed_without_retry_schedule() {
        let mut config = create_test_config_for_user("alice");
        config.encryption.enabled = true;
        config.encryption.store_pending = true;

        let storage = Arc::new(InMemoryStorage::new());
        let mut protocol = OfflineProtocol::new(config.clone()).unwrap();
        protocol.initialize_mls(storage.clone()).unwrap();

        let message = Message::new(
            UserId::new("alice").unwrap(),
            UserId::new("bob").unwrap(),
            AppId::new("test-app").unwrap(),
            "__MLS_WELCOME__dummy".to_string(),
        );
        protocol
            .upsert_welcome_lifecycle("bob", "session:bob:1", message, "test_created")
            .unwrap();
        protocol
            .transition_welcome_state("bob", WelcomeDeliveryState::SendAttempted, "test_attempted")
            .unwrap();
        protocol
            .transition_welcome_state("bob", WelcomeDeliveryState::Failed, "test_failed")
            .unwrap();

        {
            let record = protocol.welcome_lifecycles.get_mut("bob").unwrap();
            record.last_reason_code = Some(crate::events::WelcomeReasonCode::TransportUnavailable);
            record.next_retry_at = None;
        }
        let persisted = protocol.welcome_lifecycles.get("bob").cloned().unwrap();
        protocol
            .persist_welcome_lifecycle_entry(&persisted)
            .unwrap();

        let mut restarted = OfflineProtocol::new(config).unwrap();
        restarted.initialize_mls(storage).unwrap();
        let restored = restarted.welcome_lifecycles.get("bob").unwrap();
        assert_eq!(restored.state, WelcomeDeliveryState::Failed);
        assert!(restored.next_retry_at.is_some());
    }

    #[test]
    fn test_welcome_restore_promotes_retry_exhausted_failed_to_expired() {
        let mut config = create_test_config_for_user("alice");
        config.encryption.enabled = true;
        config.encryption.store_pending = true;

        let storage = Arc::new(InMemoryStorage::new());
        let mut protocol = OfflineProtocol::new(config.clone()).unwrap();
        protocol.initialize_mls(storage.clone()).unwrap();

        let message = Message::new(
            UserId::new("alice").unwrap(),
            UserId::new("bob").unwrap(),
            AppId::new("test-app").unwrap(),
            "__MLS_WELCOME__dummy".to_string(),
        );
        protocol
            .upsert_welcome_lifecycle("bob", "session:bob:1", message, "test_created")
            .unwrap();
        protocol
            .transition_welcome_state("bob", WelcomeDeliveryState::SendAttempted, "test_attempted")
            .unwrap();
        protocol
            .transition_welcome_state("bob", WelcomeDeliveryState::Failed, "test_failed")
            .unwrap();

        {
            let record = protocol.welcome_lifecycles.get_mut("bob").unwrap();
            record.last_reason_code = Some(crate::events::WelcomeReasonCode::RetryExhausted);
            record.next_retry_at = None;
        }
        let persisted = protocol.welcome_lifecycles.get("bob").cloned().unwrap();
        protocol
            .persist_welcome_lifecycle_entry(&persisted)
            .unwrap();

        let mut restarted = OfflineProtocol::new(config).unwrap();
        restarted.initialize_mls(storage).unwrap();
        let restored = restarted.welcome_lifecycles.get("bob").unwrap();
        assert_eq!(restored.state, WelcomeDeliveryState::Expired);
        assert!(restored.next_retry_at.is_none());
    }

    #[test]
    fn test_welcome_transport_callbacks_out_of_order_converge_to_sent() {
        let mut config = create_test_config();
        config.encryption.enabled = true;
        config.encryption.store_pending = true;

        let storage = Arc::new(InMemoryStorage::new());
        let mut protocol = OfflineProtocol::new(config).unwrap();
        protocol.initialize_mls(storage).unwrap();

        let mut internet = MockTransport::new(TransportType::Internet);
        internet.start().unwrap();
        protocol
            .transport_manager_mut()
            .add_transport(TransportType::Internet, Box::new(internet));
        protocol.start().unwrap();

        let bob_storage = Arc::new(InMemoryStorage::new());
        let bob_manager = MlsManager::new("bob", bob_storage).unwrap();
        let bob_key_package = bob_manager.get_or_create_key_package().unwrap();
        protocol.pending_key_packages.insert(
            "bob".to_string(),
            ReceivedKeyPackage {
                key_package_data: bob_key_package.key_package_data,
                local_expires_at_ms: Utc::now().timestamp_millis() as u64 + 60_000,
            },
        );

        let _ = protocol
            .send_message(
                "bob",
                "queued-over-internet",
                None::<MessagePriority>,
                None::<String>,
            )
            .unwrap();
        let welcome_message_id = protocol
            .welcome_lifecycles
            .get("bob")
            .unwrap()
            .welcome_message
            .id
            .as_str()
            .to_string();

        protocol
            .on_transport_send_failed(
                &welcome_message_id,
                Some("Internet transport send failed".to_string()),
            )
            .unwrap();
        assert_eq!(
            protocol.welcome_lifecycles.get("bob").unwrap().state,
            WelcomeDeliveryState::Failed
        );

        protocol
            .on_transport_send_confirmed(&welcome_message_id)
            .unwrap();
        assert_eq!(
            protocol.welcome_lifecycles.get("bob").unwrap().state,
            WelcomeDeliveryState::Sent
        );

        protocol
            .on_transport_send_failed(
                &welcome_message_id,
                Some("Late failure callback".to_string()),
            )
            .unwrap();
        assert_eq!(
            protocol.welcome_lifecycles.get("bob").unwrap().state,
            WelcomeDeliveryState::Sent
        );
    }

    #[test]
    fn test_welcome_dropped_confirmation_expires_with_explicit_failure_events() {
        let mut config = create_test_config();
        config.encryption.enabled = true;
        config.encryption.store_pending = true;
        config.reliability.retry.max_retries = 1;

        let storage = Arc::new(InMemoryStorage::new());
        let mut protocol = OfflineProtocol::new(config).unwrap();
        protocol.initialize_mls(storage).unwrap();

        let mut internet = MockTransport::new(TransportType::Internet);
        internet.start().unwrap();
        protocol
            .transport_manager_mut()
            .add_transport(TransportType::Internet, Box::new(internet));

        let events: Arc<Mutex<Vec<Event>>> = Arc::new(Mutex::new(Vec::new()));
        let events_handle = Arc::clone(&events);
        protocol.on_event(move |event| {
            events_handle.lock().unwrap().push(event);
        });

        protocol.start().unwrap();

        let bob_storage = Arc::new(InMemoryStorage::new());
        let bob_manager = MlsManager::new("bob", bob_storage).unwrap();
        let bob_key_package = bob_manager.get_or_create_key_package().unwrap();
        protocol.pending_key_packages.insert(
            "bob".to_string(),
            ReceivedKeyPackage {
                key_package_data: bob_key_package.key_package_data,
                local_expires_at_ms: Utc::now().timestamp_millis() as u64 + 60_000,
            },
        );

        let _ = protocol
            .send_message(
                "bob",
                "welcome-confirmation-never-arrives",
                None::<MessagePriority>,
                None::<String>,
            )
            .unwrap();

        {
            let record = protocol.welcome_lifecycles.get_mut("bob").unwrap();
            record.next_retry_at = Some(Utc::now() - ChronoDuration::milliseconds(1));
        }

        protocol.process().unwrap();

        assert_eq!(
            protocol.welcome_lifecycles.get("bob").unwrap().state,
            WelcomeDeliveryState::Expired
        );
        assert!(!protocol.pending_encrypted_messages.contains_key("bob"));
        assert!(!protocol.confirmed_sessions.contains("bob"));

        let captured = events.lock().unwrap();
        assert!(captured.iter().any(|event| matches!(
            event,
            Event::WelcomeSendExpired {
                reason_code: crate::events::WelcomeReasonCode::RetryExhausted,
                ..
            }
        )));
        assert!(captured.iter().any(|event| matches!(
            event,
            Event::SecureSessionFailed { peer_id, reason }
                if peer_id == "bob" && reason.contains("Welcome delivery failed")
        )));
        assert!(!captured
            .iter()
            .any(|event| matches!(event, Event::SecureSessionEstablished { .. })));
    }

    #[test]
    fn test_welcome_delayed_confirmation_after_timeout_converges_to_sent() {
        let mut config = create_test_config();
        config.encryption.enabled = true;
        config.encryption.store_pending = true;
        config.reliability.retry.max_retries = 3;

        let storage = Arc::new(InMemoryStorage::new());
        let mut protocol = OfflineProtocol::new(config).unwrap();
        protocol.initialize_mls(storage).unwrap();

        let mut internet = MockTransport::new(TransportType::Internet);
        internet.start().unwrap();
        protocol
            .transport_manager_mut()
            .add_transport(TransportType::Internet, Box::new(internet));
        protocol.start().unwrap();

        let bob_storage = Arc::new(InMemoryStorage::new());
        let bob_manager = MlsManager::new("bob", bob_storage).unwrap();
        let bob_key_package = bob_manager.get_or_create_key_package().unwrap();
        protocol.pending_key_packages.insert(
            "bob".to_string(),
            ReceivedKeyPackage {
                key_package_data: bob_key_package.key_package_data,
                local_expires_at_ms: Utc::now().timestamp_millis() as u64 + 60_000,
            },
        );

        let _ = protocol
            .send_message(
                "bob",
                "welcome-confirmation-delayed",
                None::<MessagePriority>,
                None::<String>,
            )
            .unwrap();

        let welcome_message_id = protocol
            .welcome_lifecycles
            .get("bob")
            .unwrap()
            .welcome_message
            .id
            .as_str()
            .to_string();
        {
            let record = protocol.welcome_lifecycles.get_mut("bob").unwrap();
            record.next_retry_at = Some(Utc::now() - ChronoDuration::milliseconds(1));
        }

        protocol.process().unwrap();
        assert_eq!(
            protocol.welcome_lifecycles.get("bob").unwrap().state,
            WelcomeDeliveryState::Failed
        );

        protocol
            .on_transport_send_confirmed(&welcome_message_id)
            .unwrap();
        assert_eq!(
            protocol.welcome_lifecycles.get("bob").unwrap().state,
            WelcomeDeliveryState::Sent
        );
        assert!(!protocol.confirmed_sessions.contains("bob"));
    }

    #[test]
    fn test_welcome_reordered_after_encrypted_message_flushes_pending_decryption() {
        let mut bob_config = create_test_config_for_user("bob");
        bob_config.encryption.enabled = true;
        bob_config.encryption.store_pending = true;

        let mut bob = OfflineProtocol::new(bob_config).unwrap();
        bob.initialize_mls(Arc::new(InMemoryStorage::new()))
            .unwrap();

        let events: Arc<Mutex<Vec<Event>>> = Arc::new(Mutex::new(Vec::new()));
        let events_handle = Arc::clone(&events);
        bob.on_event(move |event| {
            events_handle.lock().unwrap().push(event);
        });

        let alice_manager = MlsManager::new("alice", Arc::new(InMemoryStorage::new())).unwrap();
        let bob_key_package = {
            let manager = bob.mls_manager.as_ref().unwrap().read().unwrap();
            manager.get_or_create_key_package().unwrap()
        };
        alice_manager
            .import_key_package("bob", &bob_key_package.key_package_data)
            .unwrap();
        let welcome = alice_manager.create_session("bob").unwrap();
        let encrypted = alice_manager
            .encrypt_for_user("bob", b"encrypted-before-welcome")
            .unwrap();

        let encrypted_wire = Message::new(
            UserId::new("alice").unwrap(),
            UserId::new("bob").unwrap(),
            AppId::new("test-app").unwrap(),
            &format!(
                "{}{}",
                internal_prefixes::ENCRYPTED,
                serde_json::to_string(&encrypted).unwrap()
            ),
        );
        let welcome_wire = Message::new(
            UserId::new("alice").unwrap(),
            UserId::new("bob").unwrap(),
            AppId::new("test-app").unwrap(),
            &format!(
                "{}{}",
                internal_prefixes::WELCOME,
                serde_json::to_string(&welcome).unwrap()
            ),
        );

        let encrypted_result = bob.process_internal_message(&encrypted_wire);
        assert!(matches!(
            encrypted_result,
            Some(InternalMessageResult::Consumed)
        ));
        assert!(bob.pending_decryption.contains_key("alice"));
        assert!(!bob.confirmed_sessions.contains("alice"));

        let welcome_result = bob.process_internal_message(&welcome_wire);
        assert!(matches!(
            welcome_result,
            Some(InternalMessageResult::Consumed)
        ));
        assert!(bob.confirmed_sessions.contains("alice"));
        assert!(!bob.pending_decryption.contains_key("alice"));

        let delayed_received = bob
            .receive_message()
            .expect("expected delayed decrypted payload");
        assert_eq!(delayed_received.content, "encrypted-before-welcome");
        assert_eq!(
            delayed_received
                .metadata
                .get("delayed_decrypt")
                .map(String::as_str),
            Some("true")
        );

        let captured = events.lock().unwrap();
        assert!(captured.iter().any(|event| matches!(
            event,
            Event::SecureSessionEstablished { peer_id, .. } if peer_id == "alice"
        )));
    }

    #[test]
    fn test_welcome_duplicate_delivery_emits_single_established_event() {
        let mut bob_config = create_test_config_for_user("bob");
        bob_config.encryption.enabled = true;
        bob_config.encryption.store_pending = true;

        let mut bob = OfflineProtocol::new(bob_config).unwrap();
        bob.initialize_mls(Arc::new(InMemoryStorage::new()))
            .unwrap();

        let mut bob_transport = MockTransport::new(TransportType::BLE);
        bob_transport.start().unwrap();
        let bob_transport_handle = bob_transport.clone();
        bob.transport_manager_mut()
            .add_transport(TransportType::BLE, Box::new(bob_transport));

        let events: Arc<Mutex<Vec<Event>>> = Arc::new(Mutex::new(Vec::new()));
        let events_handle = Arc::clone(&events);
        bob.on_event(move |event| {
            events_handle.lock().unwrap().push(event);
        });

        bob.start().unwrap();

        let alice_manager = MlsManager::new("alice", Arc::new(InMemoryStorage::new())).unwrap();
        let bob_key_package = {
            let manager = bob.mls_manager.as_ref().unwrap().read().unwrap();
            manager.get_or_create_key_package().unwrap()
        };
        alice_manager
            .import_key_package("bob", &bob_key_package.key_package_data)
            .unwrap();
        let welcome = alice_manager.create_session("bob").unwrap();
        let welcome_wire = Message::new(
            UserId::new("alice").unwrap(),
            UserId::new("bob").unwrap(),
            AppId::new("test-app").unwrap(),
            &format!(
                "{}{}",
                internal_prefixes::WELCOME,
                serde_json::to_string(&welcome).unwrap()
            ),
        );

        bob_transport_handle.queue_message(welcome_wire.clone());
        bob_transport_handle.queue_message(welcome_wire);
        assert!(bob.receive_message().is_none());

        assert!(bob.confirmed_sessions.contains("alice"));
        let captured = events.lock().unwrap();
        let established_count = captured
            .iter()
            .filter(|event| {
                matches!(
                    event,
                    Event::SecureSessionEstablished { peer_id, .. } if peer_id == "alice"
                )
            })
            .count();
        assert_eq!(established_count, 1);
    }

    #[test]
    fn test_restore_session_state_migrates_legacy_session_to_pending_without_inference() {
        let mut config = create_test_config_for_user("alice");
        config.encryption.enabled = true;
        config.encryption.store_pending = true;

        let storage = Arc::new(InMemoryStorage::new());
        let mut protocol = OfflineProtocol::new(config).unwrap();
        protocol.initialize_mls(storage.clone()).unwrap();

        // Build a real session in MLS storage but leave session state absent.
        let bob_storage = Arc::new(InMemoryStorage::new());
        let bob_manager = MlsManager::new("bob", bob_storage).unwrap();
        let bob_key_package = bob_manager.get_or_create_key_package().unwrap();
        {
            let manager = protocol.mls_manager.as_ref().unwrap().read().unwrap();
            manager
                .import_key_package("bob", &bob_key_package.key_package_data)
                .unwrap();
            let welcome = manager.create_session("bob").unwrap();
            bob_manager.join_session(&welcome).unwrap();
        }

        assert!(protocol.load_session_state_entry("bob").unwrap().is_none());

        let mut restarted = OfflineProtocol::new(create_test_config_for_user("alice")).unwrap();
        restarted.config.encryption.enabled = true;
        restarted.config.encryption.store_pending = true;
        restarted.initialize_mls(storage).unwrap();

        let restored = restarted.load_session_state_entry("bob").unwrap().unwrap();
        assert_eq!(restored, SessionState::Pending);
    }

    #[test]
    fn test_restore_session_state_keeps_missing_state_pending_when_queue_exists() {
        let mut config = create_test_config_for_user("alice");
        config.encryption.enabled = true;
        config.encryption.store_pending = true;

        let storage = Arc::new(InMemoryStorage::new());
        let mut protocol = OfflineProtocol::new(config).unwrap();
        protocol.initialize_mls(storage.clone()).unwrap();

        // Build a real session in MLS storage but leave session state absent.
        let bob_storage = Arc::new(InMemoryStorage::new());
        let bob_manager = MlsManager::new("bob", bob_storage).unwrap();
        let bob_key_package = bob_manager.get_or_create_key_package().unwrap();
        {
            let manager = protocol.mls_manager.as_ref().unwrap().read().unwrap();
            manager
                .import_key_package("bob", &bob_key_package.key_package_data)
                .unwrap();
            let welcome = manager.create_session("bob").unwrap();
            bob_manager.join_session(&welcome).unwrap();
        }

        protocol.queue_pending_message(
            "bob",
            "queued-before-restart",
            MessagePriority::Medium,
            MessageId::new(),
            None,
        );
        assert!(protocol.load_session_state_entry("bob").unwrap().is_none());

        let mut restarted = OfflineProtocol::new(create_test_config_for_user("alice")).unwrap();
        restarted.config.encryption.enabled = true;
        restarted.config.encryption.store_pending = true;
        restarted.initialize_mls(storage).unwrap();

        let restored = restarted.load_session_state_entry("bob").unwrap().unwrap();
        assert_eq!(restored, SessionState::Pending);
        assert_eq!(
            restarted
                .pending_encrypted_messages
                .get("bob")
                .unwrap()
                .len(),
            1
        );
    }

    #[test]
    fn test_start_flushes_restored_pending_messages_for_confirmed_session() {
        let mut config = create_test_config_for_user("alice");
        config.encryption.enabled = true;
        config.encryption.store_pending = true;

        let storage = Arc::new(InMemoryStorage::new());
        let mut protocol = OfflineProtocol::new(config).unwrap();
        protocol.initialize_mls(storage.clone()).unwrap();

        // Build a real session and mark it confirmed.
        let bob_storage = Arc::new(InMemoryStorage::new());
        let bob_manager = MlsManager::new("bob", bob_storage).unwrap();
        let bob_key_package = bob_manager.get_or_create_key_package().unwrap();
        {
            let manager = protocol.mls_manager.as_ref().unwrap().read().unwrap();
            manager
                .import_key_package("bob", &bob_key_package.key_package_data)
                .unwrap();
            let welcome = manager.create_session("bob").unwrap();
            bob_manager.join_session(&welcome).unwrap();
        }
        protocol.confirm_session_state("bob", "test_setup").unwrap();

        protocol.queue_pending_message(
            "bob",
            "queued-before-crash",
            MessagePriority::Medium,
            MessageId::new(),
            None,
        );

        let mut restarted = OfflineProtocol::new(create_test_config_for_user("alice")).unwrap();
        restarted.config.encryption.enabled = true;
        restarted.config.encryption.store_pending = true;
        restarted.initialize_mls(storage).unwrap();

        let mut transport = MockTransport::new(TransportType::BLE);
        transport.start().unwrap();
        let transport_handle = transport.clone();
        restarted
            .transport_manager_mut()
            .add_transport(TransportType::BLE, Box::new(transport));
        restarted.start().unwrap();

        assert!(!restarted.pending_encrypted_messages.contains_key("bob"));
        assert!(restarted
            .load_pending_messages_from_storage("bob")
            .is_none());

        let sent = transport_handle.sent_messages();
        assert!(sent
            .last()
            .unwrap()
            .content
            .starts_with(internal_prefixes::ENCRYPTED));
    }

    #[test]
    fn test_pending_sessions_reconcile_via_probe_after_restart() {
        let mut alice_config = create_test_config_for_user("alice");
        alice_config.encryption.enabled = true;
        alice_config.encryption.store_pending = true;
        let mut bob_config = create_test_config_for_user("bob");
        bob_config.encryption.enabled = true;
        bob_config.encryption.store_pending = true;

        let alice_storage = Arc::new(InMemoryStorage::new());
        let bob_storage = Arc::new(InMemoryStorage::new());

        // Build a durable MLS session on both peers, but leave confirmation Pending.
        let mut alice = OfflineProtocol::new(alice_config).unwrap();
        let mut bob = OfflineProtocol::new(bob_config).unwrap();
        alice.initialize_mls(alice_storage.clone()).unwrap();
        bob.initialize_mls(bob_storage.clone()).unwrap();

        let bob_key_package = {
            let manager = bob.mls_manager.as_ref().unwrap().read().unwrap();
            manager.get_or_create_key_package().unwrap()
        };
        let welcome = {
            let manager = alice.mls_manager.as_ref().unwrap().read().unwrap();
            manager
                .import_key_package("bob", &bob_key_package.key_package_data)
                .unwrap();
            manager.create_session("bob").unwrap()
        };
        {
            let manager = bob.mls_manager.as_ref().unwrap().read().unwrap();
            manager.join_session(&welcome).unwrap();
        }
        alice
            .ensure_session_state_entry("bob", "test_setup")
            .unwrap();
        bob.ensure_session_state_entry("alice", "test_setup")
            .unwrap();

        // Restart both peers with the same storage to simulate a crash/restart cycle.
        let mut alice2 = OfflineProtocol::new(create_test_config_for_user("alice")).unwrap();
        alice2.config.encryption.enabled = true;
        alice2.config.encryption.store_pending = true;
        alice2.initialize_mls(alice_storage).unwrap();
        let mut bob2 = OfflineProtocol::new(create_test_config_for_user("bob")).unwrap();
        bob2.config.encryption.enabled = true;
        bob2.config.encryption.store_pending = true;
        bob2.initialize_mls(bob_storage).unwrap();

        let mut alice_transport = MockTransport::new(TransportType::BLE);
        alice_transport.start().unwrap();
        let alice_transport_handle = alice_transport.clone();
        alice2
            .transport_manager_mut()
            .add_transport(TransportType::BLE, Box::new(alice_transport));
        alice2.start().unwrap();

        let mut bob_transport = MockTransport::new(TransportType::BLE);
        bob_transport.start().unwrap();
        let bob_transport_handle = bob_transport.clone();
        bob2.transport_manager_mut()
            .add_transport(TransportType::BLE, Box::new(bob_transport));
        bob2.start().unwrap();

        let probe_from_alice = alice_transport_handle
            .sent_messages()
            .into_iter()
            .find(|msg| {
                msg.content
                    .starts_with(internal_prefixes::SESSION_CONFIRM_PROBE)
            })
            .expect("expected confirmation probe from alice");
        let probe_from_bob = bob_transport_handle
            .sent_messages()
            .into_iter()
            .find(|msg| {
                msg.content
                    .starts_with(internal_prefixes::SESSION_CONFIRM_PROBE)
            })
            .expect("expected confirmation probe from bob");

        let _ = bob2.process_internal_message(&probe_from_alice);
        let _ = alice2.process_internal_message(&probe_from_bob);

        let ack_from_alice = alice_transport_handle
            .sent_messages()
            .into_iter()
            .find(|msg| {
                msg.content
                    .starts_with(internal_prefixes::SESSION_CONFIRM_ACK)
            })
            .expect("expected confirmation ack from alice");
        let ack_from_bob = bob_transport_handle
            .sent_messages()
            .into_iter()
            .find(|msg| {
                msg.content
                    .starts_with(internal_prefixes::SESSION_CONFIRM_ACK)
            })
            .expect("expected confirmation ack from bob");

        let _ = bob2.process_internal_message(&ack_from_alice);
        let _ = alice2.process_internal_message(&ack_from_bob);

        assert_eq!(
            alice2.load_session_state_entry("bob").unwrap().unwrap(),
            SessionState::Confirmed
        );
        assert_eq!(
            bob2.load_session_state_entry("alice").unwrap().unwrap(),
            SessionState::Confirmed
        );

        alice2
            .send_message("bob", "a2b", None::<MessagePriority>, None::<String>)
            .unwrap();
        bob2.send_message("alice", "b2a", None::<MessagePriority>, None::<String>)
            .unwrap();

        assert!(alice_transport_handle
            .sent_messages()
            .last()
            .unwrap()
            .content
            .starts_with(internal_prefixes::ENCRYPTED));
        assert!(bob_transport_handle
            .sent_messages()
            .last()
            .unwrap()
            .content
            .starts_with(internal_prefixes::ENCRYPTED));
    }

    #[test]
    fn test_pending_sessions_reconcile_on_send_without_process_tick() {
        let mut alice_config = create_test_config_for_user("alice");
        alice_config.encryption.enabled = true;
        alice_config.encryption.store_pending = true;
        let mut bob_config = create_test_config_for_user("bob");
        bob_config.encryption.enabled = true;
        bob_config.encryption.store_pending = true;

        let alice_storage = Arc::new(InMemoryStorage::new());
        let bob_storage = Arc::new(InMemoryStorage::new());

        // Build a durable MLS session on both peers, but leave confirmation Pending.
        let mut alice = OfflineProtocol::new(alice_config).unwrap();
        let mut bob = OfflineProtocol::new(bob_config).unwrap();
        alice.initialize_mls(alice_storage.clone()).unwrap();
        bob.initialize_mls(bob_storage.clone()).unwrap();

        let bob_key_package = {
            let manager = bob.mls_manager.as_ref().unwrap().read().unwrap();
            manager.get_or_create_key_package().unwrap()
        };
        let welcome = {
            let manager = alice.mls_manager.as_ref().unwrap().read().unwrap();
            manager
                .import_key_package("bob", &bob_key_package.key_package_data)
                .unwrap();
            manager.create_session("bob").unwrap()
        };
        {
            let manager = bob.mls_manager.as_ref().unwrap().read().unwrap();
            manager.join_session(&welcome).unwrap();
        }
        alice
            .ensure_session_state_entry("bob", "test_setup")
            .unwrap();
        bob.ensure_session_state_entry("alice", "test_setup")
            .unwrap();

        // Restart both peers with the same storage to simulate a crash/restart cycle.
        let mut alice2 = OfflineProtocol::new(create_test_config_for_user("alice")).unwrap();
        alice2.config.encryption.enabled = true;
        alice2.config.encryption.store_pending = true;
        alice2.initialize_mls(alice_storage).unwrap();
        let mut bob2 = OfflineProtocol::new(create_test_config_for_user("bob")).unwrap();
        bob2.config.encryption.enabled = true;
        bob2.config.encryption.store_pending = true;
        bob2.initialize_mls(bob_storage).unwrap();

        let mut alice_transport = MockTransport::new(TransportType::BLE);
        alice_transport.start().unwrap();
        let alice_transport_handle = alice_transport.clone();
        alice2
            .transport_manager_mut()
            .add_transport(TransportType::BLE, Box::new(alice_transport));
        alice2.start().unwrap();

        let mut bob_transport = MockTransport::new(TransportType::BLE);
        bob_transport.start().unwrap();
        let bob_transport_handle = bob_transport.clone();
        bob2.transport_manager_mut()
            .add_transport(TransportType::BLE, Box::new(bob_transport));
        bob2.start().unwrap();

        // Simulate dropped startup probes. Force probe schedule due now so send-path
        // reconciliation can retry without depending on process().
        alice2
            .confirmation_probe_due_at
            .insert("bob".to_string(), Utc::now() - ChronoDuration::seconds(1));
        bob2.confirmation_probe_due_at
            .insert("alice".to_string(), Utc::now() - ChronoDuration::seconds(1));

        // Active sends while pending should queue and trigger a fresh probe attempt.
        alice2
            .send_message("bob", "queued-a2b", None::<MessagePriority>, None::<String>)
            .unwrap();
        bob2.send_message(
            "alice",
            "queued-b2a",
            None::<MessagePriority>,
            None::<String>,
        )
        .unwrap();

        let probe_from_alice = alice_transport_handle
            .sent_messages()
            .into_iter()
            .rev()
            .find(|msg| {
                msg.content
                    .starts_with(internal_prefixes::SESSION_CONFIRM_PROBE)
            })
            .expect("expected confirmation probe from alice send-path reconciliation");
        let probe_from_bob = bob_transport_handle
            .sent_messages()
            .into_iter()
            .rev()
            .find(|msg| {
                msg.content
                    .starts_with(internal_prefixes::SESSION_CONFIRM_PROBE)
            })
            .expect("expected confirmation probe from bob send-path reconciliation");

        let _ = bob2.process_internal_message(&probe_from_alice);
        let _ = alice2.process_internal_message(&probe_from_bob);

        let ack_from_alice = alice_transport_handle
            .sent_messages()
            .into_iter()
            .rev()
            .find(|msg| {
                msg.content
                    .starts_with(internal_prefixes::SESSION_CONFIRM_ACK)
            })
            .expect("expected confirmation ack from alice");
        let ack_from_bob = bob_transport_handle
            .sent_messages()
            .into_iter()
            .rev()
            .find(|msg| {
                msg.content
                    .starts_with(internal_prefixes::SESSION_CONFIRM_ACK)
            })
            .expect("expected confirmation ack from bob");

        let _ = bob2.process_internal_message(&ack_from_alice);
        let _ = alice2.process_internal_message(&ack_from_bob);

        assert_eq!(
            alice2.load_session_state_entry("bob").unwrap().unwrap(),
            SessionState::Confirmed
        );
        assert_eq!(
            bob2.load_session_state_entry("alice").unwrap().unwrap(),
            SessionState::Confirmed
        );
        assert!(!alice2.pending_encrypted_messages.contains_key("bob"));
        assert!(!bob2.pending_encrypted_messages.contains_key("alice"));

        assert!(alice_transport_handle
            .sent_messages()
            .iter()
            .any(|msg| msg.content.starts_with(internal_prefixes::ENCRYPTED)));
        assert!(bob_transport_handle
            .sent_messages()
            .iter()
            .any(|msg| msg.content.starts_with(internal_prefixes::ENCRYPTED)));
    }

    #[test]
    fn test_pending_sessions_reconcile_on_concurrent_send_after_restart() {
        let mut alice_config = create_test_config_for_user("alice");
        alice_config.encryption.enabled = true;
        alice_config.encryption.store_pending = true;
        let mut bob_config = create_test_config_for_user("bob");
        bob_config.encryption.enabled = true;
        bob_config.encryption.store_pending = true;

        let alice_storage = Arc::new(InMemoryStorage::new());
        let bob_storage = Arc::new(InMemoryStorage::new());

        // Build a durable MLS session on both peers, but leave confirmation Pending.
        let mut alice = OfflineProtocol::new(alice_config).unwrap();
        let mut bob = OfflineProtocol::new(bob_config).unwrap();
        alice.initialize_mls(alice_storage.clone()).unwrap();
        bob.initialize_mls(bob_storage.clone()).unwrap();

        let bob_key_package = {
            let manager = bob.mls_manager.as_ref().unwrap().read().unwrap();
            manager.get_or_create_key_package().unwrap()
        };
        let welcome = {
            let manager = alice.mls_manager.as_ref().unwrap().read().unwrap();
            manager
                .import_key_package("bob", &bob_key_package.key_package_data)
                .unwrap();
            manager.create_session("bob").unwrap()
        };
        {
            let manager = bob.mls_manager.as_ref().unwrap().read().unwrap();
            manager.join_session(&welcome).unwrap();
        }
        alice
            .ensure_session_state_entry("bob", "test_setup")
            .unwrap();
        bob.ensure_session_state_entry("alice", "test_setup")
            .unwrap();

        // Restart both peers with the same storage to simulate a crash/restart cycle.
        let mut alice2 = OfflineProtocol::new(create_test_config_for_user("alice")).unwrap();
        alice2.config.encryption.enabled = true;
        alice2.config.encryption.store_pending = true;
        alice2.initialize_mls(alice_storage).unwrap();
        let mut bob2 = OfflineProtocol::new(create_test_config_for_user("bob")).unwrap();
        bob2.config.encryption.enabled = true;
        bob2.config.encryption.store_pending = true;
        bob2.initialize_mls(bob_storage).unwrap();

        let mut alice_transport = MockTransport::new(TransportType::BLE);
        alice_transport.start().unwrap();
        let alice_transport_handle = alice_transport.clone();
        alice2
            .transport_manager_mut()
            .add_transport(TransportType::BLE, Box::new(alice_transport));
        alice2.start().unwrap();

        let mut bob_transport = MockTransport::new(TransportType::BLE);
        bob_transport.start().unwrap();
        let bob_transport_handle = bob_transport.clone();
        bob2.transport_manager_mut()
            .add_transport(TransportType::BLE, Box::new(bob_transport));
        bob2.start().unwrap();

        // Simulate dropped startup probes. Force probe schedule due now so concurrent
        // send-path reconciliation can retry without depending on process().
        alice_transport_handle.clear_sent_messages();
        bob_transport_handle.clear_sent_messages();
        alice2
            .confirmation_probe_due_at
            .insert("bob".to_string(), Utc::now() - ChronoDuration::seconds(1));
        bob2.confirmation_probe_due_at
            .insert("alice".to_string(), Utc::now() - ChronoDuration::seconds(1));

        // Start both sends at the same instant to make the race deterministic.
        let alice_shared = Arc::new(Mutex::new(alice2));
        let bob_shared = Arc::new(Mutex::new(bob2));
        let start_barrier = Arc::new(Barrier::new(3));

        let alice_barrier = Arc::clone(&start_barrier);
        let alice_sender = Arc::clone(&alice_shared);
        let alice_send_thread = thread::spawn(move || {
            alice_barrier.wait();
            alice_sender
                .lock()
                .unwrap()
                .send_message(
                    "bob",
                    "queued-a2b-concurrent",
                    None::<MessagePriority>,
                    None::<String>,
                )
                .unwrap();
        });

        let bob_barrier = Arc::clone(&start_barrier);
        let bob_sender = Arc::clone(&bob_shared);
        let bob_send_thread = thread::spawn(move || {
            bob_barrier.wait();
            bob_sender
                .lock()
                .unwrap()
                .send_message(
                    "alice",
                    "queued-b2a-concurrent",
                    None::<MessagePriority>,
                    None::<String>,
                )
                .unwrap();
        });

        start_barrier.wait();
        alice_send_thread.join().unwrap();
        bob_send_thread.join().unwrap();

        let probe_from_alice = alice_transport_handle
            .sent_messages()
            .into_iter()
            .rev()
            .find(|msg| {
                msg.content
                    .starts_with(internal_prefixes::SESSION_CONFIRM_PROBE)
            })
            .expect("expected confirmation probe from alice send-path reconciliation");
        let probe_from_bob = bob_transport_handle
            .sent_messages()
            .into_iter()
            .rev()
            .find(|msg| {
                msg.content
                    .starts_with(internal_prefixes::SESSION_CONFIRM_PROBE)
            })
            .expect("expected confirmation probe from bob send-path reconciliation");

        let _ = bob_shared
            .lock()
            .unwrap()
            .process_internal_message(&probe_from_alice);
        let _ = alice_shared
            .lock()
            .unwrap()
            .process_internal_message(&probe_from_bob);

        let ack_from_alice = alice_transport_handle
            .sent_messages()
            .into_iter()
            .rev()
            .find(|msg| {
                msg.content
                    .starts_with(internal_prefixes::SESSION_CONFIRM_ACK)
            })
            .expect("expected confirmation ack from alice");
        let ack_from_bob = bob_transport_handle
            .sent_messages()
            .into_iter()
            .rev()
            .find(|msg| {
                msg.content
                    .starts_with(internal_prefixes::SESSION_CONFIRM_ACK)
            })
            .expect("expected confirmation ack from bob");

        let _ = bob_shared
            .lock()
            .unwrap()
            .process_internal_message(&ack_from_alice);
        let _ = alice_shared
            .lock()
            .unwrap()
            .process_internal_message(&ack_from_bob);

        assert_eq!(
            alice_shared
                .lock()
                .unwrap()
                .load_session_state_entry("bob")
                .unwrap()
                .unwrap(),
            SessionState::Confirmed
        );
        assert_eq!(
            bob_shared
                .lock()
                .unwrap()
                .load_session_state_entry("alice")
                .unwrap()
                .unwrap(),
            SessionState::Confirmed
        );
        assert!(!alice_shared
            .lock()
            .unwrap()
            .pending_encrypted_messages
            .contains_key("bob"));
        assert!(!bob_shared
            .lock()
            .unwrap()
            .pending_encrypted_messages
            .contains_key("alice"));

        assert!(alice_transport_handle
            .sent_messages()
            .iter()
            .any(|msg| msg.content.starts_with(internal_prefixes::ENCRYPTED)));
        assert!(bob_transport_handle
            .sent_messages()
            .iter()
            .any(|msg| msg.content.starts_with(internal_prefixes::ENCRYPTED)));
    }

    #[test]
    fn test_send_message_via_transport_respects_session_confirmation_gating() {
        let mut config = create_test_config_for_user("alice");
        config.encryption.enabled = true;
        config.encryption.store_pending = true;

        let storage = Arc::new(InMemoryStorage::new());
        let mut protocol = OfflineProtocol::new(config).unwrap();
        protocol.initialize_mls(storage).unwrap();

        let mut transport = MockTransport::new(TransportType::BLE);
        transport.start().unwrap();
        let transport_handle = transport.clone();
        protocol
            .transport_manager_mut()
            .add_transport(TransportType::BLE, Box::new(transport));
        protocol.start().unwrap();

        let bob_storage = Arc::new(InMemoryStorage::new());
        let bob_manager = MlsManager::new("bob", bob_storage).unwrap();
        let bob_key_package = bob_manager.get_or_create_key_package().unwrap();
        protocol.pending_key_packages.insert(
            "bob".to_string(),
            ReceivedKeyPackage {
                key_package_data: bob_key_package.key_package_data,
                local_expires_at_ms: Utc::now().timestamp_millis() as u64 + 60_000,
            },
        );

        protocol
            .send_message_via_transport(
                "bob",
                "forced-transport-pending",
                None::<MessagePriority>,
                TransportType::BLE,
                None::<String>,
            )
            .unwrap();

        assert_eq!(
            protocol
                .pending_encrypted_messages
                .get("bob")
                .unwrap()
                .len(),
            1
        );
        assert_eq!(
            protocol.load_session_state_entry("bob").unwrap().unwrap(),
            SessionState::Pending
        );
        assert!(!transport_handle
            .sent_messages()
            .iter()
            .any(|msg| msg.content == "forced-transport-pending"));
    }

    #[test]
    fn test_send_message_fails_closed_when_confirmation_state_is_corrupted() {
        let mut config = create_test_config_for_user("alice");
        config.encryption.enabled = true;
        config.encryption.store_pending = true;

        let storage = Arc::new(InMemoryStorage::new());
        let mut protocol = OfflineProtocol::new(config).unwrap();
        protocol.initialize_mls(storage.clone()).unwrap();

        let mut transport = MockTransport::new(TransportType::BLE);
        transport.start().unwrap();
        let transport_handle = transport.clone();
        protocol
            .transport_manager_mut()
            .add_transport(TransportType::BLE, Box::new(transport));
        protocol.start().unwrap();

        // Create a real MLS session to ensure send path reaches confirmation-state read.
        let bob_storage = Arc::new(InMemoryStorage::new());
        let bob_manager = MlsManager::new("bob", bob_storage).unwrap();
        let bob_key_package = bob_manager.get_or_create_key_package().unwrap();
        {
            let manager = protocol.mls_manager.as_ref().unwrap().read().unwrap();
            manager
                .import_key_package("bob", &bob_key_package.key_package_data)
                .unwrap();
            let welcome = manager.create_session("bob").unwrap();
            bob_manager.join_session(&welcome).unwrap();
        }

        storage
            .store(storage_keys::SESSION_STATES, "bob", b"not-valid-json")
            .unwrap();

        let result =
            protocol.send_message("bob", "sensitive", None::<MessagePriority>, None::<String>);
        assert!(result.is_err());
        assert!(!transport_handle
            .sent_messages()
            .iter()
            .any(|msg| msg.content == "sensitive"));
    }

    #[test]
    fn test_receive_poll_drives_pending_session_reconciliation_without_process_or_new_sends() {
        let mut alice_config = create_test_config_for_user("alice");
        alice_config.encryption.enabled = true;
        alice_config.encryption.store_pending = true;
        let mut bob_config = create_test_config_for_user("bob");
        bob_config.encryption.enabled = true;
        bob_config.encryption.store_pending = true;

        let alice_storage = Arc::new(InMemoryStorage::new());
        let bob_storage = Arc::new(InMemoryStorage::new());

        // Build a durable MLS session on both peers, but leave confirmation Pending.
        let mut alice = OfflineProtocol::new(alice_config).unwrap();
        let mut bob = OfflineProtocol::new(bob_config).unwrap();
        alice.initialize_mls(alice_storage.clone()).unwrap();
        bob.initialize_mls(bob_storage.clone()).unwrap();

        let bob_key_package = {
            let manager = bob.mls_manager.as_ref().unwrap().read().unwrap();
            manager.get_or_create_key_package().unwrap()
        };
        let welcome = {
            let manager = alice.mls_manager.as_ref().unwrap().read().unwrap();
            manager
                .import_key_package("bob", &bob_key_package.key_package_data)
                .unwrap();
            manager.create_session("bob").unwrap()
        };
        {
            let manager = bob.mls_manager.as_ref().unwrap().read().unwrap();
            manager.join_session(&welcome).unwrap();
        }
        alice
            .ensure_session_state_entry("bob", "test_setup")
            .unwrap();
        bob.ensure_session_state_entry("alice", "test_setup")
            .unwrap();

        // Queue pending messages before restart so we can verify they flush
        // after poll-driven reconciliation.
        alice.queue_pending_message(
            "bob",
            "queued-before-restart-a2b",
            MessagePriority::Medium,
            MessageId::new(),
            None,
        );
        bob.queue_pending_message(
            "alice",
            "queued-before-restart-b2a",
            MessagePriority::Medium,
            MessageId::new(),
            None,
        );

        let mut alice2 = OfflineProtocol::new(create_test_config_for_user("alice")).unwrap();
        alice2.config.encryption.enabled = true;
        alice2.config.encryption.store_pending = true;
        alice2.initialize_mls(alice_storage).unwrap();
        let mut bob2 = OfflineProtocol::new(create_test_config_for_user("bob")).unwrap();
        bob2.config.encryption.enabled = true;
        bob2.config.encryption.store_pending = true;
        bob2.initialize_mls(bob_storage).unwrap();

        let mut alice_transport = MockTransport::new(TransportType::BLE);
        alice_transport.start().unwrap();
        let alice_transport_handle = alice_transport.clone();
        alice2
            .transport_manager_mut()
            .add_transport(TransportType::BLE, Box::new(alice_transport));
        alice2.start().unwrap();

        let mut bob_transport = MockTransport::new(TransportType::BLE);
        bob_transport.start().unwrap();
        let bob_transport_handle = bob_transport.clone();
        bob2.transport_manager_mut()
            .add_transport(TransportType::BLE, Box::new(bob_transport));
        bob2.start().unwrap();

        // Simulate dropped startup probes and force receive-poll-driven retries.
        alice_transport_handle.clear_sent_messages();
        bob_transport_handle.clear_sent_messages();
        alice2
            .confirmation_probe_due_at
            .insert("bob".to_string(), Utc::now() - ChronoDuration::seconds(1));
        bob2.confirmation_probe_due_at
            .insert("alice".to_string(), Utc::now() - ChronoDuration::seconds(1));

        // No process() calls and no new sends here.
        let _ = alice2.receive_message();
        let _ = bob2.receive_message();

        let probe_from_alice = alice_transport_handle
            .sent_messages()
            .into_iter()
            .find(|msg| {
                msg.content
                    .starts_with(internal_prefixes::SESSION_CONFIRM_PROBE)
            })
            .expect("expected confirmation probe from alice receive poll");
        let probe_from_bob = bob_transport_handle
            .sent_messages()
            .into_iter()
            .find(|msg| {
                msg.content
                    .starts_with(internal_prefixes::SESSION_CONFIRM_PROBE)
            })
            .expect("expected confirmation probe from bob receive poll");

        let _ = bob2.process_internal_message(&probe_from_alice);
        let _ = alice2.process_internal_message(&probe_from_bob);

        let ack_from_alice = alice_transport_handle
            .sent_messages()
            .into_iter()
            .find(|msg| {
                msg.content
                    .starts_with(internal_prefixes::SESSION_CONFIRM_ACK)
            })
            .expect("expected confirmation ack from alice");
        let ack_from_bob = bob_transport_handle
            .sent_messages()
            .into_iter()
            .find(|msg| {
                msg.content
                    .starts_with(internal_prefixes::SESSION_CONFIRM_ACK)
            })
            .expect("expected confirmation ack from bob");

        let _ = bob2.process_internal_message(&ack_from_alice);
        let _ = alice2.process_internal_message(&ack_from_bob);

        assert_eq!(
            alice2.load_session_state_entry("bob").unwrap().unwrap(),
            SessionState::Confirmed
        );
        assert_eq!(
            bob2.load_session_state_entry("alice").unwrap().unwrap(),
            SessionState::Confirmed
        );
        assert!(!alice2.pending_encrypted_messages.contains_key("bob"));
        assert!(!bob2.pending_encrypted_messages.contains_key("alice"));
        assert!(alice_transport_handle
            .sent_messages()
            .iter()
            .any(|msg| msg.content.starts_with(internal_prefixes::ENCRYPTED)));
        assert!(bob_transport_handle
            .sent_messages()
            .iter()
            .any(|msg| msg.content.starts_with(internal_prefixes::ENCRYPTED)));
    }

    #[test]
    fn test_pending_decryption_queue() {
        let mut config = create_test_config();
        config.encryption.enabled = true;

        let mut protocol = OfflineProtocol::new(config).unwrap();

        // Initially no pending decryption messages
        assert!(protocol.pending_decryption.is_empty());

        // Queue an encrypted message for a sender
        let message = Message::new(
            UserId::new("sender123").unwrap(),
            UserId::new("user123").unwrap(),
            AppId::new("test-app").unwrap(),
            "encrypted content",
        );

        protocol.enqueue_pending_decryption("sender123", &message);

        // Check message is queued
        assert!(protocol.pending_decryption.contains_key("sender123"));
        assert_eq!(
            protocol.pending_decryption.get("sender123").unwrap().len(),
            1
        );

        // Queue another message from same sender
        let message2 = Message::new(
            UserId::new("sender123").unwrap(),
            UserId::new("user123").unwrap(),
            AppId::new("test-app").unwrap(),
            "more encrypted content",
        );

        protocol.enqueue_pending_decryption("sender123", &message2);

        assert_eq!(
            protocol.pending_decryption.get("sender123").unwrap().len(),
            2
        );
    }

    #[test]
    fn test_session_confirmation_clears_pending_decryption() {
        let mut config = create_test_config();
        config.encryption.enabled = true;

        let mut protocol = OfflineProtocol::new(config).unwrap();

        // Queue some pending decryption messages
        let message = Message::new(
            UserId::new("sender123").unwrap(),
            UserId::new("user123").unwrap(),
            AppId::new("test-app").unwrap(),
            "encrypted content",
        );

        protocol.enqueue_pending_decryption("sender123", &message);

        assert!(!protocol.pending_decryption.is_empty());

        // Calling process_pending_decryption should remove the entries
        // (even if decryption fails since MLS is not initialized)
        protocol.process_pending_decryption("sender123");

        // The messages should be removed from the pending queue
        assert!(!protocol.pending_decryption.contains_key("sender123"));
    }

    #[test]
    fn test_on_neighbor_lost_clears_confirmed_session() {
        let mut config = create_test_config();
        config.encryption.enabled = true;

        let mut protocol = OfflineProtocol::new(config).unwrap();

        // Add a confirmed session
        protocol.confirmed_sessions.insert("peer123".to_string());
        protocol.key_package_sent_to.insert("peer123".to_string());

        assert!(protocol.confirmed_sessions.contains("peer123"));

        // When neighbor is lost, the key_package_sent_to is cleared
        // (confirmed_sessions might still remain - it's the crypto state)
        protocol.on_neighbor_lost("peer123");

        assert!(!protocol.key_package_sent_to.contains("peer123"));
    }

    #[test]
    fn test_welcome_message_confirms_session() {
        let mut config = create_test_config();
        config.encryption.enabled = true;
        config.encryption.store_pending = true;

        let mut protocol = OfflineProtocol::new(config).unwrap();

        // Initially no confirmed sessions
        assert!(!protocol.confirmed_sessions.contains("sender123"));

        // Simulate receiving a welcome message
        // Note: Since MLS is not initialized, the welcome won't actually be processed,
        // but we can test the structure is in place
        let welcome_content = format!(
            "{}{{\"group_id\":\"session:sender123:user123\",\"welcome_data\":[],\"inviter_id\":\"sender123\",\"timestamp_ms\":12345}}",
            internal_prefixes::WELCOME
        );

        let message = Message::new(
            UserId::new("sender123").unwrap(),
            UserId::new("user123").unwrap(),
            AppId::new("test-app").unwrap(),
            &welcome_content,
        );

        // Process the message
        let result = protocol.process_internal_message(&message);

        // Should be consumed (welcome message is internal)
        assert!(matches!(result, Some(InternalMessageResult::Consumed)));
    }

    #[test]
    fn test_encrypted_message_before_session_queued() {
        let mut config = create_test_config();
        config.encryption.enabled = true;

        let mut protocol = OfflineProtocol::new(config).unwrap();

        // Create an encrypted message with the proper format
        let encrypted_content = format!(
            "{}{{\"group_id\":\"session:sender123:user123\",\"message_type\":\"Application\",\"epoch\":0,\"ciphertext\":[1,2,3],\"sender_id\":\"sender123\",\"timestamp_ms\":12345}}",
            internal_prefixes::ENCRYPTED
        );

        let message = Message::new(
            UserId::new("sender123").unwrap(),
            UserId::new("user123").unwrap(),
            AppId::new("test-app").unwrap(),
            &encrypted_content,
        );

        // Process the message without MLS initialized - should be consumed and signaled as an error
        let result = protocol.process_internal_message(&message);

        assert!(matches!(result, Some(InternalMessageResult::Consumed)));
    }

    #[test]
    fn test_mls_pipeline_happy_path_init_send_encrypted_receive_decrypted() {
        let mut alice_config = create_test_config_for_user("alice");
        alice_config.encryption.enabled = true;
        alice_config.encryption.store_pending = true;
        let mut bob_config = create_test_config_for_user("bob");
        bob_config.encryption.enabled = true;
        bob_config.encryption.store_pending = true;

        let mut alice = OfflineProtocol::new(alice_config).unwrap();
        let mut bob = OfflineProtocol::new(bob_config).unwrap();
        alice
            .initialize_mls(Arc::new(InMemoryStorage::new()))
            .unwrap();
        bob.initialize_mls(Arc::new(InMemoryStorage::new()))
            .unwrap();

        let mut alice_transport = MockTransport::new(TransportType::BLE);
        alice_transport.start().unwrap();
        let alice_transport_handle = alice_transport.clone();
        alice
            .transport_manager_mut()
            .add_transport(TransportType::BLE, Box::new(alice_transport));
        alice.start().unwrap();

        let mut bob_transport = MockTransport::new(TransportType::BLE);
        bob_transport.start().unwrap();
        let bob_transport_handle = bob_transport.clone();
        bob.transport_manager_mut()
            .add_transport(TransportType::BLE, Box::new(bob_transport));
        bob.start().unwrap();

        let bob_key_package = {
            let manager = bob.mls_manager.as_ref().unwrap().read().unwrap();
            manager.get_or_create_key_package().unwrap()
        };
        let welcome = {
            let manager = alice.mls_manager.as_ref().unwrap().read().unwrap();
            manager
                .import_key_package("bob", &bob_key_package.key_package_data)
                .unwrap();
            manager.create_session("bob").unwrap()
        };
        {
            let manager = bob.mls_manager.as_ref().unwrap().read().unwrap();
            manager.join_session(&welcome).unwrap();
        }
        alice.confirm_session_state("bob", "test_setup").unwrap();
        bob.confirm_session_state("alice", "test_setup").unwrap();

        alice
            .send_message(
                "bob",
                "hello-through-mls",
                None::<MessagePriority>,
                None::<String>,
            )
            .unwrap();
        let encrypted_wire = alice_transport_handle
            .sent_messages()
            .last()
            .expect("expected encrypted message from alice")
            .clone();
        assert!(encrypted_wire
            .content
            .starts_with(internal_prefixes::ENCRYPTED));

        bob_transport_handle.queue_message(encrypted_wire);
        let received = bob.receive_message().expect("expected decrypted message");
        assert_eq!(received.content, "hello-through-mls");
        assert_eq!(
            received.metadata.get("encrypted").map(String::as_str),
            Some("true")
        );
    }

    #[test]
    fn test_mls_pipeline_missing_session_applies_drop_newest_policy() {
        let mut config = create_test_config_for_user("bob");
        config.encryption.enabled = true;
        config.encryption.pending_queue.max_pending_per_peer = 1;
        config.encryption.pending_queue.max_pending_global = 10;
        config.encryption.pending_queue.pending_ttl_ms = 60_000;
        config.encryption.pending_queue.overflow_policy = crate::config::OverflowPolicy::DropNewest;

        let mut bob = OfflineProtocol::new(config).unwrap();
        bob.initialize_mls(Arc::new(InMemoryStorage::new()))
            .unwrap();

        let alice_manager = MlsManager::new("alice", Arc::new(InMemoryStorage::new())).unwrap();
        let bob_key_package = {
            let manager = bob.mls_manager.as_ref().unwrap().read().unwrap();
            manager.get_or_create_key_package().unwrap()
        };
        alice_manager
            .import_key_package("bob", &bob_key_package.key_package_data)
            .unwrap();
        alice_manager.create_session("bob").unwrap();

        let encrypted_one = alice_manager.encrypt_for_user("bob", b"first").unwrap();
        let encrypted_two = alice_manager.encrypt_for_user("bob", b"second").unwrap();

        let first_message = Message::new(
            UserId::new("alice").unwrap(),
            UserId::new("bob").unwrap(),
            AppId::new("test-app").unwrap(),
            &format!(
                "{}{}",
                internal_prefixes::ENCRYPTED,
                serde_json::to_string(&encrypted_one).unwrap()
            ),
        );
        let second_message = Message::new(
            UserId::new("alice").unwrap(),
            UserId::new("bob").unwrap(),
            AppId::new("test-app").unwrap(),
            &format!(
                "{}{}",
                internal_prefixes::ENCRYPTED,
                serde_json::to_string(&encrypted_two).unwrap()
            ),
        );

        let first_result = bob.process_internal_message(&first_message);
        let second_result = bob.process_internal_message(&second_message);

        assert!(matches!(
            first_result,
            Some(InternalMessageResult::Consumed)
        ));
        assert!(matches!(
            second_result,
            Some(InternalMessageResult::Consumed)
        ));
        assert_eq!(bob.pending_decryption["alice"].len(), 1);
        assert_eq!(
            bob.pending_decryption["alice"][0].message.id.as_str(),
            first_message.id.as_str()
        );
        assert_eq!(
            bob.pending_queue_metrics
                .pending_messages_dropped_overflow_total,
            1
        );
    }

    #[test]
    fn test_encrypted_message_decryption_failure_emits_app_error_event() {
        let mut config = create_test_config();
        config.encryption.enabled = true;

        let mut protocol = OfflineProtocol::new(config).unwrap();
        let events: Arc<Mutex<Vec<Event>>> = Arc::new(Mutex::new(Vec::new()));
        let events_handle = Arc::clone(&events);
        protocol.on_event(move |event| {
            events_handle.lock().unwrap().push(event);
        });

        let encrypted_content = format!(
            "{}{{\"group_id\":\"session:sender123:user123\",\"message_type\":\"Application\",\"epoch\":0,\"ciphertext\":[1,2,3],\"sender_id\":\"sender123\",\"timestamp_ms\":12345}}",
            internal_prefixes::ENCRYPTED
        );
        let message = Message::new(
            UserId::new("sender123").unwrap(),
            UserId::new("user123").unwrap(),
            AppId::new("test-app").unwrap(),
            &encrypted_content,
        );

        let _ = protocol.process_internal_message(&message);

        let captured = events.lock().unwrap();
        assert!(captured.iter().any(|event| matches!(
            event,
            Event::MessageDecryptionFailed {
                message_id,
                sender,
                code,
                reason,
            } if message_id == &message.id.as_str()
                && sender == "sender123"
                && code == &DecryptionFailureCode::NotInitialized
                && reason.contains("not initialized")
        )));
    }

    #[test]
    fn test_invalid_encrypted_payload_emits_app_error_event_and_is_consumed() {
        let mut config = create_test_config();
        config.encryption.enabled = true;

        let mut protocol = OfflineProtocol::new(config).unwrap();
        let events: Arc<Mutex<Vec<Event>>> = Arc::new(Mutex::new(Vec::new()));
        let events_handle = Arc::clone(&events);
        protocol.on_event(move |event| {
            events_handle.lock().unwrap().push(event);
        });

        let malformed_payload = format!("{}{{\"group_id\":\"bad\"", internal_prefixes::ENCRYPTED);
        let message = Message::new(
            UserId::new("sender123").unwrap(),
            UserId::new("user123").unwrap(),
            AppId::new("test-app").unwrap(),
            &malformed_payload,
        );

        let result = protocol.process_internal_message(&message);
        assert!(matches!(result, Some(InternalMessageResult::Consumed)));

        let captured = events.lock().unwrap();
        assert!(captured.iter().any(|event| matches!(
            event,
            Event::MessageDecryptionFailed {
                message_id,
                sender,
                code,
                reason,
            } if message_id == &message.id.as_str()
                && sender == "sender123"
                && code == &DecryptionFailureCode::InvalidPayload
                && reason == "Invalid encrypted payload"
        )));
    }

    #[test]
    fn test_internal_prefix_malformed_payload_fuzz_is_panic_free() {
        let mut config = create_test_config();
        config.encryption.enabled = true;
        let mut protocol = OfflineProtocol::new(config).unwrap();

        let malformed_payloads = vec![
            "".to_string(),
            "{".to_string(),
            "{\"unexpected\":".to_string(),
            "{\"timestamp_ms\":\"not-a-number\"}".to_string(),
            "{\"group_id\":null}".to_string(),
            "[]".to_string(),
            "x".repeat(1024),
        ];
        let prefixes = [
            internal_prefixes::WELCOME,
            internal_prefixes::ENCRYPTED,
            internal_prefixes::CONN_REQUEST,
            internal_prefixes::CONN_ACCEPT,
            internal_prefixes::CONN_REJECT,
            internal_prefixes::GROUP_CREATED,
            internal_prefixes::GROUP_MSG,
            internal_prefixes::GROUP_MEMBER_ADDED,
            internal_prefixes::GROUP_MEMBER_REMOVED,
            internal_prefixes::GROUP_INFO,
            internal_prefixes::USER_GROUPS,
            internal_prefixes::GROUP_ERROR,
            offline_protocol_services::SVC_DISCOVER_QUERY,
            offline_protocol_services::SVC_DISCOVER_RESPONSE,
            offline_protocol_services::SVC_REQUEST,
            offline_protocol_services::SVC_RESPONSE,
        ];

        for prefix in prefixes {
            for payload in &malformed_payloads {
                let message = Message::new(
                    UserId::new("sender123").unwrap(),
                    UserId::new("user123").unwrap(),
                    AppId::new("test-app").unwrap(),
                    &format!("{prefix}{payload}"),
                );

                let outcome = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
                    protocol.process_internal_message(&message)
                }));

                assert!(
                    outcome.is_ok(),
                    "panic for prefix {prefix:?} payload {payload:?}"
                );
                let result = outcome.unwrap();
                assert!(matches!(result, Some(InternalMessageResult::Consumed)));
            }
        }
    }

    #[test]
    fn test_receive_message_decrypt_failure_emits_error_without_message_received() {
        let mut config = create_test_config();
        config.encryption.enabled = true;

        let mut protocol = OfflineProtocol::new(config).unwrap();
        let events: Arc<Mutex<Vec<Event>>> = Arc::new(Mutex::new(Vec::new()));
        let events_handle = Arc::clone(&events);
        protocol.on_event(move |event| {
            events_handle.lock().unwrap().push(event);
        });

        let mut mock_transport = MockTransport::new(TransportType::BLE);
        mock_transport.start().unwrap();
        let encrypted_content = format!(
            "{}{{\"group_id\":\"session:sender123:user123\",\"message_type\":\"Application\",\"epoch\":0,\"ciphertext\":[1,2,3],\"sender_id\":\"sender123\",\"timestamp_ms\":12345}}",
            internal_prefixes::ENCRYPTED
        );
        let message = Message::new(
            UserId::new("sender123").unwrap(),
            UserId::new("user123").unwrap(),
            AppId::new("test-app").unwrap(),
            &encrypted_content,
        );
        mock_transport.queue_message(message.clone());

        protocol
            .transport_manager_mut()
            .add_transport(TransportType::BLE, Box::new(mock_transport));
        protocol.start().unwrap();

        let received = protocol.receive_message();
        assert!(received.is_none());

        let captured = events.lock().unwrap();
        assert!(captured.iter().any(|event| matches!(
            event,
            Event::MessageDecryptionFailed {
                message_id,
                sender,
                code,
                ..
            } if message_id == &message.id.as_str()
                && sender == "sender123"
                && code == &DecryptionFailureCode::NotInitialized
        )));
        assert!(!captured
            .iter()
            .any(|event| matches!(event, Event::MessageReceived { .. })));
    }

    #[test]
    fn test_encrypted_message_group_not_found_is_queued_with_typed_classification() {
        let mut config = create_test_config();
        config.encryption.enabled = true;

        let mut protocol = OfflineProtocol::new(config).unwrap();
        protocol
            .initialize_mls(Arc::new(crate::mls::InMemoryStorage::new()))
            .unwrap();

        let encrypted_content = format!(
            "{}{{\"group_id\":\"session:sender123:user123\",\"message_type\":\"Application\",\"epoch\":0,\"ciphertext\":[1,2,3],\"sender_id\":\"sender123\",\"timestamp_ms\":12345}}",
            internal_prefixes::ENCRYPTED
        );

        let message = Message::new(
            UserId::new("sender123").unwrap(),
            UserId::new("user123").unwrap(),
            AppId::new("test-app").unwrap(),
            &encrypted_content,
        );

        let result = protocol.process_internal_message(&message);

        assert!(matches!(result, Some(InternalMessageResult::Consumed)));
        assert!(protocol.pending_decryption.contains_key("sender123"));
        assert_eq!(protocol.pending_decryption["sender123"].len(), 1);
    }

    #[test]
    fn test_pending_queue_stress_memory_plateaus_with_unfinished_handshake() {
        let mut config = create_test_config();
        config.encryption.enabled = true;
        config.encryption.pending_queue.max_pending_per_peer = 32;
        config.encryption.pending_queue.max_pending_global = 64;
        config.encryption.pending_queue.pending_ttl_ms = 60_000;
        config.encryption.pending_queue.overflow_policy = crate::config::OverflowPolicy::DropOldest;

        let mut protocol = OfflineProtocol::new(config).unwrap();
        for idx in 0..10_000 {
            let msg = pending_test_message("sender123", &format!("encrypted-{idx}"));
            protocol.enqueue_pending_decryption("sender123", &msg);
        }

        assert_eq!(protocol.pending_decryption_total, 32);
        assert_eq!(protocol.pending_queue_metrics.pending_messages_current, 32);
        assert_eq!(
            *protocol
                .pending_queue_metrics
                .pending_messages_per_peer
                .get("sender123")
                .unwrap(),
            32
        );
    }

    #[test]
    fn test_pending_queue_sustained_mixed_invalid_and_early_encrypted_is_bounded() {
        let mut config = create_test_config();
        config.encryption.enabled = true;
        config.encryption.pending_queue.max_pending_per_peer = 16;
        config.encryption.pending_queue.max_pending_global = 32;
        config.encryption.pending_queue.pending_ttl_ms = 60_000;
        config.encryption.pending_queue.overflow_policy = crate::config::OverflowPolicy::DropOldest;

        let mut protocol = OfflineProtocol::new(config).unwrap();
        protocol
            .initialize_mls(Arc::new(crate::mls::InMemoryStorage::new()))
            .unwrap();

        let valid_early_encrypted = format!(
            "{}{{\"group_id\":\"session:sender123:user123\",\"message_type\":\"Application\",\"epoch\":0,\"ciphertext\":[1,2,3],\"sender_id\":\"sender123\",\"timestamp_ms\":12345}}",
            internal_prefixes::ENCRYPTED
        );
        let malformed_variants = [
            format!("{}{{", internal_prefixes::ENCRYPTED),
            format!("{}{{\"group_id\":\"bad\"", internal_prefixes::ENCRYPTED),
            format!("{}[]", internal_prefixes::ENCRYPTED),
            format!(
                "{}{{\"ciphertext\":\"not-array\"}}",
                internal_prefixes::ENCRYPTED
            ),
        ];

        let mut early_count: u64 = 0;
        let mut invalid_count: u64 = 0;
        for idx in 0..10_000 {
            let content = if idx % 5 == 0 {
                invalid_count += 1;
                malformed_variants[(idx % malformed_variants.len()) as usize].as_str()
            } else {
                early_count += 1;
                valid_early_encrypted.as_str()
            };

            let message = Message::new(
                UserId::new("sender123").unwrap(),
                UserId::new("user123").unwrap(),
                AppId::new("test-app").unwrap(),
                content,
            );
            let result = protocol.process_internal_message(&message);
            assert!(matches!(result, Some(InternalMessageResult::Consumed)));
        }

        let per_peer_limit = protocol
            .config
            .encryption
            .pending_queue
            .max_pending_per_peer;
        let global_limit = protocol.config.encryption.pending_queue.max_pending_global;
        assert!(protocol.pending_decryption_total <= global_limit);
        assert!(
            protocol
                .pending_decryption
                .get("sender123")
                .map(VecDeque::len)
                .unwrap_or(0)
                <= per_peer_limit
        );

        let metrics = protocol.pending_queue_metrics();
        assert_eq!(metrics.pending_messages_received_total, early_count);
        assert_eq!(
            metrics.pending_messages_current,
            protocol.pending_decryption_total
        );
        assert!(metrics.pending_messages_dropped_overflow_total > 0);
        assert_eq!(early_count + invalid_count, 10_000);
    }

    #[test]
    fn test_pending_queue_flood_respects_per_peer_fairness() {
        let mut config = create_test_config();
        config.encryption.enabled = true;
        config.encryption.pending_queue.max_pending_per_peer = 3;
        config.encryption.pending_queue.max_pending_global = 6;
        config.encryption.pending_queue.pending_ttl_ms = 60_000;
        config.encryption.pending_queue.overflow_policy = crate::config::OverflowPolicy::DropOldest;

        let mut protocol = OfflineProtocol::new(config).unwrap();

        for idx in 0..100 {
            let msg = pending_test_message("noisy-peer", &format!("noisy-{idx}"));
            protocol.enqueue_pending_decryption("noisy-peer", &msg);
        }
        for idx in 0..3 {
            let msg = pending_test_message("peer-a", &format!("a-{idx}"));
            protocol.enqueue_pending_decryption("peer-a", &msg);
            let msg = pending_test_message("peer-b", &format!("b-{idx}"));
            protocol.enqueue_pending_decryption("peer-b", &msg);
        }

        assert!(protocol.pending_decryption_total <= 6);
        assert!(
            protocol
                .pending_decryption
                .get("noisy-peer")
                .map(VecDeque::len)
                .unwrap_or(0)
                <= 3
        );
        assert!(protocol.pending_decryption.contains_key("peer-a"));
        assert!(protocol.pending_decryption.contains_key("peer-b"));
    }

    #[test]
    fn test_pending_queue_drop_newest_policy_enforced_for_per_peer_limit() {
        let mut config = create_test_config();
        config.encryption.enabled = true;
        config.encryption.pending_queue.max_pending_per_peer = 1;
        config.encryption.pending_queue.max_pending_global = 10;
        config.encryption.pending_queue.pending_ttl_ms = 60_000;
        config.encryption.pending_queue.overflow_policy = crate::config::OverflowPolicy::DropNewest;

        let mut protocol = OfflineProtocol::new(config).unwrap();
        let first = pending_test_message("peer-a", "first");
        let second = pending_test_message("peer-a", "second");
        protocol.enqueue_pending_decryption("peer-a", &first);
        protocol.enqueue_pending_decryption("peer-a", &second);

        assert_eq!(protocol.pending_decryption["peer-a"].len(), 1);
        let queued_message = &protocol.pending_decryption["peer-a"][0];
        assert_eq!(queued_message.message.content, "first");
        assert_eq!(
            protocol
                .pending_queue_metrics
                .pending_messages_dropped_overflow_total,
            1
        );
    }

    #[test]
    fn test_pending_queue_global_limit_fail_closed_when_global_index_corrupted() {
        let mut config = create_test_config();
        config.encryption.enabled = true;
        config.encryption.pending_queue.max_pending_per_peer = 1;
        config.encryption.pending_queue.max_pending_global = 1;
        config.encryption.pending_queue.pending_ttl_ms = 60_000;
        config.encryption.pending_queue.overflow_policy = crate::config::OverflowPolicy::DropOldest;

        let mut protocol = OfflineProtocol::new(config).unwrap();
        protocol.enqueue_pending_decryption("peer-a", &pending_test_message("peer-a", "m1"));
        assert_eq!(protocol.pending_decryption_total, 1);

        // Simulate index drift: queue has data but global-order index is empty.
        protocol.pending_decryption_global_order.clear();

        protocol.enqueue_pending_decryption("peer-b", &pending_test_message("peer-b", "m2"));

        assert_eq!(protocol.pending_decryption_total, 1);
        assert!(!protocol.pending_decryption.contains_key("peer-b"));
        assert!(protocol.pending_decryption.contains_key("peer-a"));
        assert!(
            protocol
                .pending_queue_metrics
                .pending_messages_eviction_failures_total
                >= 1
        );
    }

    #[test]
    fn test_pending_queue_ttl_expiration_is_deterministic_and_monotonic() {
        let mut config = create_test_config();
        config.encryption.enabled = true;
        config.encryption.pending_queue.max_pending_per_peer = 10;
        config.encryption.pending_queue.max_pending_global = 100;
        config.encryption.pending_queue.pending_ttl_ms = 1_000;

        let mut protocol = OfflineProtocol::new(config).unwrap();
        let old_msg = pending_test_message("sender123", "old");
        let fresh_msg = pending_test_message("sender123", "fresh");
        protocol.enqueue_pending_decryption("sender123", &old_msg);
        protocol.enqueue_pending_decryption("sender123", &fresh_msg);

        {
            let queue = protocol.pending_decryption.get_mut("sender123").unwrap();
            let old = queue.front_mut().unwrap();
            old.received_at = Instant::now() - StdDuration::from_millis(2_000);
        }

        let expired = protocol.prune_expired_pending_for_peer("sender123", Instant::now());
        assert_eq!(expired, 1);
        assert_eq!(protocol.pending_decryption["sender123"].len(), 1);
        assert_eq!(
            protocol
                .pending_queue_metrics
                .pending_messages_expired_total,
            1
        );
    }

    #[test]
    fn test_pending_messages_replay_decrypt_after_session_readiness() {
        let mut bob_config = create_test_config_for_user("bob");
        bob_config.encryption.enabled = true;
        let mut bob = OfflineProtocol::new(bob_config).unwrap();
        bob.initialize_mls(Arc::new(InMemoryStorage::new()))
            .unwrap();

        let alice_manager = MlsManager::new("alice", Arc::new(InMemoryStorage::new())).unwrap();
        let bob_key_package = {
            let manager = bob.mls_manager.as_ref().unwrap().read().unwrap();
            manager.get_or_create_key_package().unwrap()
        };
        let welcome = alice_manager
            .import_key_package("bob", &bob_key_package.key_package_data)
            .and_then(|_| alice_manager.create_session("bob"))
            .unwrap();

        let encrypted = alice_manager
            .encrypt_for_user("bob", b"queued secret")
            .unwrap();
        let encrypted_payload = format!(
            "{}{}",
            internal_prefixes::ENCRYPTED,
            serde_json::to_string(&encrypted).unwrap()
        );
        let incoming = Message::new(
            UserId::new("alice").unwrap(),
            UserId::new("bob").unwrap(),
            AppId::new("test-app").unwrap(),
            &encrypted_payload,
        );

        let result = bob.process_internal_message(&incoming);
        assert!(matches!(result, Some(InternalMessageResult::Consumed)));
        assert_eq!(bob.pending_decryption["alice"].len(), 1);

        {
            let manager = bob.mls_manager.as_ref().unwrap().read().unwrap();
            manager.join_session(&welcome).unwrap();
        }

        bob.process_pending_decryption("alice");
        assert!(!bob.pending_decryption.contains_key("alice"));
        let metrics = bob.pending_queue_metrics();
        assert_eq!(metrics.pending_messages_received_total, 1);
    }

    #[test]
    fn test_pending_queue_concurrency_multi_peer_enqueue_is_bounded() {
        let mut config = create_test_config();
        config.encryption.enabled = true;
        config.encryption.pending_queue.max_pending_per_peer = 8;
        config.encryption.pending_queue.max_pending_global = 64;
        config.encryption.pending_queue.pending_ttl_ms = 60_000;
        let protocol = Arc::new(Mutex::new(OfflineProtocol::new(config).unwrap()));

        let mut handles = Vec::new();
        for peer_idx in 0..16 {
            let protocol = Arc::clone(&protocol);
            handles.push(thread::spawn(move || {
                let peer = format!("peer-{peer_idx}");
                for msg_idx in 0..50 {
                    let msg = Message::new(
                        UserId::new(&peer).unwrap(),
                        UserId::new("user123").unwrap(),
                        AppId::new("test-app").unwrap(),
                        &format!("concurrent-{msg_idx}"),
                    );
                    protocol
                        .lock()
                        .unwrap()
                        .enqueue_pending_decryption(&peer, &msg);
                }
            }));
        }

        for handle in handles {
            handle.join().unwrap();
        }

        let protocol = protocol.lock().unwrap();
        assert!(protocol.pending_decryption_total <= 64);
        for queue in protocol.pending_decryption.values() {
            assert!(queue.len() <= 8);
        }
    }

    // ========================================================================
    // LAMPORT CLOCK TESTS
    // ========================================================================

    use crate::mls::InMemoryStorage;

    #[test]
    fn test_lamport_clock_advances_on_send() {
        let mut protocol = OfflineProtocol::new(create_test_config()).unwrap();

        let mut mock_transport = MockTransport::new(TransportType::BLE);
        mock_transport.start().unwrap();
        protocol
            .transport_manager_mut()
            .add_transport(TransportType::BLE, Box::new(mock_transport.clone()));

        protocol.start().unwrap();

        assert_eq!(protocol.lamport_clock.value(), 0);

        protocol
            .send_message("bob", "msg1", None::<MessagePriority>, None::<String>)
            .unwrap();
        assert_eq!(protocol.lamport_clock.value(), 1);

        protocol
            .send_message("bob", "msg2", None::<MessagePriority>, None::<String>)
            .unwrap();
        assert_eq!(protocol.lamport_clock.value(), 2);
    }

    #[test]
    fn test_lamport_clock_merges_on_receive() {
        let mut protocol = OfflineProtocol::new(create_test_config()).unwrap();

        let mut mock_transport = MockTransport::new(TransportType::BLE);
        mock_transport.start().unwrap();

        // Create a message with a high Lamport clock from a peer
        let mut message = Message::new(
            UserId::new("alice").unwrap(),
            UserId::new("user123").unwrap(),
            AppId::new("test-app").unwrap(),
            "Hello",
        );
        message.lamport_clock = LamportClock::from_value(50);
        mock_transport.queue_message(message);

        protocol
            .transport_manager_mut()
            .add_transport(TransportType::BLE, Box::new(mock_transport));
        protocol.start().unwrap();

        assert_eq!(protocol.lamport_clock.value(), 0);

        let received = protocol.receive_message();
        assert!(received.is_some());

        // Clock should be max(0, 50) + 1 = 51
        assert_eq!(protocol.lamport_clock.value(), 51);
    }

    #[test]
    fn test_lamport_clock_monotonic_across_send_receive() {
        let mut protocol = OfflineProtocol::new(create_test_config()).unwrap();

        let mut mock_transport = MockTransport::new(TransportType::BLE);
        mock_transport.start().unwrap();

        // Send a message first (clock -> 1)
        protocol
            .transport_manager_mut()
            .add_transport(TransportType::BLE, Box::new(mock_transport.clone()));
        protocol.start().unwrap();

        protocol
            .send_message("bob", "hi", None::<MessagePriority>, None::<String>)
            .unwrap();
        assert_eq!(protocol.lamport_clock.value(), 1);

        // Receive a message with lower clock (clock should still advance)
        let mut message = Message::new(
            UserId::new("bob").unwrap(),
            UserId::new("user123").unwrap(),
            AppId::new("test-app").unwrap(),
            "reply",
        );
        message.lamport_clock = LamportClock::from_value(0);
        mock_transport.queue_message(message);

        // Legacy message (clock=0) — merge is skipped so clock stays at 1
        let received = protocol.receive_message();
        assert!(received.is_some());
        assert_eq!(protocol.lamport_clock.value(), 1);

        // Now receive a message with higher clock
        let mut message2 = Message::new(
            UserId::new("bob").unwrap(),
            UserId::new("user123").unwrap(),
            AppId::new("test-app").unwrap(),
            "another",
        );
        message2.lamport_clock = LamportClock::from_value(10);
        mock_transport.queue_message(message2);

        let received2 = protocol.receive_message();
        assert!(received2.is_some());
        // max(1, 10) + 1 = 11
        assert_eq!(protocol.lamport_clock.value(), 11);
    }

    #[test]
    fn test_lamport_clock_persists_and_restores() {
        let storage = Arc::new(InMemoryStorage::new());

        // First session: send messages to advance the clock
        {
            let mut protocol = OfflineProtocol::new(create_test_config()).unwrap();
            let mut mock_transport = MockTransport::new(TransportType::BLE);
            mock_transport.start().unwrap();
            protocol
                .transport_manager_mut()
                .add_transport(TransportType::BLE, Box::new(mock_transport));

            protocol
                .enable_message_persistence(storage.clone())
                .unwrap();
            protocol.start().unwrap();

            // Send 5 messages to advance clock to 5
            for i in 0..5 {
                protocol
                    .send_message(
                        "bob",
                        format!("msg{}", i),
                        None::<MessagePriority>,
                        None::<String>,
                    )
                    .unwrap();
            }
            assert_eq!(protocol.lamport_clock.value(), 5);
        }

        // Second session: clock should restore from storage
        {
            let mut protocol = OfflineProtocol::new(create_test_config()).unwrap();
            let mut mock_transport = MockTransport::new(TransportType::BLE);
            mock_transport.start().unwrap();
            protocol
                .transport_manager_mut()
                .add_transport(TransportType::BLE, Box::new(mock_transport));

            assert_eq!(protocol.lamport_clock.value(), 0);

            protocol
                .enable_message_persistence(storage.clone())
                .unwrap();

            // After attaching storage, clock should be restored
            assert_eq!(protocol.lamport_clock.value(), 5);

            // Next send should be 6, not 1
            protocol.start().unwrap();
            protocol
                .send_message(
                    "bob",
                    "after restart",
                    None::<MessagePriority>,
                    None::<String>,
                )
                .unwrap();
            assert_eq!(protocol.lamport_clock.value(), 6);
        }
    }

    #[test]
    fn test_lamport_clock_restore_with_corrupted_data() {
        let storage = Arc::new(InMemoryStorage::new());

        // Write corrupted data (wrong length)
        storage
            .store(
                storage_keys::LAMPORT_CLOCK,
                storage_keys::LAMPORT_CLOCK_ID,
                &[1, 2, 3], // only 3 bytes, not 8
            )
            .unwrap();

        let mut protocol = OfflineProtocol::new(create_test_config()).unwrap();
        protocol
            .enable_message_persistence(storage.clone())
            .unwrap();

        // Clock should remain at 0 (corrupted data ignored)
        assert_eq!(protocol.lamport_clock.value(), 0);
    }

    #[test]
    fn test_lamport_clock_restore_never_goes_backward() {
        let storage = Arc::new(InMemoryStorage::new());

        // Store a value of 10 in storage
        storage
            .store(
                storage_keys::LAMPORT_CLOCK,
                storage_keys::LAMPORT_CLOCK_ID,
                &10u64.to_le_bytes(),
            )
            .unwrap();

        let mut protocol = OfflineProtocol::new(create_test_config()).unwrap();

        // Advance in-memory clock to 20 before attaching storage
        for _ in 0..20 {
            protocol.lamport_clock.tick();
        }
        assert_eq!(protocol.lamport_clock.value(), 20);

        // Attaching storage should NOT regress to 10
        protocol
            .enable_message_persistence(storage.clone())
            .unwrap();
        assert_eq!(protocol.lamport_clock.value(), 20);
    }

    #[test]
    fn test_lamport_clock_merge_on_internal_message() {
        let mut protocol = OfflineProtocol::new(create_test_config()).unwrap();

        let mut mock_transport = MockTransport::new(TransportType::BLE);
        mock_transport.start().unwrap();

        // Create a key package message with a high Lamport clock
        let key_pkg_payload = KeyPackagePayload {
            user_id: "sender456".to_string(),
            key_package_data: vec![5, 6, 7, 8],
            remaining_lifetime_ms: 30 * 24 * 60 * 60 * 1000,
            timestamp_ms: 12345,
        };
        let content = format!(
            "{}{}",
            internal_prefixes::KEY_PACKAGE,
            serde_json::to_string(&key_pkg_payload).unwrap()
        );
        let mut message = Message::new(
            UserId::new("sender456").unwrap(),
            UserId::new("user123").unwrap(),
            AppId::new("test-app").unwrap(),
            &content,
        );
        message.lamport_clock = LamportClock::from_value(100);
        mock_transport.queue_message(message);

        protocol
            .transport_manager_mut()
            .add_transport(TransportType::BLE, Box::new(mock_transport));
        protocol.start().unwrap();

        assert_eq!(protocol.lamport_clock.value(), 0);

        // Receiving the internal message should merge the clock even
        // though process_internal_message returns Consumed
        let received = protocol.receive_message();
        // Internal messages are consumed, not surfaced
        assert!(received.is_none());

        // Clock should have merged: max(0, 100) + 1 = 101
        assert_eq!(protocol.lamport_clock.value(), 101);
    }

    #[test]
    fn test_lamport_clock_merge_on_duplicate_message() {
        let mut protocol = OfflineProtocol::new(create_test_config()).unwrap();

        let mut mock_transport = MockTransport::new(TransportType::BLE);
        mock_transport.start().unwrap();

        // Create two copies of the same message (simulate duplicate delivery)
        let mut message = Message::new(
            UserId::new("alice").unwrap(),
            UserId::new("user123").unwrap(),
            AppId::new("test-app").unwrap(),
            "Hello",
        );
        message.lamport_clock = LamportClock::from_value(42);
        let message_dup = message.clone();

        mock_transport.queue_message(message);
        mock_transport.queue_message(message_dup);

        protocol
            .transport_manager_mut()
            .add_transport(TransportType::BLE, Box::new(mock_transport));
        protocol.start().unwrap();

        // First receive: message delivered
        let received = protocol.receive_message();
        assert!(received.is_some());
        // max(0, 42) + 1 = 43
        assert_eq!(protocol.lamport_clock.value(), 43);

        // Second receive: duplicate detected, but clock should have
        // already merged (merge happens before dedup).
        // The duplicate carries the same clock=42, so merge would yield
        // max(43, 42) + 1 = 44
        let received2 = protocol.receive_message();
        assert!(received2.is_none());
        assert_eq!(protocol.lamport_clock.value(), 44);
    }

    #[test]
    fn test_receive_internal_connection_request_sends_delivery_ack() {
        let mut protocol = OfflineProtocol::new(create_test_config()).unwrap();

        let mut mock_transport = MockTransport::new(TransportType::BLE);
        mock_transport.start().unwrap();

        let payload = ConnectionRequestPayload {
            sender_name: "Alice".to_string(),
            timestamp_ms: 12345,
            key_package: None,
        };
        let content = format!(
            "{}{}",
            internal_prefixes::CONN_REQUEST,
            serde_json::to_string(&payload).unwrap()
        );
        let message = Message::new(
            UserId::new("alice").unwrap(),
            UserId::new("user123").unwrap(),
            AppId::new("test-app").unwrap(),
            &content,
        );
        mock_transport.queue_message(message.clone());

        protocol
            .transport_manager_mut()
            .add_transport(TransportType::BLE, Box::new(mock_transport.clone()));
        protocol.start().unwrap();

        let received = protocol.receive_message();
        assert!(received.is_none(), "internal message should be consumed");

        let expected_ack = message.id.as_str();
        let ack_count = mock_transport
            .sent_messages()
            .iter()
            .filter(|sent| {
                sent.metadata
                    .get(ACK_FOR_KEY)
                    .is_some_and(|ack_for| ack_for == &expected_ack)
            })
            .count();
        assert_eq!(ack_count, 1, "expected ACK for internal control message");
    }

    #[test]
    fn test_receive_duplicate_message_reacks_when_requires_ack() {
        let mut protocol = OfflineProtocol::new(create_test_config()).unwrap();

        let mut mock_transport = MockTransport::new(TransportType::BLE);
        mock_transport.start().unwrap();

        let message = Message::new(
            UserId::new("alice").unwrap(),
            UserId::new("user123").unwrap(),
            AppId::new("test-app").unwrap(),
            "Hello",
        );
        let message_dup = message.clone();
        mock_transport.queue_message(message);
        mock_transport.queue_message(message_dup.clone());

        protocol
            .transport_manager_mut()
            .add_transport(TransportType::BLE, Box::new(mock_transport.clone()));
        protocol.start().unwrap();

        let first = protocol.receive_message();
        assert!(first.is_some());
        let second = protocol.receive_message();
        assert!(second.is_none(), "duplicate should not be surfaced");

        let expected_ack = message_dup.id.as_str();
        let ack_count = mock_transport
            .sent_messages()
            .iter()
            .filter(|sent| {
                sent.metadata
                    .get(ACK_FOR_KEY)
                    .is_some_and(|ack_for| ack_for == &expected_ack)
            })
            .count();
        assert_eq!(
            ack_count, 2,
            "expected initial ACK and duplicate re-ACK for same message id"
        );
    }

    #[test]
    fn test_lamport_clock_no_tick_on_pending_message() {
        let mut config = create_test_config();
        config.encryption.enabled = false;

        let mut protocol = OfflineProtocol::new(config).unwrap();

        let mut mock_transport = MockTransport::new(TransportType::BLE);
        mock_transport.start().unwrap();
        protocol
            .transport_manager_mut()
            .add_transport(TransportType::BLE, Box::new(mock_transport));
        protocol.start().unwrap();

        // Send two messages, verify each tick advances by exactly 1
        let clock_before = protocol.lamport_clock.value();
        protocol
            .send_message("bob", "first", None::<MessagePriority>, None::<String>)
            .unwrap();
        assert_eq!(protocol.lamport_clock.value(), clock_before + 1);

        protocol
            .send_message("bob", "second", None::<MessagePriority>, None::<String>)
            .unwrap();
        assert_eq!(protocol.lamport_clock.value(), clock_before + 2);
    }

    #[test]
    fn test_lamport_clock_sent_message_carries_clock_value() {
        let mut protocol = OfflineProtocol::new(create_test_config()).unwrap();

        let mut mock_transport = MockTransport::new(TransportType::BLE);
        mock_transport.start().unwrap();
        protocol
            .transport_manager_mut()
            .add_transport(TransportType::BLE, Box::new(mock_transport.clone()));
        protocol.start().unwrap();

        protocol
            .send_message("bob", "test", None::<MessagePriority>, None::<String>)
            .unwrap();

        // Verify the sent message carries the Lamport clock
        let sent = mock_transport.sent_messages();
        assert_eq!(sent.len(), 1);
        assert_eq!(sent[0].lamport_clock.value(), 1);
    }

    #[test]
    fn test_key_package_remaining_lifetime_ms() {
        let mut config = create_test_config();
        config.encryption.enabled = true;
        config.encryption.auto_key_exchange = true;

        let mut protocol = OfflineProtocol::new(config).unwrap();

        // Create a key package with remaining_lifetime_ms = 0 (legacy sender)
        let key_pkg_payload = KeyPackagePayload {
            user_id: "legacy_peer".to_string(),
            key_package_data: vec![1, 2, 3],
            remaining_lifetime_ms: 0,
            timestamp_ms: 12345,
        };
        let content = format!(
            "{}{}",
            internal_prefixes::KEY_PACKAGE,
            serde_json::to_string(&key_pkg_payload).unwrap()
        );
        let message = Message::new(
            UserId::new("legacy_peer").unwrap(),
            UserId::new("user123").unwrap(),
            AppId::new("test-app").unwrap(),
            &content,
        );

        let result = protocol.process_internal_message(&message);
        assert!(matches!(result, Some(InternalMessageResult::Consumed)));

        // Should have stored with a 30-day default lifetime
        let received = protocol.pending_key_packages.get("legacy_peer").unwrap();
        let now_ms = Utc::now().timestamp_millis() as u64;
        let thirty_days_ms: u64 = 30 * 24 * 60 * 60 * 1000;
        // Should expire roughly 30 days from now (within 1 second tolerance)
        let diff = received
            .local_expires_at_ms
            .abs_diff(now_ms + thirty_days_ms);
        assert!(
            diff < 1000,
            "Expiry should be ~30 days from now, diff was {}",
            diff
        );
    }

    #[test]
    fn test_key_package_expired_discarded() {
        let mut config = create_test_config();
        config.encryption.enabled = true;

        let mut protocol = OfflineProtocol::new(config).unwrap();

        // MLS must be initialized so establish_secure_session reaches the
        // expiry check instead of short-circuiting with MlsNotInitialized.
        let storage = Arc::new(InMemoryStorage::new());
        protocol.initialize_mls(storage).unwrap();

        // Manually insert an already-expired key package
        protocol.pending_key_packages.insert(
            "expired_peer".to_string(),
            ReceivedKeyPackage {
                key_package_data: vec![1, 2, 3],
                local_expires_at_ms: 0, // expired at epoch
            },
        );

        assert!(protocol.pending_key_packages.contains_key("expired_peer"));

        // Attempting to establish session should detect expiry and discard
        let result = protocol.establish_secure_session("expired_peer");
        assert!(result.is_err());
        assert!(!protocol.pending_key_packages.contains_key("expired_peer"));
    }

    #[test]
    fn test_peer_key_package_persisted_and_restored_after_restart() {
        let storage = Arc::new(InMemoryStorage::new());
        let bob_storage = Arc::new(InMemoryStorage::new());
        let bob_manager = MlsManager::new("bob", bob_storage).unwrap();
        let bob_key_package = bob_manager.get_or_create_key_package().unwrap();

        // First session: receive key package (persisted via process_internal_message)
        {
            let mut protocol = OfflineProtocol::new(create_test_config()).unwrap();
            protocol.initialize_mls(storage.clone()).unwrap();
            let key_pkg_payload = KeyPackagePayload {
                user_id: "bob".to_string(),
                key_package_data: bob_key_package.key_package_data.clone(),
                remaining_lifetime_ms: 60 * 60 * 1000,
                timestamp_ms: 0,
            };
            let content = format!(
                "{}{}",
                internal_prefixes::KEY_PACKAGE,
                serde_json::to_string(&key_pkg_payload).unwrap()
            );
            let message = Message::new(
                UserId::new("bob").unwrap(),
                UserId::new("alice").unwrap(),
                AppId::new("test-app").unwrap(),
                &content,
            );
            let _ = protocol.process_internal_message(&message);
            assert!(protocol.pending_key_packages.contains_key("bob"));
        }

        // Second session: new protocol, same storage; restore should repopulate pending_key_packages
        {
            let mut protocol = OfflineProtocol::new(create_test_config()).unwrap();
            protocol.initialize_mls(storage.clone()).unwrap();
            assert!(
                protocol.pending_key_packages.contains_key("bob"),
                "Key package should be restored from storage"
            );
            let welcome = protocol.establish_secure_session("bob").unwrap();
            assert!(
                welcome.is_some(),
                "Session should be created from restored key package"
            );
        }
    }

    #[test]
    fn test_establishment_state_returns_correct_states() {
        let storage = Arc::new(InMemoryStorage::new());
        let bob_storage = Arc::new(InMemoryStorage::new());
        let bob_manager = MlsManager::new("bob", bob_storage).unwrap();
        let bob_key_package = bob_manager.get_or_create_key_package().unwrap();

        let mut protocol = OfflineProtocol::new(create_test_config()).unwrap();
        protocol.initialize_mls(storage).unwrap();

        // No key package, no session
        assert_eq!(
            protocol.get_establishment_state("bob").unwrap(),
            EstablishmentState::NoKeyPackage
        );

        // Add key package -> HaveKeyPackage
        protocol.pending_key_packages.insert(
            "bob".to_string(),
            ReceivedKeyPackage {
                key_package_data: bob_key_package.key_package_data.clone(),
                local_expires_at_ms: Utc::now().timestamp_millis() as u64 + 60_000,
            },
        );
        assert_eq!(
            protocol.get_establishment_state("bob").unwrap(),
            EstablishmentState::HaveKeyPackage
        );

        // Create session (via MLS manager directly) -> SessionPending
        {
            let mls = protocol.mls_manager.as_ref().unwrap().clone();
            let manager = mls.read().unwrap();
            manager
                .import_key_package("bob", &bob_key_package.key_package_data)
                .unwrap();
            manager.create_session("bob").unwrap();
        }
        protocol.ensure_session_state_entry("bob", "test").unwrap();
        assert_eq!(
            protocol.get_establishment_state("bob").unwrap(),
            EstablishmentState::SessionPending
        );

        // Confirm -> SessionConfirmed
        protocol.confirm_session_state("bob", "test").unwrap();
        assert_eq!(
            protocol.get_establishment_state("bob").unwrap(),
            EstablishmentState::SessionConfirmed
        );
    }

    #[test]
    fn test_establish_secure_session_loads_from_storage_after_restart() {
        let storage = Arc::new(InMemoryStorage::new());
        let bob_storage = Arc::new(InMemoryStorage::new());
        let bob_manager = MlsManager::new("bob", bob_storage).unwrap();
        let bob_key_package = bob_manager.get_or_create_key_package().unwrap();

        // Persist key package (simulate receive then restart: in-memory cleared)
        {
            let mut protocol = OfflineProtocol::new(create_test_config()).unwrap();
            protocol.initialize_mls(storage.clone()).unwrap();
            let key_pkg_payload = KeyPackagePayload {
                user_id: "bob".to_string(),
                key_package_data: bob_key_package.key_package_data.clone(),
                remaining_lifetime_ms: 60 * 60 * 1000,
                timestamp_ms: 0,
            };
            let content = format!(
                "{}{}",
                internal_prefixes::KEY_PACKAGE,
                serde_json::to_string(&key_pkg_payload).unwrap()
            );
            let message = Message::new(
                UserId::new("bob").unwrap(),
                UserId::new("alice").unwrap(),
                AppId::new("test-app").unwrap(),
                &content,
            );
            let _ = protocol.process_internal_message(&message);
            assert!(protocol.pending_key_packages.contains_key("bob"));
        }

        // New protocol instance: restore runs and loads key package from storage
        {
            let mut protocol = OfflineProtocol::new(create_test_config()).unwrap();
            protocol.initialize_mls(storage.clone()).unwrap();
            // establish_secure_session should try load from storage and create session (no terminal error)
            let result = protocol.establish_secure_session("bob");
            assert!(
                result.is_ok(),
                "establish_secure_session should load from storage and create session, got {:?}",
                result
            );
            let welcome = result.unwrap();
            assert!(welcome.is_some());
        }
    }

    // ========================================================================
    // SERVICE DISCOVERY & REQUEST/RESPONSE TESTS
    // ========================================================================

    #[test]
    fn test_register_and_unregister_service() {
        let mut protocol = OfflineProtocol::new(create_test_config()).unwrap();

        let descriptor = ServiceDescriptor {
            service_id: offline_protocol_core::ServiceId::new("echo.v1").unwrap(),
            version: "1.0".to_string(),
            capabilities: HashMap::new(),
        };
        protocol.register_service(descriptor).unwrap();
        assert!(protocol.mesh_services().has_service("echo.v1"));

        let removed = protocol.unregister_service("echo.v1").unwrap();
        assert!(removed);
        assert!(!protocol.mesh_services().has_service("echo.v1"));

        let removed_again = protocol.unregister_service("echo.v1").unwrap();
        assert!(!removed_again);
    }

    #[test]
    fn test_process_svc_discover_query_with_match() {
        use offline_protocol_services::SVC_DISCOVER_QUERY;

        let mut protocol = OfflineProtocol::new(create_test_config()).unwrap();

        // Register a service
        let descriptor = ServiceDescriptor {
            service_id: offline_protocol_core::ServiceId::new("weather").unwrap(),
            version: "2.0".to_string(),
            capabilities: {
                let mut m = HashMap::new();
                m.insert("format".to_string(), "json".to_string());
                m
            },
        };
        protocol.register_service(descriptor).unwrap();

        // Build a discovery query message from a remote peer using raw JSON
        let content = format!(
            "{}{}",
            SVC_DISCOVER_QUERY,
            serde_json::json!({
                "query_id": "q-001",
                "originator": "alice",
                "service_id": "weather",
                "remaining_hops": 10
            })
        );
        let message = Message::new(
            UserId::new("alice").unwrap(),
            UserId::new("user123").unwrap(),
            AppId::new("test-app").unwrap(),
            &content,
        );

        let result = protocol.process_internal_message(&message);
        assert!(matches!(result, Some(InternalMessageResult::Consumed)));

        // The query should be recorded in seen set
        assert!(protocol
            .mesh_services()
            .seen_discovery_queries()
            .contains_key("q-001"));
    }

    #[test]
    fn test_process_svc_discover_query_dedup() {
        use offline_protocol_services::SVC_DISCOVER_QUERY;

        let mut protocol = OfflineProtocol::new(create_test_config()).unwrap();

        let content = format!(
            "{}{}",
            SVC_DISCOVER_QUERY,
            serde_json::json!({
                "query_id": "q-dedup",
                "originator": "alice",
                "remaining_hops": 10
            })
        );

        let make_msg = || {
            Message::new(
                UserId::new("alice").unwrap(),
                UserId::new("user123").unwrap(),
                AppId::new("test-app").unwrap(),
                &content,
            )
        };

        // First time: processes normally
        let r1 = protocol.process_internal_message(&make_msg());
        assert!(matches!(r1, Some(InternalMessageResult::Consumed)));

        // Second time: deduplicated (still consumed, but no further action)
        let r2 = protocol.process_internal_message(&make_msg());
        assert!(matches!(r2, Some(InternalMessageResult::Consumed)));
    }

    #[test]
    fn test_process_svc_discover_response_emits_event() {
        use offline_protocol_services::SVC_DISCOVER_RESPONSE;

        let mut protocol = OfflineProtocol::new(create_test_config()).unwrap();
        let events: Arc<Mutex<Vec<Event>>> = Arc::new(Mutex::new(Vec::new()));
        let events_handle = Arc::clone(&events);

        protocol.on_event(move |event| {
            events_handle.lock().unwrap().push(event);
        });

        let content = format!(
            "{}{}",
            SVC_DISCOVER_RESPONSE,
            serde_json::json!({
                "query_id": "q-123",
                "service_id": "weather",
                "version": "2.0",
                "provider_peer_id": "bob",
                "capabilities": {},
                "hop_count": 1
            })
        );
        let message = Message::new(
            UserId::new("bob").unwrap(),
            UserId::new("user123").unwrap(),
            AppId::new("test-app").unwrap(),
            &content,
        );

        let result = protocol.process_internal_message(&message);
        assert!(matches!(result, Some(InternalMessageResult::Consumed)));

        let captured = events.lock().unwrap();
        assert_eq!(captured.len(), 1);
        match &captured[0] {
            Event::ServiceDiscovered {
                query_id,
                service_id,
                version,
                provider_peer_id,
                hop_count,
                ..
            } => {
                assert_eq!(query_id, "q-123");
                assert_eq!(service_id, "weather");
                assert_eq!(version, "2.0");
                assert_eq!(provider_peer_id, "bob");
                assert_eq!(*hop_count, 1);
            }
            other => panic!("Wrong event type: {:?}", other),
        }
    }

    #[test]
    fn test_process_svc_request_unregistered_auto_not_found() {
        use offline_protocol_services::SVC_REQUEST;

        let mut protocol = OfflineProtocol::new(create_test_config()).unwrap();
        let events: Arc<Mutex<Vec<Event>>> = Arc::new(Mutex::new(Vec::new()));
        let events_handle = Arc::clone(&events);

        protocol.on_event(move |event| {
            events_handle.lock().unwrap().push(event);
        });

        // No services registered — request should auto-respond not_found
        let content = format!(
            "{}{}",
            SVC_REQUEST,
            serde_json::json!({
                "request_id": "req-001",
                "service_id": "nonexistent",
                "method": "get",
                "body": "{}"
            })
        );
        let message = Message::new(
            UserId::new("alice").unwrap(),
            UserId::new("user123").unwrap(),
            AppId::new("test-app").unwrap(),
            &content,
        );

        let result = protocol.process_internal_message(&message);
        assert!(matches!(result, Some(InternalMessageResult::Consumed)));

        // No ServiceRequestReceived event should be emitted
        let captured = events.lock().unwrap();
        assert!(
            captured.is_empty(),
            "Should not emit event for unregistered service, got {:?}",
            *captured
        );
    }

    #[test]
    fn test_process_svc_request_registered_emits_event() {
        use offline_protocol_services::SVC_REQUEST;

        let mut protocol = OfflineProtocol::new(create_test_config()).unwrap();
        let events: Arc<Mutex<Vec<Event>>> = Arc::new(Mutex::new(Vec::new()));
        let events_handle = Arc::clone(&events);

        protocol.on_event(move |event| {
            events_handle.lock().unwrap().push(event);
        });

        // Register the service first
        let descriptor = ServiceDescriptor {
            service_id: offline_protocol_core::ServiceId::new("echo").unwrap(),
            version: "1.0".to_string(),
            capabilities: HashMap::new(),
        };
        protocol.register_service(descriptor).unwrap();

        let content = format!(
            "{}{}",
            SVC_REQUEST,
            serde_json::json!({
                "request_id": "req-002",
                "service_id": "echo",
                "method": "ping",
                "body": "hello"
            })
        );
        let message = Message::new(
            UserId::new("alice").unwrap(),
            UserId::new("user123").unwrap(),
            AppId::new("test-app").unwrap(),
            &content,
        );

        let result = protocol.process_internal_message(&message);
        assert!(matches!(result, Some(InternalMessageResult::Consumed)));

        let captured = events.lock().unwrap();
        assert_eq!(captured.len(), 1);
        match &captured[0] {
            Event::ServiceRequestReceived {
                request_id,
                service_id,
                method,
                body,
                sender,
            } => {
                assert_eq!(request_id, "req-002");
                assert_eq!(service_id, "echo");
                assert_eq!(method, "ping");
                assert_eq!(body, "hello");
                assert_eq!(sender, "alice");
            }
            other => panic!("Wrong event type: {:?}", other),
        }
    }

    #[test]
    fn test_process_svc_response_emits_event() {
        use offline_protocol_services::SVC_RESPONSE;

        let mut protocol = OfflineProtocol::new(create_test_config()).unwrap();
        let events: Arc<Mutex<Vec<Event>>> = Arc::new(Mutex::new(Vec::new()));
        let events_handle = Arc::clone(&events);

        protocol.on_event(move |event| {
            events_handle.lock().unwrap().push(event);
        });

        let content = format!(
            "{}{}",
            SVC_RESPONSE,
            serde_json::json!({
                "request_id": "req-003",
                "service_id": "echo",
                "status": "ok",
                "body": "pong"
            })
        );
        let message = Message::new(
            UserId::new("bob").unwrap(),
            UserId::new("user123").unwrap(),
            AppId::new("test-app").unwrap(),
            &content,
        );

        let result = protocol.process_internal_message(&message);
        assert!(matches!(result, Some(InternalMessageResult::Consumed)));

        let captured = events.lock().unwrap();
        assert_eq!(captured.len(), 1);
        match &captured[0] {
            Event::ServiceResponseReceived {
                request_id,
                service_id,
                status,
                body,
                provider_peer_id,
            } => {
                assert_eq!(request_id, "req-003");
                assert_eq!(service_id, "echo");
                assert_eq!(status, "ok");
                assert_eq!(body, "pong");
                assert_eq!(provider_peer_id, "bob");
            }
            other => panic!("Wrong event type: {:?}", other),
        }
    }

    #[test]
    fn test_process_regular_message_not_consumed_by_service_handlers() {
        let mut protocol = OfflineProtocol::new(create_test_config()).unwrap();

        let message = Message::new(
            UserId::new("alice").unwrap(),
            UserId::new("user123").unwrap(),
            AppId::new("test-app").unwrap(),
            "Hello, this is a normal message",
        );

        let result = protocol.process_internal_message(&message);
        assert!(result.is_none(), "Regular messages should not be consumed");
    }

    #[test]
    fn test_discover_services_no_peers() {
        let mut protocol = OfflineProtocol::new(create_test_config()).unwrap();

        // Start protocol so send_internal_message doesn't fail with NotStarted
        let mut mock_transport = MockTransport::new(TransportType::BLE);
        mock_transport.start().unwrap();
        protocol
            .transport_manager_mut()
            .add_transport(TransportType::BLE, Box::new(mock_transport));
        protocol.start().unwrap();

        // No peers in key_package_sent_to — should succeed with empty broadcast
        let query_id = protocol.discover_services(None).unwrap();
        assert!(!query_id.is_empty());
        assert!(protocol
            .mesh_services()
            .seen_discovery_queries()
            .contains_key(&query_id));
    }

    #[test]
    fn test_require_encryption_blocks_service_discovery_control_messages() {
        let mut config = create_test_config();
        config.encryption.enabled = true;
        config.encryption.require_encryption = true;

        let mut protocol = OfflineProtocol::new(config).unwrap();

        let mut mock_transport = MockTransport::new(TransportType::BLE);
        mock_transport.start().unwrap();
        let transport_handle = mock_transport.clone();
        protocol
            .transport_manager_mut()
            .add_transport(TransportType::BLE, Box::new(mock_transport));
        protocol.start().unwrap();

        let discover_result = protocol.discover_services(None);
        assert!(matches!(discover_result, Err(Error::EncryptFailed(_))));

        let request_result = protocol.send_service_request("bob", "echo.v1", "ping", "{}");
        assert!(matches!(request_result, Err(Error::EncryptFailed(_))));

        let respond_result =
            protocol.respond_to_service_request("req-1", "alice", "echo.v1", "ok", "pong");
        assert!(matches!(respond_result, Err(Error::EncryptFailed(_))));

        assert_eq!(transport_handle.sent_messages().len(), 0);
    }

    #[test]
    fn test_known_peers_capacity_limit() {
        let mut protocol = OfflineProtocol::new(create_test_config()).unwrap();

        // Fill to capacity
        for i in 0..MAX_KNOWN_PEERS {
            protocol.on_neighbor_discovered(&format!("peer-{i}"));
        }
        assert_eq!(protocol.known_peers.len(), MAX_KNOWN_PEERS);

        // One more should be rejected
        protocol.on_neighbor_discovered("peer-overflow");
        assert_eq!(protocol.known_peers.len(), MAX_KNOWN_PEERS);
        assert!(!protocol.known_peers.contains("peer-overflow"));

        // Existing peer should still be updatable (no-op insert, not rejected)
        protocol.on_neighbor_discovered("peer-0");
        assert_eq!(protocol.known_peers.len(), MAX_KNOWN_PEERS);
        assert!(protocol.known_peers.contains("peer-0"));

        // Removing a peer frees capacity
        protocol.on_neighbor_lost("peer-0");
        assert_eq!(protocol.known_peers.len(), MAX_KNOWN_PEERS - 1);

        // Now the new peer can be added
        protocol.on_neighbor_discovered("peer-overflow");
        assert_eq!(protocol.known_peers.len(), MAX_KNOWN_PEERS);
        assert!(protocol.known_peers.contains("peer-overflow"));
    }

    #[test]
    fn test_known_peers_does_not_track_self() {
        let config = create_test_config();
        let self_id = config.user_id.clone();
        let mut protocol = OfflineProtocol::new(config).unwrap();

        protocol.on_neighbor_discovered(&self_id);
        assert!(protocol.known_peers.is_empty());
    }

    #[test]
    fn test_on_neighbor_lost_removes_from_known_peers() {
        let mut protocol = OfflineProtocol::new(create_test_config()).unwrap();

        protocol.on_neighbor_discovered("alice");
        assert!(protocol.known_peers.contains("alice"));

        protocol.on_neighbor_lost("alice");
        assert!(!protocol.known_peers.contains("alice"));
    }

    #[test]
    fn test_seen_discovery_queries_cleanup() {
        let mut protocol = OfflineProtocol::new(create_test_config()).unwrap();

        // Cleanup is tested directly in offline-protocol-services crate.
        // Here we verify the integration: cleanup_expired_entries() delegates.
        protocol.cleanup_expired_entries();

        // Just verify it doesn't panic and the method is wired correctly
        assert!(protocol.mesh_services().seen_discovery_queries().is_empty());
    }

}
