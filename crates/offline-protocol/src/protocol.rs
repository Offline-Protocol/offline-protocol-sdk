//! Main protocol engine.

use crate::constants::{
    ACK_FOR_KEY, ACK_HOP_COUNT_KEY, ACK_TRANSPORT_KEY, MAX_OUTBOX_ENTRIES,
};
use crate::{Error, Event, EventCallback, ProtocolConfig, Result, TransportManager};
use chrono::{DateTime, Duration as ChronoDuration, Utc};
use offline_protocol_core::{AppId, Message, MessageId, MessagePriority, UserId, TTL};
use offline_protocol_reliability::{AckManager, Deduplicator, RetryQueue};
use offline_protocol_router::{DorsConfig, PathSelector, RelayManager, TransportSelector};
use offline_protocol_transport::TransportType;
use std::collections::HashMap;
use std::sync::{Arc, Mutex};
use tracing::{debug, error, warn};

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
    state.lock().map_err(|_| Error::Other("Shared state mutex poisoned".to_string()))
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

    /// Path selector for routing.
    #[allow(dead_code)]
    path_selector: PathSelector,

    /// Relay manager.
    #[allow(dead_code)]
    relay_manager: RelayManager,

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
            relay_manager: RelayManager::with_config(config.relay.clone()),
            ack_manager: AckManager::with_config(config.reliability.ack.clone()),
            retry_queue: RetryQueue::with_config(config.reliability.retry.clone()),
            deduplicator: Deduplicator::with_config(config.reliability.dedup.clone()),
            shared_state: Arc::new(Mutex::new(SharedState::new())),
            outbox: HashMap::new(),
            config,
        })
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
                        error!("Failed to lock shared state for ACK event, skipping event emission");
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
        &self,
        recipient: impl Into<String>,
        content: impl Into<String>,
        priority: Option<MessagePriority>,
    ) -> Result<Message> {
        let sender = UserId::new(&self.config.user_id)?;
        let recipient = UserId::new(recipient)?;
        let app_id = AppId::new(&self.config.app_id)?;

        Ok(Message::builder(sender, recipient, app_id)
            .content(content)
            .priority(priority.unwrap_or(MessagePriority::Medium))
            .ttl(TTL::new(self.config.initial_ttl)?)
            .build())
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
    fn handle_send_failure(
        &mut self,
        message: &Message,
        transport: Option<TransportType>,
    ) -> Result<()> {
        self.ensure_outbox_entry(message);
        self.retry_queue.enqueue(message.clone(), 0)?;

        if let Some(transport) = transport {
            self.transport_manager.record_retry_failure(transport);
        }

        warn!(
            message_id = %message.id,
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
                let state = lock_shared_state(&self.shared_state)
                    .map_err(|e| {
                        error!("Failed to lock shared state for transport switch event: {}", e);
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
        let state = lock_shared_state(&self.shared_state)
            .map_err(|e| {
                error!("Failed to lock shared state for message sent event: {}", e);
                e
            })?;
        state.emit_event(Event::message_sent(message));
        drop(state);
        Ok(())
    }

    /// Sends a message.
    ///
    /// # Arguments
    ///
    /// * `recipient` - Recipient's user ID
    /// * `content` - Message content
    /// * `priority` - Message priority (optional, defaults to Medium)
    ///
    /// # Returns
    ///
    /// Returns the message ID if successful.
    pub fn send_message(
        &mut self,
        recipient: impl Into<String>,
        content: impl Into<String>,
        priority: Option<MessagePriority>,
    ) -> Result<MessageId> {
        // Check if protocol is running
        {
            let state = lock_shared_state(&self.shared_state)?;
            if state.state != ProtocolState::Running {
                return Err(Error::NotStarted);
            }
        }

        // Create message
        let message = self.create_message(recipient, content, priority)?;
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
            }
            Err(err) => {
                self.handle_send_failure(&message, current_transport.or(previous_transport))?;
                warn!(
                    message_id = %message.id,
                    error = %err,
                    "Send failed, message deferred"
                );
            }
        }

        // Emit events
        self.emit_transport_switch_event(previous_transport, current_transport)?;
        self.emit_message_sent_event(&message)?;

        Ok(message_id)
    }

    /// Sends a message via a specific transport, bypassing DORS selection.
    ///
    /// # Arguments
    ///
    /// * `recipient` - Recipient's user ID
    /// * `content` - Message content
    /// * `priority` - Message priority (optional, defaults to Medium)
    /// * `transport` - The transport to use
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
    ) -> Result<MessageId> {
        // Check if protocol is running
        {
            let state = lock_shared_state(&self.shared_state)?;
            if state.state != ProtocolState::Running {
                return Err(Error::NotStarted);
            }
        }

        // Create message
        let message = self.create_message(recipient, content, priority)?;
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
        let send_result = self.transport_manager.send_via_transport(&message, transport);
        let current_transport = Some(transport);

        // Handle send result
        match send_result {
            Ok(()) => {
                self.handle_send_success(&message, current_transport)?;
            }
            Err(err) => {
                self.handle_send_failure(&message, current_transport.or(previous_transport))?;
                warn!(
                    message_id = %message.id,
                    transport = ?transport,
                    error = %err,
                    "Send via forced transport failed, message deferred"
                );
            }
        }

        // Emit events
        self.emit_transport_switch_event(previous_transport, current_transport)?;
        self.emit_message_sent_event(&message)?;

        Ok(message_id)
    }

    /// Receives the next available message.
    ///
    /// # Returns
    ///
    /// Returns `Some(Message)` if a message is available, `None` otherwise.
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
                Ok(Some((transport_used, message))) => {
                    if message.metadata.contains_key(ACK_FOR_KEY) {
                        self.handle_ack_message(&message);
                        continue;
                    }

                    if self.deduplicator.is_duplicate(&message.id) {
                        continue;
                    }

                    self.deduplicator.mark_seen(message.id.clone());

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
    fn process_retry_queue(&mut self) -> Result<()> {
        while let Some(entry) = self.retry_queue.dequeue_ready() {
            let previous_transport = self.transport_manager.current_transport();
            self.ensure_outbox_entry(&entry.message);

            if self.transport_manager.send(&entry.message).is_err() {
                // Re-enqueue with incremented retry count
                let _ = self
                    .retry_queue
                    .enqueue(entry.message.clone(), entry.retry_count + 1);

                if let Some(transport) = previous_transport {
                    self.transport_manager.record_retry_failure(transport);
                }
            } else {
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
            }
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
        let state = lock_shared_state(&self.shared_state)
            .map_err(|e| {
                error!("Failed to lock shared state for message failed event: {}", e);
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
    fn handle_ack_timeout_retry(
        &mut self,
        message_id: &MessageId,
        retry_count: u32,
    ) -> Result<()> {
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
        let state = lock_shared_state(&self.shared_state)
            .map_err(|e| {
                error!("Failed to lock shared state for retry queue error event: {}", e);
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
        let state = lock_shared_state(&self.shared_state)
            .map_err(|e| {
                error!("Failed to lock shared state for missing outbox entry event: {}", e);
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
            let state = lock_shared_state(&self.shared_state)
                .map_err(|e| {
                    error!("Failed to lock shared state for WiFi escalation event: {}", e);
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

        let result = protocol.send_message("bob", "Hello!", None);
        assert!(result.is_ok());
    }

    #[test]
    fn test_send_message_not_started() {
        let mut protocol = OfflineProtocol::new(create_test_config()).unwrap();

        let result = protocol.send_message("bob", "Hello!", None);
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
        protocol.send_message("bob", "Hello!", None).unwrap();

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
        protocol.send_message("bob", "Hello!", None).unwrap();
        let result = protocol.send_message("bob", "Hello!", None);

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

        protocol.send_message("bob", "Hello!", None).unwrap();
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
}
