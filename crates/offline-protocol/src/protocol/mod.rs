//! Main protocol engine.

mod blocking;
mod config_accessors;
mod decryption_queue;
mod message_dispatch;
mod observability;
mod pending_queue;
mod prefixes;
mod receive;
mod security;
mod send;
mod session;
pub(crate) mod state_crypto;
mod storage;
mod types;

pub(crate) use decryption_queue::PendingDecryptionQueue;
pub use decryption_queue::PendingQueueMetrics;
pub(crate) use prefixes::*;
pub(crate) use types::*;
pub use types::{MediaSendOptions, ProtocolState, SendMessageOptions};

use crate::file_transfer::{FileTransferManager, OutboundTransferState};
use crate::mls_observability::{MlsEventEmitter, MlsEventRateLimiter, NoopMlsEventEmitter};
use crate::telemetry::aggregator::{
    build_metrics_frame, device_battery_from_available, diff_device_capability,
    diff_transport_state, DeviceSnap,
};
use crate::telemetry::{
    dispatch_record, Scrubber, TelemetryConfig, TelemetryContext, TelemetryRecord, TelemetrySink,
};
use crate::{
    Error, EstablishmentState, Event, ProtocolConfig, ProtocolStateStorage, Result,
    TransportManager,
};
use chrono::{DateTime, Utc};
use offline_protocol_core::{LamportClock, Message, MessageId, MutexExt};
use offline_protocol_mls::{EncryptedMessage, MlsManager, MlsStorage, WelcomeMessage};
use offline_protocol_reliability::{AckManager, Deduplicator, RetryQueue};
use offline_protocol_router::{
    PathSelector, RelayDemotionReason, RelayManager, RelayRole, RelayTransition, TransportSelector,
};
use offline_protocol_services::MeshServices;
use offline_protocol_transport::{BleTransport, TransportStatus, TransportType};
use std::collections::HashMap;
use std::collections::HashSet;
use std::sync::{Arc, Mutex, RwLock};
use std::time::Instant;
use tracing::{debug, error, info, warn};
use zeroize::Zeroizing;

/// Returns whether `timestamp` is at least `lifetime_ms` old without relying
/// on Chrono's panicking `DateTime - Duration` operator.
///
/// A lifetime too large to represent, or whose cutoff predates Chrono's
/// calendar range, cannot expire a representable timestamp.
fn lifetime_expired(now: DateTime<Utc>, timestamp: DateTime<Utc>, lifetime_ms: u64) -> bool {
    let Ok(lifetime_ms) = i64::try_from(lifetime_ms) else {
        return false;
    };
    now.checked_sub_signed(chrono::Duration::milliseconds(lifetime_ms))
        .is_some_and(|cutoff| timestamp <= cutoff)
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
    pub(crate) transport_manager: TransportManager,

    /// Path selector for routing (includes relay scoring logic).
    path_selector: PathSelector,

    /// ACK manager for tracking acknowledgments.
    ack_manager: AckManager,

    /// Retry queue for failed messages.
    retry_queue: RetryQueue,

    /// Deduplicator for preventing duplicates.
    pub(crate) deduplicator: Deduplicator,

    /// Shared mutable state.
    shared_state: Arc<Mutex<SharedState>>,

    /// Messages awaiting delivery/acknowledgment (store-and-forward outbox).
    outbox: HashMap<MessageId, OutboxEntry>,

    /// Transient staging for outbox re-seal provenance, keyed by message id.
    /// Populated at send time (where the plaintext is in scope) and consumed
    /// when the outbox entry is first created (which happens deep in the send/
    /// retry machinery, from the already-sealed `Message`). Once consumed it
    /// lives on the `OutboxEntry` itself. See [`OutboxReseal`].
    pending_reseal: HashMap<MessageId, OutboxReseal>,

    /// Dedicated outbox for file chunk messages, separate from the main outbox
    /// to prevent large file transfers from evicting regular messages.
    media_outbox: HashMap<MessageId, OutboxEntry>,

    /// Consecutive unreachable-park counter per recipient for plain DMs,
    /// driving the escalating reachability-probe interval on every carrier
    /// (see `handle_recipient_unreachable_for_message` —
    /// mirrors `WelcomeLifecycleRecord::unreachable_parks`). Reset on every
    /// reachability edge (`flush_outbox_for_peer_via` / `flush_outbox_all`) and
    /// on delivery; pruned alongside `cleanup_outbox` so only peers that
    /// still hold outbox entries can retain a counter. While live, ACK
    /// exhaustion for the recipient's plain DMs re-parks instead of
    /// settling terminally (`try_repark_exhausted_dm`). In-memory only: a
    /// restart re-drives the outbox anyway, which is itself a fresh probe.
    dm_unreachable_parks: HashMap<String, u32>,

    /// MLS manager for end-to-end encryption.
    mls_manager: Option<Arc<RwLock<MlsManager>>>,

    /// Pending messages waiting for session establishment (recipient -> messages).
    pending_encrypted_messages: HashMap<String, Vec<PendingMessage>>,

    /// Recipients whose persisted pending queue was not read *this session* and
    /// is still on disk: either the read failed (`PendingRestore::Unavailable`)
    /// or the restore walk stopped at one of its bounds before reaching it.
    ///
    /// The pending queue is persisted as one record per recipient holding the
    /// whole queue, so honoring `Unavailable` at restore is not enough on its
    /// own: the record is left in place, nothing is in memory for that
    /// recipient, and the very next enqueue would write a snapshot of that
    /// empty-plus-one view straight over it — destroying queued messages the
    /// app is still holding ids for, with no settlement at all. That is the
    /// silent loss the three-state read exists to prevent, arriving through
    /// the runtime path instead of the restore path. A record the walk simply
    /// never opened is the same record in the same state, so it is frozen for
    /// the same reason — a bound that promises to leave the tail "for a later
    /// launch" has to mean it for the whole session, not just until the next
    /// send.
    ///
    /// So a recipient recorded here is *frozen on disk* for the rest of the
    /// session: writes and deletes for its record are refused, and the
    /// in-memory queue is used exactly as it is when no storage is configured
    /// at all. A later launch reads the record and settles or restores it
    /// properly. The outbox needs no equivalent — it is keyed per message id,
    /// so an unreadable entry cannot be overwritten by an unrelated write.
    pending_queues_unreadable_this_session: HashSet<String>,

    /// Terminal message settlements produced while restoring, held until the
    /// event pipeline is live.
    ///
    /// Restore runs inside `initialize_mls`, which apps routinely call before
    /// installing an event callback — so a `message_failed` emitted there would
    /// be dropped, and the app would keep an id that never resolves. `start()`
    /// drains this, mirroring how restored media descriptors already wait to be
    /// announced.
    ///
    /// Explicitly capped at [`MAX_DEFERRED_RESTORE_SETTLEMENTS`] rather than
    /// left to the restore caps that feed it. Those caps do bound it, but they
    /// bound it as a *sum* across every category, and nothing drains this until
    /// `start()` — which an app that only ever calls `initialize_mls`, or that
    /// retries it against a failing store, may never reach.
    deferred_restore_settlements: Vec<Event>,

    /// Number of settlements suppressed by the cap above, so the count is
    /// reported even though the individual events are not.
    suppressed_restore_settlements: usize,

    /// Earliest wall-clock expiry in `pending_encrypted_messages`.
    ///
    /// `process()` consults this before scanning the bounded queue, avoiding an
    /// O(N) walk on every 100 ms tick while preserving exact configured expiry.
    /// A stale early deadline is harmless: it causes one extra scan, which then
    /// recomputes the real minimum.
    next_pending_message_expiry: Option<DateTime<Utc>>,

    /// Key packages received but not yet used (sender_id -> package).
    pub(crate) pending_key_packages: HashMap<String, ReceivedKeyPackage>,

    /// Set of peers we've already sent our key package to.
    pub(crate) key_package_sent_to: std::collections::HashSet<String>,

    /// All discovered/connected peers, tracked independently of encryption.
    /// Used by service discovery to know who to broadcast queries to.
    /// Values are last-seen instants: refreshed on every discovery signal,
    /// swept by [`Self::prune_stale_known_peers`] after `KNOWN_PEER_TTL_SECS`,
    /// and used to pick the least-recently-seen victim when an insert hits
    /// `MAX_KNOWN_PEERS`.
    known_peers: HashMap<String, Instant>,

    /// Sessions confirmed established (received Welcome or successful decrypt).
    /// Only encrypt messages when the session is confirmed to avoid race conditions.
    confirmed_sessions: std::collections::HashSet<String>,

    /// Peers whose key package advertised the compact MLS envelope
    /// ([`MLS_ENVELOPE_COMPACT_V1`] in `env_versions`), so
    /// `seal_encrypted_content` may emit it instead of the legacy JSON form.
    /// Learned from key-package exchange, persisted per peer as
    /// [`PeerCapabilities`], and repopulated by `restore_peer_capabilities`
    /// on `initialize_mls` — unlike the transport manager's binary-wire
    /// registry, which stays in-memory because direct connections re-exchange
    /// key packages anyway. Bounded like `key_package_sent_to` (it is keyed
    /// by the wire-claimed sender).
    peer_compact_envelope: std::collections::HashSet<String>,

    /// Peers whose key package advertised the sealed rich payload
    /// ([`RICH_PAYLOAD_V1`] in `rich_versions`), so the send path may seal
    /// rich extras (reply context, rich media metadata, forward attribution)
    /// inside the `__RICH_V1__` body. Same lifecycle as
    /// `peer_compact_envelope` above: learned from key-package exchange,
    /// persisted, restored on `initialize_mls`, bounded like
    /// `key_package_sent_to`. Forgetting a peer only costs silently dropped
    /// rich extras — never a cleartext fallback.
    peer_rich_payload: std::collections::HashSet<String>,

    /// Peers whose sealed-rich-payload support we learned *indirectly*: a
    /// group inviter attested it on the Add commit (to existing members) or
    /// the Welcome (to the joining member), because the members of a group
    /// never directly exchange key packages with everyone else. Kept
    /// separate from `peer_rich_payload` (direct self-advertisement) so
    /// direct knowledge can stay authoritative: any directly received key
    /// package from a peer evicts its attested entry. Consulted only by
    /// `group_rich_seal_active` — never DM sealing (which always has direct
    /// knowledge: session establishment exchanges key packages) and never
    /// envelope selection (a stale attestation there could corrupt
    /// decoding; here the worst case is one member rendering a literal
    /// `__RICH_V1__` body). Persisted inside `PeerCapabilities`, restored
    /// on `initialize_mls`, bounded like `key_package_sent_to`.
    peer_rich_attested: std::collections::HashSet<String>,

    /// Peers already flagged with a `PlaintextSend` security warning, so the
    /// explicit-opt-out plaintext path warns once per peer instead of once
    /// per message.
    pub(crate) plaintext_send_warned: std::collections::HashSet<String>,

    /// Peers already flagged with a `PlaintextReceiveRejected` security
    /// warning, so a chatty legacy or malicious peer warns once per peer
    /// instead of once per rejected message. Keys are wire-claimed
    /// (attacker-controllable) sender ids, so the set is bounded: it resets
    /// at `MAX_PLAINTEXT_RECEIVE_WARNED_PEERS` instead of growing without
    /// limit.
    pub(crate) plaintext_receive_warned: std::collections::HashSet<String>,

    /// Bounded pending decryption queue for encrypted messages received before
    /// the MLS session is ready.
    pub(crate) pending_queue: PendingDecryptionQueue,

    /// Secure storage for MLS/key material and SDK-owned secrets.
    secure_storage: Option<Arc<dyn MlsStorage>>,

    /// Install-scoped storage for restartable message-plane and protocol state.
    protocol_state_storage: Option<Arc<dyn ProtocolStateStorage>>,

    /// Seals sensitive protocol-state records before they reach the
    /// install-scoped store, with a per-install key held in `secure_storage`.
    ///
    /// `None` until that key is available (and if it never becomes available,
    /// for the whole session). Sealed categories then fail *closed*: they are
    /// not persisted at all rather than written in the clear, since losing
    /// crash recovery is recoverable and losing at-rest confidentiality is not.
    /// See `state_crypto` and `restore_or_init_state_record_key`.
    ///
    /// Always belongs to the currently attached `secure_storage` — it is
    /// re-derived, never carried across a storage swap.
    state_record_cipher: Option<state_crypto::StateRecordCipher>,

    /// Lamport logical clock for causal message ordering.
    /// Ticked on send, merged on receive.
    lamport_clock: LamportClock,

    /// Retry schedule for peers whose confirmation persistence failed.
    confirmation_retry_due_at: HashMap<String, DateTime<Utc>>,

    /// Probe schedule for pending sessions to guarantee post-restart convergence.
    confirmation_probe_due_at: HashMap<String, DateTime<Utc>>,

    /// Rate-limit schedule for 1:1 session re-keys triggered by an epoch-desync
    /// decrypt failure. Bounds the re-key to at most one per peer per
    /// `REKEY_INTERVAL_SECS` so a peer replaying stale-epoch ciphertext (or an
    /// injected wrong-epoch frame) cannot drive a re-key storm. The floor is
    /// **never reset early** — not even by a successful decrypt on the healed
    /// session — because a genuine re-fork and a replay are indistinguishable
    /// here; it lapses only by the interval elapsing (see
    /// `schedule_session_rekey`). In-memory only: a re-key is a live-connectivity
    /// action, and a fresh desync after restart simply re-arms it.
    rekey_due_at: HashMap<String, DateTime<Utc>>,

    /// Outbound welcome lifecycle records keyed by peer id.
    welcome_lifecycles: HashMap<String, WelcomeLifecycleRecord>,

    /// Outbound connection requests still awaiting a transport outcome,
    /// keyed by message id → recipient. Lets `on_transport_send_failed`
    /// turn the relay's authoritative "recipient offline" DeliveryError
    /// into a typed `ConnectionRequestUndeliverable` event instead of
    /// silently discarding it (the welcome-only path never matches a
    /// connection request's message id). Entries are dropped on emission
    /// and on proof of delivery (the request's delivery ack, or an inbound
    /// accept/reject from the recipient), and TTL-pruned on insert and at
    /// read; bounded, so an app spamming requests cannot grow it
    /// unboundedly.
    pending_connection_requests: HashMap<String, PendingConnectionRequest>,

    /// Backoff state for presence-driven welcome rescue, keyed by peer id.
    /// Bounds how often an online-but-never-confirming peer triggers a
    /// welcome re-arm/re-send (see `on_peer_presence`). Entries are dropped
    /// lazily once the peer has no unconfirmed welcome left.
    welcome_presence_rescue: HashMap<String, PresenceRescueThrottle>,

    /// Peers for which we are the both-create "owner" (kept our own session
    /// group on the lexicographic tiebreak) and are still awaiting a
    /// *group-aware* proof that the peer adopted our group. While a peer is in
    /// this set, only `decrypt_success` may confirm the session — a plaintext
    /// confirmation probe/ack is NOT group-aware (it only proves the peer holds
    /// *some* session, possibly its own pre-adoption group) and must not be
    /// allowed to stop our Welcome retransmission, or the peer could be left
    /// stranded on a divergent group. Persisted (see
    /// `restore_both_create_awaiting_decrypt`) so an owner restart mid-convergence
    /// keeps the gate, rather than letting a stale plaintext probe/ack confirm
    /// prematurely. Mutate only via `mark_both_create_awaiting_decrypt` /
    /// `clear_both_create_awaiting_decrypt` to keep memory and storage in sync.
    both_create_awaiting_decrypt: std::collections::HashSet<String>,

    /// Sink for MLS lifecycle telemetry.
    mls_event_emitter: Arc<dyn MlsEventEmitter>,

    /// Rate limiting policy for MLS failure event floods.
    mls_event_rate_limiter: MlsEventRateLimiter,

    /// Pre-install scrubber used by MLS emit sites before
    /// `install_telemetry_sink` is called. Once a sink is installed, emit
    /// sites read `self.telemetry.scrubber` instead via `current_scrubber()`.
    /// Both scrubbers share `telemetry_fallback_secret` so opaque identifiers
    /// observed by the legacy `MlsEventEmitter` stay consistent across the
    /// install boundary.
    telemetry_scrubber: Scrubber,

    /// Per-instance fallback secret for identifier scrubbing. Random at
    /// construction. Reused by both the pre-install `telemetry_scrubber`
    /// and by any scrubber built inside `install_telemetry_sink` (unless the
    /// installed `TelemetryConfig` carries its own `scrub_secret`). Keeping
    /// the fallback stable across installs means the legacy
    /// `MlsEventEmitter` path observes consistent opaque IDs before and
    /// after a sink is installed on the same protocol instance.
    telemetry_fallback_secret: [u8; 16],

    /// Whether the persistent per-install scrub secret has been loaded from
    /// (or initialized into) secure storage yet. Starts `false`; set `true`
    /// the first time `restore_or_init_scrub_secret` succeeds so the load is
    /// idempotent across the two storage-entry paths
    /// (`initialize_mls` also enables persistence, which would otherwise
    /// re-load and rebuild the scrubber a second time).
    telemetry_secret_persisted: bool,

    /// Whether the persistent per-install Nostr signing secret has been
    /// loaded from (or initialized into) secure storage and installed into
    /// the Nostr transport yet. Same idempotency role across the two
    /// storage-entry paths as `telemetry_secret_persisted`; see
    /// `restore_or_init_nostr_signing_secret`.
    nostr_secret_persisted: bool,

    /// A Nostr signing secret that was installed into the transport but
    /// could not be persisted (storage `store` failed). Kept so the next
    /// `restore_or_init_nostr_signing_secret` attempt retries persisting
    /// this same secret instead of rotating the install's relay-visible
    /// identity again mid-session.
    nostr_unpersisted_secret: Option<Zeroizing<[u8; 32]>>,

    /// Installed telemetry context (sink + config + scrubber). `None` until
    /// `install_telemetry_sink` is called; thereafter shared with
    /// `SharedState` via `Arc` clone so both emit paths dispatch through the
    /// same configuration.
    ///
    /// The duplication with [`SharedState::telemetry`] is deliberate: MLS
    /// lifecycle emission is driven from `&self` on `OfflineProtocol` and
    /// does not hold the shared-state lock, while protocol-event emission
    /// happens inside `SharedState::emit_event` under the lock. Each path
    /// reads the context from whichever side it already has in hand, and
    /// `install_telemetry_sink` is the single writer that keeps both copies
    /// in sync (guaranteed atomic from the caller's perspective because
    /// `&mut self` excludes concurrent calls).
    pub(crate) telemetry: Option<Arc<TelemetryContext>>,

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
    /// Descriptors of transfers that were in flight when the previous process
    /// died, restored from storage. `start()` emits one `MediaResendRequired`
    /// event each but deliberately does NOT drain them: entries stay parked
    /// (and persisted) until a same-`file_id` resend consumes them — after
    /// `send_media_with` checksum-validates the re-supplied bytes against
    /// the descriptor — or the restore TTL prunes them, so an app that
    /// misses the signal gets it again next restart.
    restored_media_descriptors: HashMap<String, MediaTransferDescriptor>,

    /// Mesh service registry and handler (extracted crate).
    mesh_services: MeshServices,

    /// Bundled state for mesh group messaging (member cache, dedup, pending commits).
    pub(crate) group_mesh: crate::group_mesh::GroupMeshState,

    /// TOFU (Trust-On-First-Use) key store for peer Ed25519 public keys.
    ///
    /// The first signed control message from a peer pins their public key here.
    /// Subsequent messages from the same peer must present the same key;
    /// a mismatch triggers a security warning and the message is dropped.
    /// Entries track a last-seen timestamp for LRU eviction when the store is full.
    ///
    /// Persisted via `MlsStorage` (when available) to survive restarts.
    // Manual TOFU reset is available via `reset_tofu_for_peer()`; see MAX_TOFU_PEERS doc.
    known_peer_public_keys: HashMap<String, TofuEntry>,

    /// Set of blocked user IDs. Messages from blocked users are silently
    /// dropped (no ACK, no event). Persisted via `ProtocolStateStorage`.
    blocked_users: HashSet<String>,

    /// Timestamp of the last `kick_pending_session_reconciliation` execution.
    /// Used to throttle reconciliation to avoid expensive storage I/O
    /// (list_sessions → Keychain/Keystore) on every process tick / receive poll.
    last_reconciliation_at: Option<Instant>,

    /// Instant of the last periodic `MetricsFrame` emission, used to rate
    /// the telemetry aggregator to `TelemetryConfig::metrics_cadence`.
    last_metrics_emit_at: Option<Instant>,

    /// Last observed per-transport status map, used by the telemetry
    /// aggregator to diff against the current map and emit one
    /// `TransportStateEvent` per transition.
    transport_status_snapshot: HashMap<TransportType, TransportStatus>,

    /// Last observed (battery, charging, relay_role) snapshot, used by the
    /// telemetry aggregator to fire `DeviceCapabilitySnapshot` on change.
    device_capability_snapshot: Option<DeviceSnap>,

    /// The Lamport clock value at the time of the last storage write.
    /// Used to debounce `persist_lamport_clock()` — only write when the
    /// in-memory value has advanced past this by `LAMPORT_PERSIST_INTERVAL`
    /// ticks. On crash, at most `LAMPORT_PERSIST_INTERVAL` ticks are lost,
    /// which is harmless for a logical clock (the gap is absorbed by the
    /// next merge with any peer).
    last_persisted_lamport: u64,
}

impl Drop for OfflineProtocol {
    fn drop(&mut self) {
        // Flush debounced Lamport clock so no ticks are lost when the
        // protocol is dropped without an explicit stop() call.
        self.flush_lamport_clock();
    }
}

#[cfg(test)]
struct TestProtocolStateStorage {
    storage: Arc<dyn MlsStorage>,
}

/// Maps the fixture's MLS-storage failures onto the protocol-state contract,
/// mirroring what the UniFFI adapter does for real providers.
#[cfg(test)]
pub(crate) fn map_test_storage_error(
    error: offline_protocol_mls::StorageError,
) -> crate::ProtocolStateError {
    use crate::ProtocolStateError as P;
    use offline_protocol_mls::StorageError as S;
    match error {
        S::KeyNotFound(detail) => P::NotFound(detail),
        S::CorruptedData(detail) => P::Corrupted(detail),
        S::StoreFailed(detail) => P::StoreFailed(detail),
        S::LoadFailed(detail) => P::LoadFailed(detail),
        S::DeleteFailed(detail) => P::DeleteFailed(detail),
        S::Unavailable(detail) => P::LoadFailed(detail),
    }
}

#[cfg(test)]
impl ProtocolStateStorage for TestProtocolStateStorage {
    fn store(&self, key_type: &str, key_id: &str, data: &[u8]) -> crate::ProtocolStateResult<()> {
        self.storage
            .store(key_type, key_id, data)
            .map_err(map_test_storage_error)
    }

    fn load(&self, key_type: &str, key_id: &str) -> crate::ProtocolStateResult<Option<Vec<u8>>> {
        self.storage
            .load(key_type, key_id)
            .map_err(map_test_storage_error)
    }

    fn delete(&self, key_type: &str, key_id: &str) -> crate::ProtocolStateResult<()> {
        self.storage
            .delete(key_type, key_id)
            .map_err(map_test_storage_error)
    }

    fn list_keys(&self, key_type: &str) -> crate::ProtocolStateResult<Vec<String>> {
        self.storage
            .list_keys(key_type)
            .map_err(map_test_storage_error)
    }
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

        // Per-instance fallback secret for identifier scrubbing. Stable for
        // the life of this protocol instance so opaque IDs stay consistent
        // across any later `install_telemetry_sink` call that does not
        // supply its own `scrub_secret`.
        let telemetry_fallback_secret = *uuid::Uuid::new_v4().as_bytes();

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
            pending_reseal: HashMap::new(),
            media_outbox: HashMap::new(),
            dm_unreachable_parks: HashMap::new(),
            mls_manager: None,
            pending_encrypted_messages: HashMap::new(),
            pending_queues_unreadable_this_session: HashSet::new(),
            deferred_restore_settlements: Vec::new(),
            suppressed_restore_settlements: 0,
            next_pending_message_expiry: None,
            pending_key_packages: HashMap::new(),
            key_package_sent_to: std::collections::HashSet::new(),
            known_peers: HashMap::new(),
            confirmed_sessions: std::collections::HashSet::new(),
            peer_compact_envelope: std::collections::HashSet::new(),
            peer_rich_payload: std::collections::HashSet::new(),
            peer_rich_attested: std::collections::HashSet::new(),
            plaintext_send_warned: std::collections::HashSet::new(),
            plaintext_receive_warned: std::collections::HashSet::new(),
            pending_queue: PendingDecryptionQueue::default(),
            secure_storage: None,
            protocol_state_storage: None,
            state_record_cipher: None,
            lamport_clock: LamportClock::new(),
            confirmation_retry_due_at: HashMap::new(),
            confirmation_probe_due_at: HashMap::new(),
            rekey_due_at: HashMap::new(),
            welcome_lifecycles: HashMap::new(),
            pending_connection_requests: HashMap::new(),
            welcome_presence_rescue: HashMap::new(),
            both_create_awaiting_decrypt: std::collections::HashSet::new(),
            mls_event_emitter: Arc::new(NoopMlsEventEmitter),
            mls_event_rate_limiter: MlsEventRateLimiter::default(),
            // The pre-install scrubber uses `TelemetryConfig::default()` —
            // `scrub_ids=true`, no explicit `scrub_secret`. Identifier hashing
            // is therefore on from the moment the protocol is constructed,
            // matching today's always-scrub MLS observability semantics. When
            // `install_telemetry_sink` later supplies a config with
            // `scrub_ids(false)`, a new scrubber is derived from that config
            // but reuses `telemetry_fallback_secret` so opaque IDs remain
            // stable across the install boundary.
            telemetry_scrubber: Scrubber::from_config(
                &TelemetryConfig::default(),
                telemetry_fallback_secret,
            ),
            telemetry_fallback_secret,
            telemetry_secret_persisted: false,
            nostr_secret_persisted: false,
            nostr_unpersisted_secret: None,
            telemetry: None,
            file_transfer_manager: FileTransferManager::new(),
            pending_media_metadata: HashMap::new(),
            outbound_media_transfers: HashMap::new(),
            outbound_media_chunks: HashMap::new(),
            outbound_media_windows: HashMap::new(),
            restored_media_descriptors: HashMap::new(),
            mesh_services: MeshServices::new(),
            group_mesh: crate::group_mesh::GroupMeshState::default(),
            known_peer_public_keys: HashMap::new(),
            blocked_users: HashSet::new(),
            last_reconciliation_at: None,
            last_metrics_emit_at: None,
            transport_status_snapshot: HashMap::new(),
            device_capability_snapshot: None,
            last_persisted_lamport: 0,
            config,
        })
    }

    /// Initializes MLS encryption and protocol persistence.
    ///
    /// This must be called before encryption can be used. The storage
    /// `secure_storage` must be a platform-native credential store.
    /// `protocol_state_storage` must be scoped to the app container and is
    /// removed when that container is deleted.
    ///
    /// Ownership model:
    /// - `OfflineProtocol` is the single authoritative owner of `MlsManager`
    /// - initialization is idempotent per protocol instance
    /// - subsequent calls return without replacing the existing manager
    /// - manager publication is transactional: restore must succeed before
    ///   `mls_manager` becomes visible to callers
    ///
    /// The two storage domains are intentionally different trait objects. This
    /// prevents message-plane state from inheriting the lifecycle of Keychain
    /// or another credential store.
    pub fn initialize_mls(
        &mut self,
        secure_storage: Arc<dyn MlsStorage>,
        protocol_state_storage: Arc<dyn ProtocolStateStorage>,
    ) -> Result<()> {
        self.initialize_mls_inner(secure_storage, protocol_state_storage, true)
    }

    /// `adopt_legacy_state` exists only so test fixtures can point both handles
    /// at one backend. Adoption moves records *between* the two stores, which
    /// on a shared backend would mean deleting a record through the same store
    /// it was just read from.
    fn initialize_mls_inner(
        &mut self,
        secure_storage: Arc<dyn MlsStorage>,
        protocol_state_storage: Arc<dyn ProtocolStateStorage>,
        adopt_legacy_state: bool,
    ) -> Result<()> {
        if self.mls_manager.is_some() {
            return Ok(());
        }

        let manager = Arc::new(RwLock::new(MlsManager::new(
            &self.config.user_id,
            secure_storage.clone(),
        )?));

        // Keep initialization transactional so a restore failure cannot leave
        // partially-initialized MLS state visible and then permanently block retries.
        let previous_secure_storage = self.secure_storage.clone();
        let previous_protocol_state_storage = self.protocol_state_storage.clone();
        // The record cipher belongs to the secure store it was loaded from, so
        // it is part of the same transaction as the storage handles: taken here
        // so a failed init cannot leave the new store's key installed next to
        // the rolled-back handles, and restored below alongside them.
        let previous_state_record_cipher = self.state_record_cipher.take();
        let previous_pending_messages = self.pending_encrypted_messages.clone();
        let previous_unreadable_pending_queues =
            self.pending_queues_unreadable_this_session.clone();
        let previous_pending_message_expiry = self.next_pending_message_expiry;
        // Populated by restore steps that run before other fallible ones, so
        // they belong in the transaction like everything else: a failed init
        // must not leave a key-package cache, parked media descriptors, or an
        // owner gate sourced from a store the rollback has just detached.
        let previous_pending_key_packages = self.pending_key_packages.clone();
        let previous_restored_media_descriptors = self.restored_media_descriptors.clone();
        let previous_both_create_awaiting_decrypt = self.both_create_awaiting_decrypt.clone();
        let previous_confirmed_sessions = self.confirmed_sessions.clone();
        let previous_welcome_lifecycles = self.welcome_lifecycles.clone();
        let previous_lamport_clock = self.lamport_clock.value();
        let previous_tofu_keys = self.known_peer_public_keys.clone();
        let previous_blocked_users = self.blocked_users.clone();
        let previous_outbox = self.outbox.clone();
        let previous_peer_compact_envelope = self.peer_compact_envelope.clone();
        let previous_peer_rich_payload = self.peer_rich_payload.clone();
        let previous_peer_rich_attested = self.peer_rich_attested.clone();

        self.secure_storage = Some(secure_storage);
        self.protocol_state_storage = Some(protocol_state_storage);

        // Must precede every restore below: sealed protocol-state records
        // (pending messages, outbox, media descriptors) are unreadable without
        // this key, so restoring first would silently start from empty and then
        // overwrite durable state with that empty view.
        self.restore_or_init_state_record_key();

        // Then adopt anything the pre-split build left in secure storage, so
        // the restores below see one complete view. Must follow the record key
        // (adoption seals what it moves) and precede every restore (a restore
        // that ran first would find nothing and then overwrite the legacy state
        // with that empty view). Best-effort and resumable: a failure here
        // leaves the legacy records in place for the next launch.
        if adopt_legacy_state {
            self.adopt_legacy_protocol_state();
        }

        // The settlement queue is deliberately NOT captured for rollback, and
        // that is the whole point rather than an omission.
        //
        // The rollback below restores *in-memory* state. It restores no storage
        // state, because restore has none to give back: by the time a later
        // step fails, an earlier one has already deleted an unaddressable
        // pending queue, dropped a record its store reported corrupt, and
        // rewritten a peer's snapshot without the entries it evicted for
        // capacity. Those records are gone. So the invariant adoption is
        // documented with — nothing can re-derive a settlement for a record
        // that no longer exists — is not special to adoption; it holds for
        // every settlement produced under this transaction, and rolling any of
        // them back means the application keeps an id that resolves to nothing,
        // on this launch and on every launch after it.
        //
        // The cost of not rolling back is that a retry which re-examines a
        // still-present record can settle the same id twice. `message_failed`
        // is terminal and idempotent for any reasonable consumer, and a
        // duplicate terminal event is a far smaller lie than silence.
        //
        // Load the persistent scrub secret outside the transactional restore
        // below: it is independent of MLS state, and a later MLS-restore
        // rollback must not undo it (the secret is idempotent and reused on
        // the next attempt).
        self.restore_or_init_scrub_secret();

        // Same reasoning for the Nostr signing secret: independent of MLS
        // state, idempotent, and must survive an MLS-restore rollback.
        self.restore_or_init_nostr_signing_secret();

        // Restore state from previous session
        let restore_result = (|| {
            self.restore_pending_messages()?;
            self.restore_lamport_clock();
            self.restore_tofu_keys();
            self.restore_blocked_users()?;
            self.restore_session_states_from_manager(manager.clone())?;
            self.restore_peer_key_packages(&manager)?;
            // Must precede start(): flush_restored_confirmed_pending_messages
            // re-makes the rich seal decision against these sets, and an
            // empty set there silently drops queued rich extras.
            self.restore_peer_capabilities(&manager);
            self.restore_welcome_lifecycles()?;
            self.restore_outbox()?;
            self.restore_media_descriptors()?;
            self.restore_both_create_awaiting_decrypt();
            Ok(())
        })();

        if let Err(err) = restore_result {
            self.secure_storage = previous_secure_storage;
            self.protocol_state_storage = previous_protocol_state_storage;
            self.state_record_cipher = previous_state_record_cipher;
            self.pending_encrypted_messages = previous_pending_messages;
            self.pending_queues_unreadable_this_session = previous_unreadable_pending_queues;
            self.next_pending_message_expiry = previous_pending_message_expiry;
            // `deferred_restore_settlements` is deliberately left alone — see
            // the comment where the other baselines are captured.
            self.pending_key_packages = previous_pending_key_packages;
            self.restored_media_descriptors = previous_restored_media_descriptors;
            self.both_create_awaiting_decrypt = previous_both_create_awaiting_decrypt;
            self.confirmed_sessions = previous_confirmed_sessions;
            self.welcome_lifecycles = previous_welcome_lifecycles;
            self.lamport_clock = LamportClock::from_value(previous_lamport_clock);
            self.known_peer_public_keys = previous_tofu_keys;
            self.blocked_users = previous_blocked_users;
            self.outbox = previous_outbox;
            self.peer_compact_envelope = previous_peer_compact_envelope;
            self.peer_rich_payload = previous_peer_rich_payload;
            self.peer_rich_attested = previous_peer_rich_attested;
            return Err(err);
        }

        self.mls_manager = Some(manager);
        self.emit_mls_initialized();

        info!(user_id = %self.config.user_id, "MLS encryption initialized with split secure and protocol-state storage");
        Ok(())
    }

    /// Test-only adapter for fixtures that intentionally use one
    /// in-memory/fault-injection backend to observe both storage domains.
    #[cfg(test)]
    pub(crate) fn initialize_mls_for_test(&mut self, storage: Arc<dyn MlsStorage>) -> Result<()> {
        let protocol_state_storage = Arc::new(TestProtocolStateStorage {
            storage: storage.clone(),
        });
        self.initialize_mls_inner(storage, protocol_state_storage, false)
    }

    /// Test-only storage initialization for persistence-focused fixtures that
    /// do not need an MLS manager.
    #[cfg(test)]
    pub(crate) fn enable_message_persistence_for_test(
        &mut self,
        storage: Arc<dyn MlsStorage>,
    ) -> Result<()> {
        self.secure_storage = Some(storage.clone());
        self.protocol_state_storage = Some(Arc::new(TestProtocolStateStorage { storage }));
        self.restore_or_init_state_record_key();
        self.restore_or_init_scrub_secret();
        self.restore_or_init_nostr_signing_secret();
        self.restore_pending_messages()?;
        self.restore_lamport_clock();
        self.restore_tofu_keys();
        self.restore_blocked_users()?;
        self.restore_outbox()?;
        self.restore_media_descriptors()?;
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

    /// Installs the unified telemetry sink for this protocol instance.
    ///
    /// Every subsequent protocol [`Event`] and MLS lifecycle event is
    /// additionally forwarded to `sink` as a
    /// [`crate::telemetry::TelemetryRecord`], gated by the verbosity tier
    /// and identifier-scrubbing preferences in `config`. Protocol events
    /// flow through as [`TelemetryRecord::Protocol`], MLS lifecycle events
    /// as [`TelemetryRecord::Mls`].
    ///
    /// This does not replace the legacy [`crate::EventCallback`] and
    /// [`MlsEventEmitter`] paths — they continue to fire independently for
    /// backward compatibility.
    ///
    /// Events emitted before this call (for example
    /// [`crate::mls_observability::MlsLifecycleEvent::Initialized`] fired
    /// during `initialize_mls`) are not replayed to a sink installed later.
    ///
    /// # Re-install semantics
    ///
    /// Calling this method a second time *replaces* the previously installed
    /// sink and config — the old sink stops receiving records as soon as
    /// this call returns. The per-instance fallback secret is preserved so
    /// opaque identifiers stay stable across the swap when the new config
    /// does not carry its own `scrub_secret`.
    ///
    /// The per-tick diff snapshots (transport status, device capability,
    /// metrics cadence) are **rearmed** on every install: the next
    /// `process()` tick observes the current state as its baseline and emits
    /// no synthetic transitions for transports that were already available
    /// at install time. Apps that need the current transport statuses at
    /// install time should pull them explicitly via
    /// [`TransportManager::get_all_transport_statuses`].
    ///
    /// The escalation-trigger dedupe window (`ESCALATION_TRIGGER_DEDUPE_SECS`
    /// inside [`TransportManager`]) is **not** rearmed. A sink installed
    /// within that window after an escalation event fired to a previous
    /// sink may miss the next occurrence of the same reason until the
    /// window elapses. This is intentional: the dedupe is a property of
    /// the routing engine, not of the observer.
    ///
    /// # Lifecycle
    ///
    /// Installing a sink wires the structured routing-decision callback
    /// on the underlying [`TransportManager`]. The callback lives for the
    /// life of the protocol instance or until another
    /// `install_telemetry_sink` call replaces it — `start()` and `stop()`
    /// do not toggle it. This means apps can install a sink before
    /// `start()` and see routing records from the very first `send()`,
    /// and a `stop() → start()` cycle preserves the wiring so no
    /// re-install is needed. Conversely, apps that never install a sink
    /// pay no per-`send()` routing-emission overhead.
    ///
    /// # Errors
    ///
    /// Returns `Err` if the shared-state mutex is poisoned (indicating an
    /// earlier panic in another thread while holding the lock). On error no
    /// sink is installed, and the legacy emission paths are unaffected.
    ///
    /// [`TelemetryRecord::Protocol`]: crate::telemetry::TelemetryRecord::Protocol
    /// [`TelemetryRecord::Mls`]: crate::telemetry::TelemetryRecord::Mls
    pub fn install_telemetry_sink(
        &mut self,
        sink: Arc<dyn TelemetrySink>,
        config: TelemetryConfig,
    ) -> Result<()> {
        // Forward the routing-diagnostic preference to the TransportManager
        // before wiring any callbacks so the very first routing decision
        // post-install already reflects the requested tier.
        self.transport_manager
            .set_routing_diagnostic(config.routing_diagnostic());

        let ctx = TelemetryContext::new(sink, config, self.telemetry_fallback_secret);
        let mut state = lock_shared_state(&self.shared_state).map_err(|err| {
            error!(
                error = %err,
                "install_telemetry_sink: shared-state mutex poisoned; sink NOT installed",
            );
            err
        })?;
        state.telemetry = Some(ctx.clone());
        drop(state);
        self.telemetry = Some(ctx);

        // Wire the structured routing-decision callback here so the
        // `TransportManager` pays the per-`send()` emission cost only when
        // a sink is actually installed, and so the wiring persists across
        // `stop() → start()` cycles (the callback reads `s.telemetry` on
        // every invocation and that field stays set). A subsequent
        // `install_telemetry_sink` replaces the closure with one capturing
        // the fresh `shared_routing` clone; both closures read the same
        // `SharedState::telemetry` slot, so even a concurrent in-flight
        // emission from the pre-replace closure would still resolve to
        // the currently-installed sink.
        let shared_routing = self.shared_state.clone();
        self.transport_manager
            .set_routing_decision_callback(Some(Arc::new(move |decision| {
                let s = shared_routing.lock_or_recover();
                if let Some(ctx) = &s.telemetry {
                    let record = TelemetryRecord::Routing(Box::new(decision));
                    // Dispatch is panic-isolated so a sink that panics
                    // here cannot unwind through the live `MutexGuard`
                    // and poison `SharedState`.
                    dispatch_record(&ctx.sink, &record);
                }
            })));

        // Rearm the per-tick diff snapshots so the first tick after install
        // reports honest transitions only. Without this, a sink installed
        // after start() would observe a synthetic `Unavailable → Available`
        // transition for every already-running transport on the next tick.
        let (statuses, available) = self.transport_manager.snapshot_status_and_available();
        self.transport_status_snapshot = statuses;
        let (battery_level, is_charging) =
            device_battery_from_available(self.transport_manager.current_transport(), &available);
        let relay_role = self.path_selector.current_relay_role();
        self.device_capability_snapshot = Some(DeviceSnap::from_parts(
            battery_level,
            is_charging,
            relay_role,
        ));
        // Leave `last_metrics_emit_at` at None so the first tick after
        // install fires a fresh metrics snapshot — that is the bootstrap
        // payload a new sink actually wants (full counter state in one
        // record).
        self.last_metrics_emit_at = None;

        Ok(())
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
                shared.lock_or_recover().emit_event(event);
            })));

        // NOTE: the structured routing-decision callback is wired by
        // `install_telemetry_sink`, not here. Its lifetime is tied to sink
        // presence rather than protocol start/stop — see the docstring on
        // `install_telemetry_sink` for the rationale.

        // Wire BLE fragment eviction callback so app receives FragmentAssemblyEvicted.
        if let Some(ble_arc) = self.transport_manager.get_transport(TransportType::BLE) {
            let shared = self.shared_state.clone();
            if let Some(ble) = ble_arc.as_any().downcast_ref::<BleTransport>() {
                ble.set_fragment_eviction_callback(Some(Arc::new(move |info| {
                    shared
                        .lock_or_recover()
                        .emit_event(Event::fragment_assembly_evicted(
                            info.message_id,
                            info.completion_percent,
                            "capacity".to_string(),
                        ));
                })));
            }
        }

        let mut state = lock_shared_state(&self.shared_state)?;

        state.state = ProtocolState::Running;
        drop(state);

        // Settle what restore could not recover, now that the event pipeline is
        // live. These ids were handed to the app by `send_message*` before the
        // process died; without this they would never resolve to anything.
        // Drained before the flush below so an app sees the failures first and
        // cannot mistake a restored send for the settlement of a lost one.
        self.drain_deferred_restore_settlements();

        self.flush_restored_confirmed_pending_messages();
        self.kick_pending_session_reconciliation("start");
        self.process_welcome_retry_queue()?;

        // Re-drive delivery of any outbox entries restored from a previous
        // session. They land here in the "stranded" state (not in the retry
        // queue, not awaiting an ACK), which flush_outbox_all already recovers:
        // it sends immediately where a transport is available and re-enqueues
        // the rest with backoff. Runs at start() rather than at restore time
        // because transports aren't up yet during initialize_mls.
        self.flush_outbox_all();

        // Media transfers that died with the previous process cannot be
        // re-driven — only their descriptors survive (never chunk bytes).
        // Tell the app which transfers to re-initiate, now that the event
        // pipeline is live. The descriptors stay parked (and persisted)
        // until the resend consumes them or the restore TTL prunes them,
        // so an app that misses this signal gets it again next restart.
        for descriptor in self.restored_media_descriptors.values() {
            self.emit_event(Event::media_resend_required(
                descriptor.file_id.clone(),
                descriptor.recipient.clone(),
                descriptor.file_name.clone(),
                descriptor.file_size,
            ));
        }

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

        // Flush debounced Lamport clock before stopping so no ticks are lost.
        drop(state);
        self.flush_lamport_clock();

        // Clear event callbacks to release shared_state references.
        // NOTE: the routing-decision callback is deliberately NOT cleared
        // here. Its lifetime is sink-scoped (installed by
        // `install_telemetry_sink`, replaced by a subsequent install),
        // not protocol-running-scoped — so a `stop() → start()` cycle
        // preserves the wiring without requiring the app to re-install.
        self.transport_manager.set_dors_event_callback(None);
        if let Some(ble_arc) = self.transport_manager.get_transport(TransportType::BLE) {
            if let Some(ble) = ble_arc.as_any().downcast_ref::<BleTransport>() {
                ble.set_fragment_eviction_callback(None);
            }
        }

        self.transport_manager.stop()?;
        let mut state = lock_shared_state(&self.shared_state)?;

        state.state = ProtocolState::Stopped;

        Ok(())
    }

    /// Pauses the protocol (for background mode).
    pub fn pause(&mut self) -> Result<()> {
        self.flush_lamport_clock();
        let mut state = lock_shared_state(&self.shared_state)?;

        if state.state != ProtocolState::Running {
            return Err(Error::NotStarted);
        }

        state.state = ProtocolState::Paused;
        Ok(())
    }

    /// Resumes the protocol from pause.
    pub fn resume(&mut self) -> Result<()> {
        {
            let mut state = lock_shared_state(&self.shared_state)?;

            if state.state != ProtocolState::Paused {
                return Err(Error::InvalidConfiguration(
                    "Protocol is not paused".to_string(),
                ));
            }

            state.state = ProtocolState::Running;
        }

        // A pause is the other edge back into a live event pipeline, so it owes
        // the same drain `start()` does. `settle_restored_message_failure` parks
        // anything it produces while the protocol is not `Running`, and
        // `update_retry_config` reaches it at runtime: shortening
        // `pending_message_max_lifetime_ms` in the background expires queued
        // messages and parks their terminal `message_failed`. Without this the
        // app would hold those ids until a `start()` that may never come again.
        self.drain_deferred_restore_settlements();
        Ok(())
    }

    /// Emits every terminal settlement parked while the event pipeline was not
    /// live, and reports the count of any the cap dropped.
    ///
    /// Called from both edges into `Running` ([`Self::start`] and
    /// [`Self::resume`]) so a parked settlement has no state to be stranded in.
    /// Idempotent: draining an empty queue emits nothing.
    fn drain_deferred_restore_settlements(&mut self) {
        for event in std::mem::take(&mut self.deferred_restore_settlements) {
            self.emit_event(event);
        }
        let suppressed = std::mem::take(&mut self.suppressed_restore_settlements);
        if suppressed > 0 {
            warn!(
                suppressed,
                cap = MAX_DEFERRED_RESTORE_SETTLEMENTS,
                "Restore produced more terminal settlements than any legitimate run can produce; \
                 the ids past the cap were not reported individually"
            );
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
        self.on_neighbor_discovered_via(peer_id, None);
    }

    /// [`Self::on_neighbor_discovered`] with an optional transport override
    /// for the outbox flush (see [`Self::flush_outbox_for_peer_via`]).
    /// `unpark_via` is `Some(Internet)` when the relay presence-online edge
    /// re-enters discovery through its rescue branch: a message whose
    /// forced-internet re-drive just failed sits in the retry queue with no
    /// pending ACK, so this inner flush would pick it up again — and
    /// without the override DORS could route it into the mesh void with the
    /// park counter already cleared, past re-park's reach.
    fn on_neighbor_discovered_via(&mut self, peer_id: &str, unpark_via: Option<TransportType>) {
        // Don't track ourselves
        if peer_id == self.config.user_id {
            return;
        }

        // Don't track or auto-exchange keys with blocked users
        if self.is_user_blocked(peer_id) {
            debug!(peer_id = %peer_id, "Ignoring neighbor discovery for blocked user");
            return;
        }

        // Track discovered peers for service discovery and routing. Existing
        // peers get their last-seen refreshed; a new peer at capacity evicts
        // the least-recently-seen entry so a genuinely present neighbor is
        // never locked out by stale message-path senders (issue #140).
        if let Some(last_seen) = self.known_peers.get_mut(peer_id) {
            *last_seen = Instant::now();
        } else {
            if self.known_peers.len() >= MAX_KNOWN_PEERS {
                if let Some(victim) = self
                    .known_peers
                    .iter()
                    .min_by_key(|(_, seen)| **seen)
                    .map(|(id, _)| id.clone())
                {
                    debug!(peer_id = %peer_id, evicted = %victim, cap = MAX_KNOWN_PEERS, "Known peers at capacity, evicting least-recently-seen");
                    self.evict_known_peer(&victim);
                }
            }
            self.known_peers.insert(peer_id.to_string(), Instant::now());
        }

        // Flush any pending outbox messages destined for this peer
        self.flush_outbox_for_peer_via(peer_id, unpark_via);

        // A Welcome that stalled or expired while this peer was unreachable now
        // has a fresh delivery opportunity over the carrier that surfaced this
        // peer — re-arm it. No-op when there is no pending Welcome for the peer.
        self.rearm_welcome_for_peer(peer_id, "peer_rediscovered");

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

        if let Err(e) = self.send_key_package_to(peer_id, false) {
            warn!(error = %e, peer_id = %peer_id, "Failed to send key package on discovery");
        }
    }

    /// Immediately attempts to send all pending outbox messages destined
    /// for a specific peer, bypassing backoff timers. Called on per-peer
    /// reachability edges (discovery, relay presence-online) to flush
    /// messages that were queued while the peer was unreachable.
    ///
    /// `unpark_via` is an optional transport override for the re-driven
    /// sends: `Some(Internet)` on the relay presence-online edge
    /// (`on_peer_presence`, including its rescue branch's re-entry through
    /// `on_neighbor_discovered_via`): the reachability
    /// proof is relay-scoped, so the re-drive must go out over the internet
    /// transport — DORS could otherwise route it back into the mesh, where
    /// the send locally succeeds against a peer that is not there,
    /// re-registering an unanswerable ACK that re-strands the message (and,
    /// with the platform bridge unwatching the peer on the online answer,
    /// no further presence edge would arrive to save it). Pinned media
    /// transports still win over the override.
    fn flush_outbox_for_peer_via(&mut self, peer_id: &str, unpark_via: Option<TransportType>) {
        // Probe ACKs first: with a live park counter, the peer's DMs may be
        // mid-probe — awaiting a probe ACK that may never arrive (never, over
        // a mesh carrier) — and the awaiting-ACK filter below would skip
        // exactly the messages this edge exists to re-drive.
        self.cancel_probe_state_for_parked_peer(peer_id);
        // A per-peer reachability edge: the escalating unreachable-probe
        // interval starts over (mirrors rearm_welcome_for_peer resetting
        // unreachable_parks). Snapshotted, not dropped: an edge whose
        // re-drives all fail didn't actually take, and restores it below.
        let prior_parks = self.dm_unreachable_parks.remove(peer_id);

        // Collect matching messages from outbox + media_outbox
        let mut to_send: Vec<(Message, u32)> = Vec::new();

        for entry in self.outbox.values() {
            if entry.message.recipient.as_str() == peer_id
                && !self.ack_manager.is_waiting_for_ack(&entry.message.id)
            {
                to_send.push((entry.message.clone(), entry.attempt_count));
            }
        }
        for entry in self.media_outbox.values() {
            if entry.message.recipient.as_str() == peer_id
                && !self.ack_manager.is_waiting_for_ack(&entry.message.id)
            {
                to_send.push((entry.message.clone(), entry.attempt_count));
            }
        }

        if to_send.is_empty() {
            return;
        }

        let count = to_send.len().min(crate::constants::FLUSH_BATCH_LIMIT);
        debug!(peer_id = %peer_id, count = count, "Flushing outbox for discovered peer");

        let mut iter = to_send.into_iter();
        let mut parked_dm_seen = false;
        let mut parked_dm_redriven = false;

        // Only process up to FLUSH_BATCH_LIMIT messages per edge.
        for (message, attempt_count) in iter.by_ref().take(crate::constants::FLUSH_BATCH_LIMIT) {
            let parkable = self.is_parkable_plain_dm(&message.id);
            // Remove from retry queue since we're sending immediately
            self.retry_queue.remove(&message.id.as_str());
            let sent = self.try_flush_send_via(message, attempt_count, unpark_via);
            if parkable {
                parked_dm_seen = true;
                parked_dm_redriven |= sent.is_some();
            }
        }

        // Re-enqueue the overflow (mirrors flush_outbox_all): the probe
        // cancel above may have stripped a parked message's retry entry, so
        // "still holds its backoff timer" cannot be assumed past the batch
        // limit. Enqueue dedupes by id — entries still scheduled keep their
        // existing timer, canceled ones get a fresh backoff slot instead of
        // stranding edge-only with the park counter cleared.
        for (message, attempt_count) in iter {
            parked_dm_seen = parked_dm_seen || self.is_parkable_plain_dm(&message.id);
            let _ = self.retry_queue.enqueue(message, attempt_count);
        }

        // An edge that re-drove no parkable DM at all didn't actually take:
        // the DMs now sit in the retry queue, whose later sends are
        // DORS-routed (no transport override there), and a send that succeeds
        // locally without reaching the peer would re-register an ACK nothing
        // will answer. Restore the counter so exhaustion stays within
        // `try_repark_exhausted_dm`'s reach instead of settling terminally
        // with the counter cleared.
        //
        // Restore granularity is per-peer, matching the counter: one
        // successful re-drive clears it even when a sibling's send failed on
        // the same edge, leaving that sibling on DORS-routed backoff exposed
        // to terminal exhaustion. Accepted gap — it requires one transport
        // to both succeed and fail within a single flush loop.
        if let Some(parks) = prior_parks {
            if parked_dm_seen && !parked_dm_redriven {
                self.dm_unreachable_parks.insert(peer_id.to_string(), parks);
            }
        }
    }

    /// Immediately attempts to send all pending outbox messages across all peers.
    ///
    /// Called when a transport becomes available (e.g. internet reconnects) to
    /// flush all queued messages, bypassing backoff timers.
    pub fn flush_outbox_all(&mut self) {
        // Probe ACKs first (see `flush_outbox_for_peer_via`): mid-probe DMs
        // are awaiting unanswerable mesh ACKs, and the awaiting-ACK guard
        // below would strand them past this edge with their counters
        // cleared. Canceled entries surface through the stranded-outbox
        // collection.
        let parked_peers: Vec<String> = self.dm_unreachable_parks.keys().cloned().collect();
        for peer_id in parked_peers {
            self.cancel_probe_state_for_parked_peer(&peer_id);
        }
        // A carrier-level reachability edge (reconnect / start): every
        // escalating unreachable-probe interval starts over. Snapshotted,
        // not dropped: a peer whose re-drives all fail gets its counter
        // restored below (see `flush_outbox_for_peer_via`).
        let prior_parks = std::mem::take(&mut self.dm_unreachable_parks);

        // Drain all entries from the retry queue (ignores timing)
        let retry_entries = self.retry_queue.drain_all();

        // Also collect outbox entries NOT in the retry queue (stranded after
        // previous max_retries rejection)
        let mut all_messages: Vec<(Message, u32)> = retry_entries
            .into_iter()
            .map(|e| (e.message, e.retry_count))
            .collect();

        // Add outbox entries that weren't in the retry queue (stranded) AND
        // aren't already waiting for an ACK (successfully sent, just awaiting
        // confirmation). Without the ACK check, a prior flush that succeeded
        // would cause these messages to be re-sent unnecessarily.
        let retry_ids: std::collections::HashSet<String> =
            all_messages.iter().map(|(m, _)| m.id.as_str()).collect();

        for entry in self.outbox.values() {
            if !retry_ids.contains(&entry.message.id.as_str())
                && !self.ack_manager.is_waiting_for_ack(&entry.message.id)
            {
                all_messages.push((entry.message.clone(), entry.attempt_count));
            }
        }
        for entry in self.media_outbox.values() {
            if !retry_ids.contains(&entry.message.id.as_str())
                && !self.ack_manager.is_waiting_for_ack(&entry.message.id)
            {
                all_messages.push((entry.message.clone(), entry.attempt_count));
            }
        }

        if all_messages.is_empty() {
            return;
        }

        let count = all_messages.len().min(crate::constants::FLUSH_BATCH_LIMIT);
        debug!(
            count = count,
            total = all_messages.len(),
            "Flushing all outbox messages"
        );

        let mut iter = all_messages.into_iter();
        let mut parked_dm_seen: std::collections::HashSet<String> =
            std::collections::HashSet::new();
        let mut parked_dm_redriven: std::collections::HashSet<String> =
            std::collections::HashSet::new();

        for (message, attempt_count) in iter.by_ref().take(crate::constants::FLUSH_BATCH_LIMIT) {
            let recipient = message.recipient.as_str().to_string();
            let parkable =
                prior_parks.contains_key(&recipient) && self.is_parkable_plain_dm(&message.id);
            let sent_via = self.try_flush_send(message, attempt_count);
            if parkable {
                // Unlike the per-peer edges (discovery carries mesh-scoped
                // proof, presence-online forces the internet transport), this
                // carrier-level edge re-drives via DORS with no per-peer
                // reachability proof: a mesh-routed send "locally succeeds"
                // into the mesh whether or not the peer is there, so only an
                // internet-routed send — the one that can earn a relay
                // verdict — counts as re-driven for counter-clearing. A
                // mesh-routed success keeps the counter restore-eligible;
                // if its ACK genuinely arrives, delivery prunes the counter.
                if sent_via == Some(TransportType::Internet) {
                    parked_dm_redriven.insert(recipient);
                } else {
                    parked_dm_seen.insert(recipient);
                }
            }
        }

        // Re-enqueue any messages beyond the batch limit so they aren't lost
        for (message, attempt_count) in iter {
            let recipient = message.recipient.as_str();
            if prior_parks.contains_key(recipient) && self.is_parkable_plain_dm(&message.id) {
                parked_dm_seen.insert(recipient.to_string());
            }
            let _ = self.retry_queue.enqueue(message, attempt_count);
        }

        // Restore the counter of every parked peer none of whose DMs were
        // re-driven on this edge (see `flush_outbox_for_peer_via`): their
        // retry-queue sends are DORS-routed, and a send that succeeds locally
        // without reaching the peer must leave exhaustion re-parkable rather
        // than terminal.
        for (peer_id, parks) in prior_parks {
            if parked_dm_seen.contains(&peer_id) && !parked_dm_redriven.contains(&peer_id) {
                self.dm_unreachable_parks.insert(peer_id, parks);
            }
        }
    }

    /// Peers the platform layer should watch via relay `CheckPresence`
    /// queries: every peer with an undelivered or session-unproven MLS
    /// welcome ([`Self::welcome_pending_peers`]) plus every recipient of an
    /// outbox message not currently awaiting an ACK — which includes DMs
    /// parked on a relay `recipient_unreachable` verdict. The relay's
    /// presence-online answer lands in [`Self::on_peer_presence`], whose
    /// `flush_outbox_for_peer_via` is what re-drives (un-parks) those messages,
    /// so the SDK — not the app — owns the "watch my DeliveryError
    /// recipients" duty. Bounded by the outbox cap (500 entries).
    pub fn presence_watch_peers(&self) -> Vec<String> {
        let mut peers = self.welcome_pending_peers();
        let mut seen: std::collections::HashSet<String> = peers.iter().cloned().collect();
        for entry in self.outbox.values() {
            if self.ack_manager.is_waiting_for_ack(&entry.message.id) {
                continue;
            }
            let recipient = entry.message.recipient.as_str();
            if seen.insert(recipient.to_string()) {
                peers.push(recipient.to_string());
            }
        }
        peers
    }

    /// Attempts to send a single message as part of a flush operation.
    ///
    /// Ensures the outbox entry exists before sending (it may have been evicted
    /// by capacity limits while the message sat in the retry queue). On success,
    /// registers ACK tracking and updates the outbox entry. On failure,
    /// re-enqueues the message to the retry queue with its current attempt count
    /// so backoff resumes. Returns the transport the send went out over, or
    /// `None` when it failed (or, defensively, when the transport could not
    /// be attributed — the park restore logic treats both as not-redriven,
    /// the safe direction; in practice a successful send always has one).
    fn try_flush_send(&mut self, message: Message, attempt_count: u32) -> Option<TransportType> {
        self.try_flush_send_via(message, attempt_count, None)
    }

    /// [`Self::try_flush_send`] with an optional transport override (see
    /// [`Self::flush_outbox_for_peer_via`]). A pinned media transport takes
    /// precedence over the override; without either, DORS picks.
    fn try_flush_send_via(
        &mut self,
        mut message: Message,
        attempt_count: u32,
        transport_override: Option<TransportType>,
    ) -> Option<TransportType> {
        self.ensure_outbox_entry(&message);
        // Tier 2: re-seal against the peer's current session before flushing, so
        // a post-re-key epoch change no longer leaves the bytes undecryptable.
        // No-op for media/plaintext/unconfirmed sessions.
        self.reseal_for_resend_in_place(&mut message);
        let forced_transport = self
            .pinned_media_transport_for_message(&message.id)
            .or(transport_override);
        let send_result = if let Some(transport) = forced_transport {
            self.transport_manager
                .send_via_transport(&message, transport)
        } else {
            self.transport_manager.send(&message)
        };
        let current_transport =
            forced_transport.or_else(|| self.transport_manager.current_transport());

        match send_result {
            Ok(()) => {
                if let Err(e) = self.ensure_ack_registration(&message) {
                    warn!(message_id = %message.id, error = %e, "ACK registration failed during flush");
                }
                self.mark_message_sent(
                    &message,
                    current_transport,
                    Some(attempt_count.saturating_add(1)),
                );
                debug!(message_id = %message.id, "Flush send succeeded");
                current_transport
            }
            Err(e) => {
                let _ = self.retry_queue.enqueue(message.clone(), attempt_count);
                debug!(message_id = %message.id, error = %e, "Flush send failed, re-enqueued");
                None
            }
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
        self.evict_known_peer(peer_id);
    }

    /// Drops a peer from discovery tracking. Shared by explicit neighbor
    /// loss, the TTL sweep, and least-recently-seen eviction at capacity —
    /// all three mean "treat this peer as gone until re-seen", so all three
    /// also clear the key-package marker, letting a re-appearing peer
    /// receive a fresh key package exactly as on a BLE reconnect.
    fn evict_known_peer(&mut self, peer_id: &str) {
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
        if self.is_user_blocked(peer_id) {
            return Err(Error::UserBlocked(peer_id.to_string()));
        }

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

                // Send a fresh key package so the peer has one available for
                // group invites (the original was consumed to create this session).
                if self.config.encryption.enabled {
                    let _ = self.send_key_package_to(peer_id, false);
                }

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

    /// Whether rich extras (reply context, rich media metadata, forward
    /// attribution) on a send toward `peer_id` would travel in the sealed
    /// rich body — i.e. the peer's last key package advertised the
    /// capability (surviving restarts) and our own `rich_payload_enabled`
    /// kill switch is on. When `false` the extras are silently dropped and
    /// the message degrades to plain text, so apps can use this to render
    /// honest degraded-reply UX instead of discovering the drop after the
    /// fact.
    pub fn peer_supports_rich_payload(&self, peer_id: &str) -> bool {
        self.rich_seal_active(peer_id)
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
        // Drop the both-create owner gate too (memory + storage). A leaked gate
        // entry would make `can_confirm_from_source` reject every non-decrypt
        // confirmation source — including `welcome_received` — on the NEXT
        // session with this peer, re-stranding it in Pending (and surviving
        // restart via `restore_both_create_awaiting_decrypt`).
        self.clear_both_create_awaiting_decrypt(peer_id);
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
            manager.decrypt_from_user(encrypted, &encrypted.sender_id)?
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
            manager.decrypt(encrypted, &encrypted.sender_id)?
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

    /// Processes internal MLS protocol messages.
    ///
    /// Returns `Some(InternalMessageResult::Consumed)` if the message was an internal
    /// protocol message that should not be surfaced to the application.
    /// Returns `Some(InternalMessageResult::Decrypted(plaintext))` if the message was
    /// encrypted and successfully decrypted.
    /// Returns `None` if the message is not an internal message.
    pub(crate) fn process_internal_message(
        &mut self,
        message: &Message,
    ) -> Option<InternalMessageResult> {
        self.process_internal_message_via(message, None)
    }

    /// [`Self::process_internal_message`] with the transport the frame
    /// arrived on, when the caller knows it. Handlers that trust relay-server
    /// answers (`__GROUP_CREATED__` setting `relay_synced`) require the
    /// Internet transport; with `None` they treat the frame as untrusted.
    pub(crate) fn process_internal_message_via(
        &mut self,
        message: &Message,
        arrival_transport: Option<TransportType>,
    ) -> Option<InternalMessageResult> {
        let content = &message.content;

        // Run the security gate for control messages (transport identity +
        // signature verification). Returns `Some(Consumed)` to drop the
        // message, or `None` to proceed.
        if let Some(result) = self.security_gate_control_message(message) {
            return Some(result);
        }

        let sender = message.sender.as_str();

        // Handle key package messages
        if let Some(data) = content.strip_prefix(internal_prefixes::KEY_PACKAGE) {
            self.handle_key_package_message(sender, data);
            return Some(InternalMessageResult::Consumed);
        }

        if content.starts_with(internal_prefixes::SESSION_CONFIRM_PROBE) {
            self.handle_session_confirm_probe(sender, content);
            return Some(InternalMessageResult::Consumed);
        }

        if content.starts_with(internal_prefixes::SESSION_CONFIRM_ACK) {
            self.handle_session_confirm_ack(sender, content);
            return Some(InternalMessageResult::Consumed);
        }

        // Handle welcome messages (session invitation)
        if let Some(data) = content.strip_prefix(internal_prefixes::WELCOME) {
            self.handle_welcome_message(sender, data);
            return Some(InternalMessageResult::Consumed);
        }

        // Handle encrypted messages
        if let Some(data) = content.strip_prefix(internal_prefixes::ENCRYPTED) {
            if let Some(result) =
                self.handle_encrypted_message(sender, data, message, arrival_transport)
            {
                return Some(result);
            }
            return Some(InternalMessageResult::Consumed);
        }

        // Handle connection request messages
        if let Some(data) = content.strip_prefix(internal_prefixes::CONN_REQUEST) {
            self.handle_connection_request(sender, data);
            return Some(InternalMessageResult::Consumed);
        }

        // Handle connection accepted messages
        if let Some(data) = content.strip_prefix(internal_prefixes::CONN_ACCEPT) {
            self.handle_connection_accepted(sender, data);
            return Some(InternalMessageResult::Consumed);
        }

        // Handle connection rejected messages
        if content
            .strip_prefix(internal_prefixes::CONN_REJECT)
            .is_some()
        {
            self.handle_connection_rejected(sender);
            return Some(InternalMessageResult::Consumed);
        }

        // Handle connection cancelled messages
        if content
            .strip_prefix(internal_prefixes::CONN_CANCEL)
            .is_some()
        {
            self.handle_connection_cancelled(sender);
            return Some(InternalMessageResult::Consumed);
        }

        // --- Presence, typing, and read receipt messages ---

        if let Some(data) = content.strip_prefix(internal_prefixes::PRESENCE) {
            self.handle_presence_message(sender, data);
            return Some(InternalMessageResult::Consumed);
        }

        if let Some(data) = content.strip_prefix(internal_prefixes::TYPING_INDICATOR) {
            self.handle_typing_indicator(sender, data);
            return Some(InternalMessageResult::Consumed);
        }

        if let Some(data) = content.strip_prefix(internal_prefixes::READ_RECEIPT) {
            self.handle_read_receipt(sender, data);
            return Some(InternalMessageResult::Consumed);
        }

        // --- Group (mesh/MLS) messages ---

        if let Some(data) = content.strip_prefix(internal_prefixes::GROUP_MLS_MSG) {
            return Some(self.handle_group_mls_msg_via(message, sender, data, arrival_transport));
        }

        if let Some(data) = content.strip_prefix(internal_prefixes::GROUP_MLS_WELCOME) {
            let mid = message.id.as_str();
            self.handle_group_mls_welcome(&mid, sender, data);
            return Some(InternalMessageResult::Consumed);
        }

        if let Some(data) = content.strip_prefix(internal_prefixes::GROUP_MLS_COMMIT) {
            let mid = message.id.as_str();
            self.handle_group_mls_commit(&mid, sender, data);
            return Some(InternalMessageResult::Consumed);
        }

        if let Some(data) = content.strip_prefix(internal_prefixes::GROUP_MLS_LEAVE) {
            let mid = message.id.as_str();
            self.handle_group_mls_leave(&mid, sender, data);
            return Some(InternalMessageResult::Consumed);
        }

        if let Some(data) = content.strip_prefix(internal_prefixes::GROUP_ROLE_CHANGE) {
            let mid = message.id.as_str();
            self.handle_group_role_change(&mid, sender, data);
            return Some(InternalMessageResult::Consumed);
        }

        if let Some(data) = content.strip_prefix(internal_prefixes::GROUP_RENAME) {
            let mid = message.id.as_str();
            self.handle_group_rename(&mid, sender, data);
            return Some(InternalMessageResult::Consumed);
        }

        // --- Group (relay) and service messages ---

        if content.starts_with(internal_prefixes::GROUP_CREATED)
            || content.starts_with(internal_prefixes::GROUP_MSG)
            || content.starts_with(internal_prefixes::GROUP_MEMBER_ADDED)
            || content.starts_with(internal_prefixes::GROUP_MEMBER_REMOVED)
            || content.starts_with(internal_prefixes::GROUP_INFO)
            || content.starts_with(internal_prefixes::USER_GROUPS)
            || content.starts_with(internal_prefixes::GROUP_ERROR)
        {
            self.handle_group_relay_message(sender, content, arrival_transport);
            return Some(InternalMessageResult::Consumed);
        }

        // --- Service discovery & request/response ---
        if content.starts_with(offline_protocol_services::SVC_MESSAGE_PREFIX) {
            self.handle_service_message(sender, content, message);
            return Some(InternalMessageResult::Consumed);
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
        self.process_relay_register_retries();
        self.process_timed_out_acks()?;

        // Throttle session reconciliation to avoid expensive storage I/O
        // (list_sessions → Keychain/Keystore) on every tick. Only run when
        // there's pending work AND enough time has elapsed since the last run.
        self.run_throttled_reconciliation("process_tick");

        let _ = self.prune_expired_pending_global_front(Instant::now(), 256);
        self.pump_media_transfers();
        self.cleanup_expired_entries();
        self.evaluate_relay_role();
        self.tick_telemetry_categories();

        Ok(())
    }

    /// Re-evaluates the local relay role against current connectivity and
    /// battery and emits the corresponding transition event when the role
    /// actually changes (`RelayPromoted`, `RelayDemoted`, or
    /// `RelayDemotedBattery`).
    ///
    /// Runs every tick independently of telemetry: these are app-facing
    /// events delivered through the `EventCallback` channel, which is live
    /// whether or not a telemetry sink is installed. Mutating the role here
    /// also keeps the `DeviceCapabilitySnapshot.relay_role` signal honest,
    /// since `tick_telemetry_categories` reads the role immediately after.
    ///
    /// Skipped when the battery level is unknown: the promote/demote policy
    /// is battery-dependent, and transitioning on a phantom level would emit
    /// dishonest churn.
    fn evaluate_relay_role(&mut self) {
        let (_statuses, available) = self.transport_manager.snapshot_status_and_available();
        let (battery_level, is_charging) =
            device_battery_from_available(self.transport_manager.current_transport(), &available);
        let Some(battery_level) = battery_level else {
            return;
        };
        let connection_count = self.known_peers.len();
        let Some(transition) = self.path_selector.evaluate_relay_transition(
            connection_count,
            battery_level,
            is_charging,
        ) else {
            return;
        };
        let event = match transition {
            RelayTransition::Promoted {
                connection_count,
                battery_level,
            } => Event::relay_promoted(connection_count, battery_level),
            RelayTransition::Demoted(RelayDemotionReason::LowConnections) => {
                Event::relay_demoted("connections below relay threshold".to_string())
            }
            RelayTransition::Demoted(RelayDemotionReason::LowBattery { min_required }) => {
                Event::relay_demoted_battery(battery_level, min_required)
            }
            RelayTransition::Demoted(RelayDemotionReason::RelayDisallowed) => {
                Event::relay_demoted("relaying disabled by configuration".to_string())
            }
        };
        self.emit_event(event);
    }

    /// Per-tick telemetry work: diff transport statuses, diff device
    /// capability, and emit a `MetricsFrame` when `metrics_cadence` has
    /// elapsed. No-ops unless a sink has been installed.
    ///
    /// Runs at the end of `process()` so reliability/queue state observed
    /// here is the same state the rest of the tick has already advanced.
    /// Snapshots `get_available_transports()` exactly once per tick and
    /// reuses the map across every downstream helper that needs it.
    ///
    /// If an earlier step in `process()` returns `Err(...)`, this method is
    /// skipped and telemetry emission for that interval is deferred to the
    /// next successful tick — the cadence guarantee relaxes under sustained
    /// partial failure.
    ///
    /// # Sink-panic semantics
    ///
    /// Each per-tick cursor is advanced *before* the emit call that
    /// observes it, so a panicking sink does not cause re-delivery of the
    /// record it panicked on. The transport-status path advances
    /// **per-transport**: when multiple transitions fall in a single tick,
    /// a panic on transition K commits K's entry (at-most-once for K) but
    /// leaves entries L..N in `transport_status_snapshot` at their previous
    /// values, so the next tick re-diffs and emits them fresh. This avoids
    /// silent data loss for transitions that never reached the sink.
    ///
    /// The device-capability and metrics-frame paths emit at most one
    /// record per tick each, so a simpler "advance then emit" discipline
    /// suffices there: a panic advances the cursor and the same record is
    /// not re-emitted on the next tick.
    fn tick_telemetry_categories(&mut self) {
        let Some(ctx) = self.telemetry.clone() else {
            return;
        };
        let now_ms = Utc::now().timestamp_millis();
        let now = Instant::now();

        // One lock-per-transport pass: get statuses (for the transition
        // diff) and the available-only metrics map (for the metrics frame
        // and the device-capability diff). Reused across every helper
        // below.
        let (current_statuses, available) = self.transport_manager.snapshot_status_and_available();

        // Transport-status diff: one record per changed transport. Commit
        // each transport's new snapshot entry *before* emitting its event
        // so a panic on event K is at-most-once for K; untouched entries
        // L..N stay at their previous values and get re-diffed next tick.
        let transitions =
            diff_transport_state(now_ms, &self.transport_status_snapshot, &current_statuses);
        for event in transitions {
            let transport = event.transport;
            match current_statuses.get(&transport) {
                Some(status) => {
                    self.transport_status_snapshot.insert(transport, *status);
                }
                None => {
                    self.transport_status_snapshot.remove(&transport);
                }
            }
            dispatch_record(&ctx.sink, &TelemetryRecord::TransportState(event));
        }

        // Device capability diff. At-most-one emission per tick, so the
        // simple advance-before-emit pattern preserves at-most-once.
        let (battery_level, is_charging) =
            device_battery_from_available(self.transport_manager.current_transport(), &available);
        let relay_role = self.path_selector.current_relay_role();
        let device_now = DeviceSnap::from_parts(battery_level, is_charging, relay_role);
        let device_change =
            diff_device_capability(now_ms, self.device_capability_snapshot, device_now);
        self.device_capability_snapshot = Some(device_now);
        if let Some(snapshot) = device_change {
            dispatch_record(&ctx.sink, &TelemetryRecord::Device(snapshot));
        }

        // Periodic metrics snapshot. Rearm cadence before emit so a
        // panicking sink doesn't cause an immediate retry next tick.
        if let Some(cadence) = ctx.config.metrics_cadence() {
            let due = match self.last_metrics_emit_at {
                None => true,
                Some(prev) => now.saturating_duration_since(prev) >= cadence,
            };
            if due {
                let is_local_relay = matches!(relay_role, RelayRole::Relay);
                let frame = build_metrics_frame(
                    now_ms,
                    self.transport_manager.current_transport(),
                    &available,
                    &self.retry_queue,
                    &self.deduplicator,
                    &self.ack_manager,
                    self.known_peers.len(),
                    is_local_relay,
                );
                self.last_metrics_emit_at = Some(now);
                dispatch_record(
                    &ctx.sink,
                    &TelemetryRecord::MetricsSnapshot(Box::new(frame)),
                );
            }
        }
    }

    /// Processes messages ready for retry from the retry queue.
    ///
    /// EDGE CASE HANDLING:
    /// - Checks transport availability before each retry attempt
    /// - Handles transport switch mid-retry
    /// - Properly tracks retry counts and transport failures
    fn process_retry_queue(&mut self) -> Result<()> {
        // Limit batch size to prevent blocking on large queues
        let max_batch_size = crate::constants::FLUSH_BATCH_LIMIT;
        let mut processed = 0;

        while processed < max_batch_size {
            let mut entry = match self.retry_queue.dequeue_ready() {
                Some(e) => e,
                None => break,
            };

            processed += 1;
            let previous_transport = self.transport_manager.current_transport();
            self.ensure_outbox_entry(&entry.message);

            // Tier 2: re-seal against the peer's current session before this
            // resend, so a post-re-key epoch change no longer leaves the bytes
            // undecryptable. No-op for media/plaintext/unconfirmed sessions.
            self.reseal_for_resend_in_place(&mut entry.message);

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
                    // Re-enqueue with incremented retry count for backoff
                    let next_retry_at = self
                        .retry_queue
                        .enqueue(entry.message.clone(), entry.retry_count + 1);

                    if let Some(transport) = forced_transport.or(previous_transport) {
                        self.transport_manager.record_retry_failure(transport);
                    }

                    // Surface the schedule so apps can show retry state
                    // instead of inferring it from silence. None = the id
                    // was already queued; the earlier emission stands.
                    if let Some(retry_at) = next_retry_at {
                        self.emit_event(Event::message_retrying(
                            entry.message.id.clone(),
                            entry.message.recipient.as_str().to_string(),
                            entry.retry_count + 1,
                            retry_at.timestamp_millis(),
                        ));
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
        // A plain DM whose recipient still holds a live unreachable-park
        // counter re-parks instead of settling: the exhausted budget was burnt
        // by reachability probes — sends that succeed locally without proving
        // the peer is back — not by a peer believed reachable (see
        // try_repark_exhausted_dm).
        if self.try_repark_exhausted_dm(message_id) {
            return Ok(());
        }

        // Retry exhaustion is terminal for a connection request: settle the
        // pending entry and surface the typed undeliverable signal alongside
        // the generic message_failed, so apps keep connection-request
        // context without parsing reason strings.
        let undeliverable_recipient = self.take_undeliverable_connection_request(message_id);

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
        if let Some(recipient) = undeliverable_recipient {
            warn!(
                recipient = %recipient,
                message_id = %message_id,
                "Connection request undeliverable: max retries exceeded"
            );
            state.emit_event(Event::connection_request_undeliverable(
                recipient,
                message_id.as_str(),
                "max_retries_exceeded".to_string(),
            ));
        }
        drop(state);

        self.handle_outbound_media_chunk_failed(message_id, "max retries exceeded");
        self.retry_queue.remove(&message_id.as_str());
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
            let recipient = entry.message.recipient.as_str().to_string();
            let last_transport = entry.last_transport;

            // enqueue is infallible (retry queue has no attempt limit)
            let next_retry_at = self.retry_queue.enqueue(message_clone, retry_count);
            if let Some(transport) = last_transport {
                self.transport_manager.record_retry_failure(transport);
            }
            if let Some(retry_at) = next_retry_at {
                self.emit_event(Event::message_retrying(
                    message_id.clone(),
                    recipient,
                    retry_count,
                    retry_at.timestamp_millis(),
                ));
            }
        } else {
            self.handle_missing_outbox_entry(message_id, retry_count)?;
        }
        Ok(())
    }

    /// Emits a protocol event to all registered handlers.
    ///
    /// Silently no-ops if the shared state lock is poisoned.
    pub(crate) fn emit_event(&self, event: Event) {
        if let Ok(state) = lock_shared_state(&self.shared_state) {
            state.emit_event(event);
        }
    }

    /// Emits a terminal settlement, or parks it until `start()` if the event
    /// pipeline is not live yet.
    ///
    /// Restore paths settle messages the app is still holding ids for. They run
    /// from `initialize_mls`, before `start()` and often before the app has
    /// installed its event callback, so emitting directly there would silently
    /// discard exactly the signals that exist to stop an id from hanging
    /// forever. Once running, this is a plain emit — the same call is used from
    /// `process()`-driven expiry, where no deferral is wanted.
    pub(crate) fn settle_restored_message_failure(&mut self, event: Event) {
        let running = self.event_pipeline_is_live();
        self.settle_one_restored_message_failure(event, running);
    }

    /// Settles a batch, taking the shared-state lock once rather than once per
    /// event. Restore emits these in loops that a tampered or pre-split store
    /// can drive into the thousands.
    pub(crate) fn settle_restored_message_failures(
        &mut self,
        events: impl IntoIterator<Item = Event>,
    ) {
        let running = self.event_pipeline_is_live();
        for event in events {
            self.settle_one_restored_message_failure(event, running);
        }
    }

    fn event_pipeline_is_live(&self) -> bool {
        lock_shared_state(&self.shared_state)
            .map(|state| state.state == ProtocolState::Running)
            .unwrap_or(false)
    }

    fn settle_one_restored_message_failure(&mut self, event: Event, running: bool) {
        if running {
            self.emit_event(event);
            return;
        }
        // Every other accumulation on the restore path has an explicit ceiling
        // and logs what it dropped; this one is retained until `start()`, which
        // may never come, so it gets the same treatment. Keeping the *oldest*
        // is deliberate: the settlements a restore produces first are the ones
        // for records it examined first, and dropping those in favour of later
        // ones would bias the survivors by backend listing order.
        if self.deferred_restore_settlements.len() >= MAX_DEFERRED_RESTORE_SETTLEMENTS {
            self.suppressed_restore_settlements += 1;
            return;
        }
        self.deferred_restore_settlements.push(event);
    }

    /// Returns an iterator over outbox messages (test-only).
    #[cfg(test)]
    pub(crate) fn outbox_messages(&self) -> impl Iterator<Item = &Message> {
        self.outbox.values().map(|e| &e.message)
    }

    /// Returns the total number of entries across outbox and media_outbox (test-only).
    #[cfg(test)]
    pub(crate) fn outbox_entry_count(&self) -> usize {
        self.outbox.len() + self.media_outbox.len()
    }

    /// Clears all outbox entries (test-only).
    #[cfg(test)]
    pub(crate) fn clear_outbox(&mut self) {
        self.outbox.clear();
    }

    /// Returns a reference to the MLS manager Arc (test-only).
    #[cfg(test)]
    pub(crate) fn mls_manager_for_testing(&self) -> &Arc<RwLock<offline_protocol_mls::MlsManager>> {
        self.mls_manager
            .as_ref()
            .expect("MLS not initialized in test")
    }

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
}

#[cfg(test)]
pub(crate) mod tests;
