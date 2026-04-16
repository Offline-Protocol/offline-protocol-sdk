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
mod storage;
mod types;

pub(crate) use decryption_queue::PendingDecryptionQueue;
pub use decryption_queue::PendingQueueMetrics;
pub(crate) use prefixes::*;
pub use types::ProtocolState;
pub(crate) use types::*;

use crate::file_transfer::{FileTransferManager, OutboundTransferState};
use crate::mls_observability::{MlsEventEmitter, MlsEventRateLimiter, NoopMlsEventEmitter};
use crate::{Error, EstablishmentState, Event, ProtocolConfig, Result, TransportManager};
use chrono::{DateTime, Utc};
use offline_protocol_core::{LamportClock, Message, MessageId};
use offline_protocol_mls::{EncryptedMessage, MlsManager, MlsStorage, WelcomeMessage};
use offline_protocol_reliability::{AckManager, Deduplicator, RetryQueue};
use offline_protocol_router::{PathSelector, RelayManager, TransportSelector};
use offline_protocol_services::MeshServices;
use offline_protocol_transport::{BleTransport, TransportType};
use std::collections::HashMap;
use std::collections::HashSet;
use std::sync::{Arc, Mutex, RwLock};
use std::time::Instant;
use tracing::{debug, error, info, warn};

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
    deduplicator: Deduplicator,

    /// Shared mutable state.
    shared_state: Arc<Mutex<SharedState>>,

    /// Messages awaiting delivery/acknowledgment (store-and-forward outbox).
    outbox: HashMap<MessageId, OutboxEntry>,

    /// Dedicated outbox for file chunk messages, separate from the main outbox
    /// to prevent large file transfers from evicting regular messages.
    media_outbox: HashMap<MessageId, OutboxEntry>,

    /// MLS manager for end-to-end encryption.
    mls_manager: Option<Arc<RwLock<MlsManager>>>,

    /// Pending messages waiting for session establishment (recipient -> messages).
    pending_encrypted_messages: HashMap<String, Vec<PendingMessage>>,

    /// Key packages received but not yet used (sender_id -> package).
    pub(crate) pending_key_packages: HashMap<String, ReceivedKeyPackage>,

    /// Set of peers we've already sent our key package to.
    pub(crate) key_package_sent_to: std::collections::HashSet<String>,

    /// All discovered/connected peers, tracked independently of encryption.
    /// Used by service discovery to know who to broadcast queries to.
    known_peers: std::collections::HashSet<String>,

    /// Sessions confirmed established (received Welcome or successful decrypt).
    /// Only encrypt messages when the session is confirmed to avoid race conditions.
    confirmed_sessions: std::collections::HashSet<String>,

    /// Bounded pending decryption queue for encrypted messages received before
    /// the MLS session is ready.
    pub(crate) pending_queue: PendingDecryptionQueue,

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
    /// dropped (no ACK, no event). Persisted via `MlsStorage`.
    blocked_users: HashSet<String>,

    /// Timestamp of the last `kick_pending_session_reconciliation` execution.
    /// Used to throttle reconciliation to avoid expensive storage I/O
    /// (list_sessions → Keychain/Keystore) on every process tick / receive poll.
    last_reconciliation_at: Option<Instant>,

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
            pending_queue: PendingDecryptionQueue::default(),
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
            group_mesh: crate::group_mesh::GroupMeshState::default(),
            known_peer_public_keys: HashMap::new(),
            blocked_users: HashSet::new(),
            last_reconciliation_at: None,
            last_persisted_lamport: 0,
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
        let previous_tofu_keys = self.known_peer_public_keys.clone();
        let previous_blocked_users = self.blocked_users.clone();

        // Also use this storage for pending message persistence
        self.message_storage = Some(storage);

        // Restore state from previous session
        let restore_result = (|| {
            self.restore_pending_messages()?;
            self.restore_lamport_clock();
            self.restore_tofu_keys();
            self.restore_blocked_users();
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
            self.known_peer_public_keys = previous_tofu_keys;
            self.blocked_users = previous_blocked_users;
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
        self.restore_tofu_keys();
        self.restore_blocked_users();
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

        // Wire BLE fragment eviction callback so app receives FragmentAssemblyEvicted.
        if let Some(ble_arc) = self.transport_manager.get_transport(TransportType::BLE) {
            let shared = self.shared_state.clone();
            if let Ok(transport) = ble_arc.lock() {
                if let Some(ble) = transport.as_any().downcast_ref::<BleTransport>() {
                    ble.set_fragment_eviction_callback(Some(Arc::new(move |info| {
                        if let Ok(s) = shared.lock() {
                            s.emit_event(Event::fragment_assembly_evicted(
                                info.message_id,
                                info.completion_percent,
                                "capacity".to_string(),
                            ));
                        }
                    })));
                }
            }
        }

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

        // Flush debounced Lamport clock before stopping so no ticks are lost.
        drop(state);
        self.flush_lamport_clock();

        // Clear event callbacks to release shared_state references.
        self.transport_manager.set_dors_event_callback(None);
        if let Some(ble_arc) = self.transport_manager.get_transport(TransportType::BLE) {
            if let Ok(transport) = ble_arc.lock() {
                if let Some(ble) = transport.as_any().downcast_ref::<BleTransport>() {
                    ble.set_fragment_eviction_callback(None);
                }
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
        let mut state = lock_shared_state(&self.shared_state)?;

        if state.state != ProtocolState::Paused {
            return Err(Error::InvalidConfiguration(
                "Protocol is not paused".to_string(),
            ));
        }

        state.state = ProtocolState::Running;
        Ok(())
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

        // Don't track or auto-exchange keys with blocked users
        if self.is_user_blocked(peer_id) {
            debug!(peer_id = %peer_id, "Ignoring neighbor discovery for blocked user");
            return;
        }

        // Track discovered peers for service discovery and routing, with capacity limit
        if self.known_peers.len() < MAX_KNOWN_PEERS || self.known_peers.contains(peer_id) {
            self.known_peers.insert(peer_id.to_string());
        } else {
            debug!(peer_id = %peer_id, cap = MAX_KNOWN_PEERS, "Known peers at capacity, not tracking new peer");
        }

        // Flush any pending outbox messages destined for this peer
        self.flush_outbox_for_peer(peer_id);

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

    /// Immediately attempts to send all pending outbox messages destined for a specific peer.
    ///
    /// Called when a peer is discovered to flush messages that were queued while
    /// the peer was unreachable, bypassing backoff timers.
    fn flush_outbox_for_peer(&mut self, peer_id: &str) {
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

        // Only process up to FLUSH_BATCH_LIMIT messages. Overflow messages
        // are not removed from the retry queue, so they retain their backoff
        // timers and will be picked up on the next process() tick or flush.
        for (message, attempt_count) in to_send
            .into_iter()
            .take(crate::constants::FLUSH_BATCH_LIMIT)
        {
            // Remove from retry queue since we're sending immediately
            self.retry_queue.remove(&message.id.as_str());
            self.try_flush_send(message, attempt_count);
        }
    }

    /// Immediately attempts to send all pending outbox messages across all peers.
    ///
    /// Called when a transport becomes available (e.g. internet reconnects) to
    /// flush all queued messages, bypassing backoff timers.
    pub fn flush_outbox_all(&mut self) {
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

        for (message, attempt_count) in iter.by_ref().take(crate::constants::FLUSH_BATCH_LIMIT) {
            self.try_flush_send(message, attempt_count);
        }

        // Re-enqueue any messages beyond the batch limit so they aren't lost
        for (message, attempt_count) in iter {
            self.retry_queue.enqueue(message, attempt_count);
        }
    }

    /// Attempts to send a single message as part of a flush operation.
    ///
    /// Ensures the outbox entry exists before sending (it may have been evicted
    /// by capacity limits while the message sat in the retry queue). On success,
    /// registers ACK tracking and updates the outbox entry. On failure,
    /// re-enqueues the message to the retry queue with its current attempt count
    /// so backoff resumes.
    fn try_flush_send(&mut self, message: Message, attempt_count: u32) {
        self.ensure_outbox_entry(&message);
        let forced_transport = self.pinned_media_transport_for_message(&message.id);
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
            }
            Err(e) => {
                self.retry_queue.enqueue(message.clone(), attempt_count);
                debug!(message_id = %message.id, error = %e, "Flush send failed, re-enqueued");
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
            if let Some(result) = self.handle_encrypted_message(sender, data, message) {
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
            self.handle_group_mls_msg(message, sender, data);
            return Some(InternalMessageResult::Consumed);
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
            self.handle_group_relay_message(sender, content);
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
        self.process_timed_out_acks()?;

        // Throttle session reconciliation to avoid expensive storage I/O
        // (list_sessions → Keychain/Keystore) on every tick. Only run when
        // there's pending work AND enough time has elapsed since the last run.
        self.run_throttled_reconciliation("process_tick");

        let _ = self.prune_expired_pending_global_front(Instant::now(), 256);
        self.pump_media_transfers();
        self.cleanup_expired_entries();

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
        let max_batch_size = crate::constants::FLUSH_BATCH_LIMIT;
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
                    // Re-enqueue with incremented retry count for backoff
                    self.retry_queue
                        .enqueue(entry.message.clone(), entry.retry_count + 1);

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
            let last_transport = entry.last_transport;

            // enqueue is infallible (retry queue has no attempt limit)
            self.retry_queue.enqueue(message_clone, retry_count);
            if let Some(transport) = last_transport {
                self.transport_manager.record_retry_failure(transport);
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
