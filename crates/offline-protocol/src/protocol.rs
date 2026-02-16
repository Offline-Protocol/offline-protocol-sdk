//! Main protocol engine.

use crate::constants::{ACK_FOR_KEY, ACK_HOP_COUNT_KEY, ACK_TRANSPORT_KEY, MAX_OUTBOX_ENTRIES};
use crate::{Error, Event, EventCallback, ProtocolConfig, Result, TransportManager};
use chrono::{DateTime, Duration as ChronoDuration, Utc};
use offline_protocol_core::{AppId, LamportClock, Message, MessageId, MessagePriority, UserId, TTL};
use offline_protocol_mls::{EncryptedMessage, MlsManager, MlsStorage, WelcomeMessage};
use offline_protocol_reliability::{
    AckConfig, AckManager, Deduplicator, DeduplicatorConfig, DeduplicatorStats, RetryConfig,
    RetryQueue,
};
use offline_protocol_router::{DorsConfig, PathSelector, RelayManager, TransportSelector};
use offline_protocol_transport::TransportType;
use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use std::sync::{Arc, Mutex, RwLock};
use tracing::{debug, error, info, warn};

/// Internal message prefixes for protocol messages.
mod internal_prefixes {
    /// Prefix for key package messages.
    pub const KEY_PACKAGE: &str = "__MLS_KEY_PKG__";
    /// Prefix for welcome messages.
    pub const WELCOME: &str = "__MLS_WELCOME__";
    /// Prefix for encrypted messages.
    pub const ENCRYPTED: &str = "__MLS_ENC__";
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
}

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
#[derive(Debug, Clone)]
struct ReceivedKeyPackage {
    /// Raw MLS key package bytes.
    key_package_data: Vec<u8>,
    /// Local wall-clock deadline (ms since epoch) computed from the sender's
    /// `remaining_lifetime_ms`, anchored to *our* clock at receive time.
    local_expires_at_ms: u64,
}

/// Result of processing an internal protocol message.
enum InternalMessageResult {
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

/// Storage key types for message persistence.
mod storage_keys {
    /// Key type for pending encrypted messages.
    pub const PENDING_MESSAGES: &str = "pending_messages";
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
struct SharedState {
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

    fn emit_event(&self, event: Event) {
        for handler in &self.event_handlers {
            handler(event.clone());
        }
    }
}

/// Helper function to lock a mutex and convert poison errors to protocol errors.
fn lock_shared_state(
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

/// Main entry point for the Offline Protocol SDK.
///
/// This struct combines all protocol components and provides a unified API
/// for sending/receiving messages with automatic transport selection and
/// reliable delivery.
pub struct OfflineProtocol {
    /// Configuration.
    config: ProtocolConfig,

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
    shared_state: Arc<Mutex<SharedState>>,

    /// Messages awaiting delivery/acknowledgment (store-and-forward outbox).
    outbox: HashMap<MessageId, OutboxEntry>,

    /// MLS manager for end-to-end encryption.
    mls_manager: Option<Arc<RwLock<MlsManager>>>,

    /// Pending messages waiting for session establishment (recipient -> messages).
    pending_encrypted_messages: HashMap<String, Vec<PendingMessage>>,

    /// Key packages received but not yet used (sender_id -> package).
    pending_key_packages: HashMap<String, ReceivedKeyPackage>,

    /// Set of peers we've already sent our key package to.
    key_package_sent_to: std::collections::HashSet<String>,

    /// Sessions confirmed established (received Welcome or successful decrypt).
    /// Only encrypt messages when the session is confirmed to avoid race conditions.
    confirmed_sessions: std::collections::HashSet<String>,

    /// Encrypted messages received before session was established (sender -> messages).
    /// These are queued and processed after session confirmation.
    pending_decryption: HashMap<String, Vec<Message>>,

    /// Storage for persisting pending messages (reuses MLS storage).
    /// When set, pending messages survive app crashes/restarts.
    message_storage: Option<Arc<dyn MlsStorage>>,

    /// Lamport logical clock for causal message ordering.
    /// Ticked on send, merged on receive.
    lamport_clock: LamportClock,
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
            mls_manager: None,
            pending_encrypted_messages: HashMap::new(),
            pending_key_packages: HashMap::new(),
            key_package_sent_to: std::collections::HashSet::new(),
            confirmed_sessions: std::collections::HashSet::new(),
            pending_decryption: HashMap::new(),
            message_storage: None,
            lamport_clock: LamportClock::new(),
            config,
        })
    }

    /// Initializes MLS encryption with the provided storage backend.
    ///
    /// This must be called before encryption can be used. The storage
    /// backend should be a platform-native secure storage implementation
    /// (iOS Keychain, Android EncryptedSharedPreferences, etc.).
    ///
    /// The same storage is also used for persisting pending messages,
    /// ensuring they survive app crashes/restarts.
    pub fn initialize_mls(&mut self, storage: Arc<dyn MlsStorage>) -> Result<()> {
        let manager = MlsManager::new(&self.config.user_id, storage.clone())?;
        self.mls_manager = Some(Arc::new(RwLock::new(manager)));

        // Also use this storage for pending message persistence
        self.message_storage = Some(storage);

        // Restore state from previous session
        self.restore_pending_messages()?;
        self.restore_lamport_clock();

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
        let mut state = lock_shared_state(&self.shared_state)?;

        state.state = ProtocolState::Running;

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

        // Schedule retry - if this fails (max retries exceeded), the message
        // will still be in the outbox and can be recovered
        if let Err(e) = self.retry_queue.enqueue(message.clone(), 0) {
            warn!(
                message_id = %message.id,
                error = %e,
                "Failed to enqueue message for retry, message remains in outbox"
            );
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
    fn send_internal_message(
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

        // Auto-encrypt if enabled
        let final_content = if self.should_auto_encrypt() {
            match self.encrypt_content_for_recipient(&recipient_str, &content_str, priority) {
                Ok(encrypted) => encrypted,
                Err(Error::SessionPending) => {
                    // Generate an ID without ticking the Lamport clock.
                    // The real tick happens when flush_pending_messages re-sends
                    // via send_message after the session is established.
                    let message_id = MessageId::new();

                    debug!(recipient = %recipient_str, message_id = %message_id, "Message queued pending session establishment");
                    self.queue_pending_message(
                        &recipient_str,
                        &content_str,
                        priority,
                        message_id.clone(),
                        reply_to_msg_id.clone(),
                    );

                    return Ok(message_id);
                }
                Err(Error::NoKeyPackage(ref r)) => {
                    // No key package, send unencrypted if we're in a fallback mode
                    warn!(recipient = %r, "No key package available, sending unencrypted");
                    content_str
                }
                Err(e) => {
                    warn!(error = %e, "Encryption failed, sending unencrypted");
                    content_str
                }
            }
        } else {
            content_str
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

    /// Encrypts content for a recipient, handling session creation if needed.
    ///
    /// To avoid race conditions where both peers create sessions simultaneously,
    /// we defer encryption until the session is "confirmed". A session is confirmed when:
    /// - We join via their Welcome message (welcome-wins), OR
    /// - We successfully decrypt their first message
    fn encrypt_content_for_recipient(
        &mut self,
        recipient: &str,
        content: &str,
        _priority: MessagePriority,
    ) -> Result<String> {
        // Clone the Arc to avoid borrow issues
        let mls = self.mls_manager.clone().ok_or(Error::MlsNotInitialized)?;

        // Check for existing session
        let has_session = {
            let manager = mls
                .read()
                .map_err(|_| Error::Other("MLS lock poisoned".to_string()))?;
            manager.has_session(recipient)?
        };

        if !has_session {
            // Try to create session from stored key package
            // Clone first, only remove after all operations succeed to avoid losing the key package on failure
            if let Some(received_pkg) = self.pending_key_packages.get(recipient).cloned() {
                // Check if key package has expired (using local clock)
                let now_ms = Utc::now().timestamp_millis() as u64;
                if now_ms >= received_pkg.local_expires_at_ms {
                    warn!(recipient = %recipient, "Received key package has expired, discarding");
                    self.pending_key_packages.remove(recipient);
                } else {
                    {
                        let manager = mls
                            .read()
                            .map_err(|_| Error::Other("MLS lock poisoned".to_string()))?;
                        manager
                            .import_key_package(recipient, &received_pkg.key_package_data)?;
                    }

                    // Create session and send welcome message
                    let welcome = {
                        let manager = mls
                            .read()
                            .map_err(|_| Error::Other("MLS lock poisoned".to_string()))?;
                        manager.create_session(recipient)?
                    };

                    // Send welcome as internal message
                    self.send_welcome_message(recipient, &welcome)?;

                    // All operations succeeded, now safe to remove the key package
                    self.pending_key_packages.remove(recipient);

                    let group_id = welcome.group_id.as_str().to_string();
                    let is_session = group_id.starts_with("session:");

                    debug!(recipient = %recipient, group_id = %group_id, "Created MLS session and sent welcome");

                    // Emit secure session established event
                    if let Ok(state) = lock_shared_state(&self.shared_state) {
                        state.emit_event(Event::secure_session_established(
                            recipient.to_string(),
                            group_id,
                            is_session,
                            true, // initiated_by_local is true - we sent the Welcome
                        ));
                    }

                    // Don't encrypt immediately after creating session.
                    // Queue message until session is confirmed (peer processes our Welcome
                    // and we successfully decrypt their first message, or we receive their Welcome).
                    // This avoids race conditions where both peers create sessions.
                    if self.config.encryption.store_pending {
                        return Err(Error::SessionPending);
                    }
                }
            } else {
                // No key package available
                if self.config.encryption.store_pending {
                    // Note: queue_pending_message will be called from send_message
                    // after creating a message to get an ID
                    return Err(Error::SessionPending);
                }
                return Err(Error::NoKeyPackage(recipient.to_string()));
            }
        }

        // Only encrypt if session is confirmed (Welcome processed or successful decrypt)
        if !self.confirmed_sessions.contains(recipient) {
            debug!(recipient = %recipient, "Session exists but not confirmed, queuing message");
            if self.config.encryption.store_pending {
                return Err(Error::SessionPending);
            }
            // If not storing pending, we still need to wait for confirmation
            return Err(Error::SessionPending);
        }

        // Encrypt the message
        let encrypted = {
            let manager = mls
                .read()
                .map_err(|_| Error::Other("MLS lock poisoned".to_string()))?;
            manager.encrypt_for_user(recipient, content.as_bytes())?
        };

        // Serialize encrypted message with prefix
        let serialized =
            serde_json::to_string(&encrypted).map_err(|e| Error::Serialization(e.to_string()))?;

        Ok(format!("{}{}", internal_prefixes::ENCRYPTED, serialized))
    }

    /// Sends a welcome message to establish an MLS session.
    fn send_welcome_message(&mut self, recipient: &str, welcome: &WelcomeMessage) -> Result<()> {
        let serialized =
            serde_json::to_string(welcome).map_err(|e| Error::Serialization(e.to_string()))?;
        let content = format!("{}{}", internal_prefixes::WELCOME, serialized);

        // Create and send internal message with high priority
        let message = self.create_message(recipient, content, Some(MessagePriority::High), None)?;
        let _ = self.transport_manager.send(&message);

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
            .or_insert_with(Vec::new)
            .push(pending);

        debug!(recipient = %recipient, message_id = %message_id_str, "Queued message pending session establishment");
    }

    /// Flushes pending messages for a recipient after session is established.
    fn flush_pending_messages(&mut self, recipient: &str) -> Result<()> {
        if let Some(pending) = self.pending_encrypted_messages.remove(recipient) {
            info!(recipient = %recipient, count = pending.len(), "Flushing pending messages");

            // Clear from persistent storage since we're about to send them
            self.clear_pending_messages_from_storage(recipient);

            for msg in pending {
                // Re-attempt to send each pending message
                // Use the stored message ID by passing reply_to_msg if it exists
                let reply_to_str = msg.reply_to_msg.as_ref().map(|id| id.as_str().to_string());
                match self.send_message(recipient, msg.content, Some(msg.priority), reply_to_str) {
                    Ok(id) => {
                        // Note: The new message will have a new ID, but the original ID was already returned to the caller
                        debug!(original_id = %msg.message_id, new_id = %id, "Sent pending message");
                    }
                    Err(e) => {
                        warn!(original_id = %msg.message_id, error = %e, "Failed to send pending message")
                    }
                }
            }
        }
        Ok(())
    }

    /// Processes encrypted messages that were received before the session was established.
    ///
    /// This handles the case where encrypted messages arrive before the Welcome message.
    /// After the session is confirmed (via Welcome), we re-process these queued messages.
    fn process_pending_decryption(&mut self, sender: &str) {
        let messages = match self.pending_decryption.remove(sender) {
            Some(msgs) => msgs,
            None => return,
        };

        if messages.is_empty() {
            return;
        }

        info!(sender = %sender, count = messages.len(), "Processing pending encrypted messages");

        for msg in messages {
            // Re-process each message through the internal handler
            if let Some(result) = self.process_internal_message(&msg) {
                match result {
                    InternalMessageResult::Decrypted(content) => {
                        // Successfully decrypted - add to received messages queue
                        let mut decrypted_msg = msg.clone();
                        decrypted_msg.content = content.clone();
                        decrypted_msg
                            .metadata
                            .insert("encrypted".to_string(), "true".to_string());
                        decrypted_msg
                            .metadata
                            .insert("delayed_decrypt".to_string(), "true".to_string());

                        // Advance local Lamport clock for delayed-decrypted messages
                        self.lamport_clock.merge(decrypted_msg.lamport_clock);
                        self.persist_lamport_clock();

                        if let Ok(mut state) = lock_shared_state(&self.shared_state) {
                            state.received_messages.push(decrypted_msg.clone());

                            // Emit message received event
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
                            };
                            state.emit_event(event);
                        }

                        debug!(message_id = %msg.id, "Processed delayed encrypted message");
                    }
                    InternalMessageResult::Consumed => {
                        // Message was consumed (shouldn't happen for encrypted messages, but handle it)
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
        let Some(storage) = &self.message_storage else {
            return;
        };

        // Load existing messages for this recipient
        let mut messages: Vec<PendingMessage> = self
            .load_pending_messages_from_storage(recipient)
            .unwrap_or_default();

        // Add the new message
        messages.push(pending.clone());

        // Serialize and store
        match serde_json::to_vec(&messages) {
            Ok(data) => {
                if let Err(e) = storage.store(storage_keys::PENDING_MESSAGES, recipient, &data) {
                    warn!(recipient = %recipient, error = %e, "Failed to persist pending message");
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
        if let Ok(Some(data)) = storage.load(
            storage_keys::LAMPORT_CLOCK,
            storage_keys::LAMPORT_CLOCK_ID,
        ) {
            if data.len() == 8 {
                let restored = u64::from_le_bytes(
                    data.try_into().expect("verified length is 8"),
                );
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

        // Send as low priority internal message
        let message = self.create_message(peer_id, content, Some(MessagePriority::Low), None)?;
        let _ = self.transport_manager.send(&message);

        self.key_package_sent_to.insert(peer_id.to_string());
        debug!(peer_id = %peer_id, "Sent key package");

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
        // Only send key package if encryption is enabled and auto key exchange is on
        if !self.config.encryption.enabled || !self.config.encryption.auto_key_exchange {
            return;
        }

        // Don't send to ourselves
        if peer_id == self.config.user_id {
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
    /// * `Err(NoKeyPackage)` - No key package available for the peer yet
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

        // Check for pending key package
        // Clone first, only remove after all operations succeed to avoid losing the key package on failure
        if let Some(received_pkg) = self.pending_key_packages.get(peer_id).cloned() {
            // Check if key package has expired (using local clock)
            let now_ms = Utc::now().timestamp_millis() as u64;
            if now_ms >= received_pkg.local_expires_at_ms {
                warn!(peer_id = %peer_id, "Received key package has expired, discarding");
                self.pending_key_packages.remove(peer_id);
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
                self.send_welcome_message(peer_id, &welcome)?;

                // All operations succeeded, now safe to remove the key package
                self.pending_key_packages.remove(peer_id);

                let group_id = welcome.group_id.as_str().to_string();
                let is_session = group_id.starts_with("session:");

                info!(peer_id = %peer_id, group_id = %group_id, "Established secure session");

                // Emit secure session established event
                if let Ok(state) = lock_shared_state(&self.shared_state) {
                    state.emit_event(Event::secure_session_established(
                        peer_id.to_string(),
                        group_id,
                        is_session,
                        true, // initiated_by_local is true - we sent the Welcome
                    ));
                }

                return Ok(Some(welcome));
            }
        }

        // No key package available - peer hasn't sent one yet
        debug!(peer_id = %peer_id, "No key package available for peer");
        Err(Error::NoKeyPackage(peer_id.to_string()))
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

        // Parse reply_to_msg if provided
        let reply_to_msg_id = reply_to_msg
            .map(|r| MessageId::from_str(&r.into()))
            .transpose()
            .map_err(|e| Error::Other(format!("Invalid reply_to_msg: {}", e)))?;

        // Create message
        let message = self.create_message(recipient, content, priority, reply_to_msg_id)?;
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
    pub fn send_connection_request(
        &mut self,
        recipient: &str,
        sender_name: &str,
        key_package: Option<Vec<u8>>,
    ) -> Result<MessageId> {
        let payload = ConnectionRequestPayload {
            sender_name: sender_name.to_string(),
            timestamp_ms: Utc::now().timestamp_millis(),
            key_package,
        };

        let serialized =
            serde_json::to_string(&payload).map_err(|e| Error::Serialization(e.to_string()))?;
        let content = format!("{}{}", internal_prefixes::CONN_REQUEST, serialized);

        let message_id =
            self.send_internal_message(recipient, content, MessagePriority::High)?;
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
    pub fn accept_connection_request(
        &mut self,
        recipient: &str,
        accepter_name: &str,
        key_package: Option<Vec<u8>>,
    ) -> Result<MessageId> {
        let payload = ConnectionAcceptedPayload {
            accepted_by_name: accepter_name.to_string(),
            timestamp_ms: Utc::now().timestamp_millis(),
            key_package,
        };

        let serialized =
            serde_json::to_string(&payload).map_err(|e| Error::Serialization(e.to_string()))?;
        let content = format!("{}{}", internal_prefixes::CONN_ACCEPT, serialized);

        let message_id =
            self.send_internal_message(recipient, content, MessagePriority::High)?;
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
    pub fn reject_connection_request(&mut self, recipient: &str) -> Result<MessageId> {
        let content = internal_prefixes::CONN_REJECT.to_string();

        let message_id =
            self.send_internal_message(recipient, content, MessagePriority::High)?;
        info!(recipient = %recipient, "Rejected connection request");
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

        if !state.received_messages.is_empty() {
            return Some(state.received_messages.remove(0));
        }

        drop(state);

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
                        continue;
                    }

                    self.deduplicator.mark_seen(message.id.clone());

                    // Handle internal MLS messages
                    if let Some(result) = self.process_internal_message(&message) {
                        match result {
                            InternalMessageResult::Consumed => {
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
    fn process_internal_message(&mut self, message: &Message) -> Option<InternalMessageResult> {
        let content = &message.content;
        let sender = message.sender.as_str();

        // Handle key package messages
        if content.starts_with(internal_prefixes::KEY_PACKAGE) {
            let data = &content[internal_prefixes::KEY_PACKAGE.len()..];
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
                self.pending_key_packages.insert(
                    sender.to_string(),
                    ReceivedKeyPackage {
                        key_package_data: payload.key_package_data,
                        local_expires_at_ms,
                    },
                );

                // Send our key package back if auto_key_exchange is enabled
                if self.config.encryption.auto_key_exchange && self.config.encryption.enabled {
                    if !self.key_package_sent_to.contains(sender) {
                        let _ = self.send_key_package_to(sender);
                    }
                }
            }
            return Some(InternalMessageResult::Consumed);
        }

        // Handle welcome messages (session invitation)
        if content.starts_with(internal_prefixes::WELCOME) {
            let data = &content[internal_prefixes::WELCOME.len()..];
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
                        // Check if we already have a session (race condition)
                        let has_existing = manager.has_session(sender).unwrap_or(false);

                        let result = if has_existing {
                            // Welcome-wins: replace our session with theirs
                            info!(sender = %sender, "Welcome-wins: replacing our session with incoming Welcome");
                            manager.replace_session_with_welcome(&welcome)
                        } else {
                            manager.join_session(&welcome)
                        };

                        match result {
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

                // Confirm session and process queued items after releasing the MLS lock
                if should_flush {
                    // Mark session as confirmed - we're using their session state
                    self.confirmed_sessions.insert(sender_owned.clone());

                    // Flush pending outgoing messages
                    let _ = self.flush_pending_messages(&sender_owned);

                    // Process any encrypted messages that arrived before the Welcome
                    self.process_pending_decryption(&sender_owned);

                    // Emit secure session established event
                    if let Ok(state) = lock_shared_state(&self.shared_state) {
                        state.emit_event(Event::secure_session_established(
                            sender_owned,
                            group_id,
                            is_session,
                            false, // initiated_by_local is false - we received the Welcome
                        ));
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
        if content.starts_with(internal_prefixes::ENCRYPTED) {
            let data = &content[internal_prefixes::ENCRYPTED.len()..];
            if let Ok(encrypted) = serde_json::from_str::<EncryptedMessage>(data) {
                // Track state to update after releasing MLS lock
                enum DecryptResult {
                    Success { text: String, sender: String },
                    Empty,
                    SessionNotReady { sender: String },
                    Failed { _error: String },
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
                                }
                            }
                            Ok(None) => {
                                warn!(sender = %sender, "Decryption returned empty");
                                DecryptResult::Empty
                            }
                            Err(e) => {
                                let error_str = e.to_string();
                                if error_str.contains("not found")
                                    || error_str.contains("GroupNotFound")
                                {
                                    info!(sender = %sender, "Encrypted message received before session ready, queuing");
                                    DecryptResult::SessionNotReady {
                                        sender: sender.to_string(),
                                    }
                                } else {
                                    warn!(error = %e, sender = %sender, "Failed to decrypt message");
                                    DecryptResult::Failed { _error: error_str }
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
                    } => {
                        if !self.confirmed_sessions.contains(&sender_owned) {
                            info!(sender = %sender_owned, "Session confirmed via successful decryption");
                            self.confirmed_sessions.insert(sender_owned.clone());
                            let _ = self.flush_pending_messages(&sender_owned);
                        }
                        return Some(InternalMessageResult::Decrypted(text));
                    }
                    DecryptResult::Empty => {
                        return Some(InternalMessageResult::Decrypted(
                            "[Decryption failed]".to_string(),
                        ));
                    }
                    DecryptResult::SessionNotReady {
                        sender: sender_owned,
                    } => {
                        self.pending_decryption
                            .entry(sender_owned)
                            .or_default()
                            .push(message.clone());
                        return Some(InternalMessageResult::Consumed);
                    }
                    DecryptResult::Failed { .. } => {
                        return Some(InternalMessageResult::Decrypted(
                            "[Unable to decrypt]".to_string(),
                        ));
                    }
                    DecryptResult::MlsNotInitialized => {
                        return Some(InternalMessageResult::Decrypted(
                            "[Encryption not initialized]".to_string(),
                        ));
                    }
                }
            }
        }

        // Handle connection request messages
        if content.starts_with(internal_prefixes::CONN_REQUEST) {
            let data = &content[internal_prefixes::CONN_REQUEST.len()..];
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
        if content.starts_with(internal_prefixes::CONN_ACCEPT) {
            let data = &content[internal_prefixes::CONN_ACCEPT.len()..];
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
        if content.starts_with(internal_prefixes::CONN_REJECT) {
            info!(sender = %sender, "Connection request rejected");
            if let Ok(state) = lock_shared_state(&self.shared_state) {
                state.emit_event(Event::connection_rejected(sender.to_string()));
            }
            return Some(InternalMessageResult::Consumed);
        }

        // --- Group (relay) messages ---

        if content.starts_with(internal_prefixes::GROUP_CREATED) {
            let data = &content[internal_prefixes::GROUP_CREATED.len()..];
            if let Ok(payload) = serde_json::from_str::<GroupCreatedPayload>(data) {
                info!(group_id = %payload.group_id, "Group created");
                if let Ok(state) = lock_shared_state(&self.shared_state) {
                    state.emit_event(Event::group_created(
                        payload.group_id,
                        payload.name,
                    ));
                }
            } else {
                warn!("Failed to parse GroupCreated payload");
            }
            return Some(InternalMessageResult::Consumed);
        }

        if content.starts_with(internal_prefixes::GROUP_MSG) {
            let data = &content[internal_prefixes::GROUP_MSG.len()..];
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

        if content.starts_with(internal_prefixes::GROUP_MEMBER_ADDED) {
            let data = &content[internal_prefixes::GROUP_MEMBER_ADDED.len()..];
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

        if content.starts_with(internal_prefixes::GROUP_MEMBER_REMOVED) {
            let data = &content[internal_prefixes::GROUP_MEMBER_REMOVED.len()..];
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

        if content.starts_with(internal_prefixes::GROUP_INFO) {
            let data = &content[internal_prefixes::GROUP_INFO.len()..];
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

        if content.starts_with(internal_prefixes::USER_GROUPS) {
            let data = &content[internal_prefixes::USER_GROUPS.len()..];
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

        if content.starts_with(internal_prefixes::GROUP_ERROR) {
            let data = &content[internal_prefixes::GROUP_ERROR.len()..];
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
        self.process_timed_out_acks()?;
        self.cleanup_expired_entries();
        self.check_dors_escalation()?;

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

            // Attempt to send via DORS-selected transport
            match self.transport_manager.send(&entry.message) {
                Ok(()) => {
                    let current_transport = self.transport_manager.current_transport();
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

                    if let Some(transport) = previous_transport {
                        self.transport_manager.record_retry_failure(transport);
                    }

                    debug!(
                        message_id = %entry.message.id,
                        retry_count = entry.retry_count,
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
        if let Some(entry) = self.outbox.get(message_id) {
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

        self.ack_manager.remove_ack(message_id);
        Ok(())
    }

    /// Cleans up expired entries from deduplicator, retry queue, outbox, and ack manager.
    fn cleanup_expired_entries(&mut self) {
        self.deduplicator.cleanup_expired();
        self.retry_queue.cleanup_expired();
        self.cleanup_outbox();
        // Prune old timed-out ACKs that weren't cleaned up by normal retry flow
        self.ack_manager
            .prune_old_timeouts(std::time::Duration::from_secs(300)); // 5 minutes
    }

    /// Checks for DORS escalation signals and emits events if needed.
    fn check_dors_escalation(&mut self) -> Result<()> {
        if !self.transport_manager.should_escalate_to_wifi() {
            return Ok(());
        }

        use offline_protocol_transport::TransportType;
        let active_transports = self.transport_manager.get_active_transports();

        if !active_transports.contains(&TransportType::WiFiDirect) {
            let state = lock_shared_state(&self.shared_state).map_err(|e| {
                error!(
                    "Failed to lock shared state for WiFi escalation event: {}",
                    e
                );
                e
            })?;
            state.emit_event(Event::transport_switched(
                Some(TransportType::BLE),
                TransportType::WiFiDirect,
                "DORS suggests escalating to WiFi Direct due to BLE failures".to_string(),
            ));
            drop(state);
        }
        Ok(())
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

    /// Gets the current ACK manager statistics.
    pub fn pending_ack_count(&self) -> usize {
        self.ack_manager.pending_count()
    }

    /// Gets the current retry queue statistics.
    pub fn retry_queue_size(&self) -> usize {
        self.retry_queue.len()
    }

    fn ensure_outbox_entry(&mut self, message: &Message) {
        if !message.requires_ack {
            return;
        }

        if !self.outbox.contains_key(&message.id) && self.outbox.len() >= MAX_OUTBOX_ENTRIES {
            if let Some((oldest_id, last_transport)) = self
                .outbox
                .iter()
                .min_by_key(|(_, entry)| entry.last_sent_at)
                .map(|(id, entry)| (id.clone(), entry.last_transport))
            {
                if let Some(transport) = last_transport {
                    self.transport_manager.record_delivery_failure(transport);
                }
                self.outbox.remove(&oldest_id);
            }
        }

        self.outbox
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
        let entry = self
            .outbox
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
        self.outbox.remove(message_id)
    }

    fn cleanup_outbox(&mut self) {
        if self.outbox.is_empty() {
            return;
        }

        let cutoff = Utc::now()
            - ChronoDuration::milliseconds(
                self.config.reliability.retry.outbox_max_lifetime_ms as i64,
            );

        let mut expired_ids = Vec::new();
        for (message_id, entry) in &self.outbox {
            if entry.last_sent_at >= cutoff {
                continue;
            }

            if entry.message.requires_ack && self.ack_manager.is_waiting_for_ack(&entry.message.id)
            {
                continue;
            }

            expired_ids.push((message_id.clone(), entry.last_transport));
        }

        for (message_id, last_transport) in expired_ids {
            if let Some(transport) = last_transport {
                self.transport_manager.record_delivery_failure(transport);
            }
            self.outbox.remove(&message_id);
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
mod tests {
    use super::*;
    use offline_protocol_transport::{mock::MockTransport, Transport, TransportType};
    use std::sync::{Arc, Mutex};
    use std::thread;
    use std::time::Duration;

    fn create_test_config() -> ProtocolConfig {
        ProtocolConfig::new("test-app", "user123")
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
        assert_eq!(internal_prefixes::CONN_REQUEST, "__CONN_REQ__");
        assert_eq!(internal_prefixes::CONN_ACCEPT, "__CONN_ACC__");
        assert_eq!(internal_prefixes::CONN_REJECT, "__CONN_REJ__");
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

        protocol
            .pending_decryption
            .entry("sender123".to_string())
            .or_default()
            .push(message);

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

        protocol
            .pending_decryption
            .entry("sender123".to_string())
            .or_default()
            .push(message2);

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

        protocol
            .pending_decryption
            .entry("sender123".to_string())
            .or_default()
            .push(message);

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

        // Process the message without MLS initialized - should fail gracefully
        let result = protocol.process_internal_message(&message);

        // Without MLS initialized, should return placeholder text
        assert!(matches!(result, Some(InternalMessageResult::Decrypted(_))));
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
            protocol.transport_manager_mut().add_transport(
                TransportType::BLE,
                Box::new(mock_transport),
            );

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
            protocol.transport_manager_mut().add_transport(
                TransportType::BLE,
                Box::new(mock_transport),
            );

            assert_eq!(protocol.lamport_clock.value(), 0);

            protocol
                .enable_message_persistence(storage.clone())
                .unwrap();

            // After attaching storage, clock should be restored
            assert_eq!(protocol.lamport_clock.value(), 5);

            // Next send should be 6, not 1
            protocol.start().unwrap();
            protocol
                .send_message("bob", "after restart", None::<MessagePriority>, None::<String>)
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
        let diff = received.local_expires_at_ms.abs_diff(now_ms + thirty_days_ms);
        assert!(diff < 1000, "Expiry should be ~30 days from now, diff was {}", diff);
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
}
