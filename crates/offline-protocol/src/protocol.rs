//! Main protocol engine.

use crate::{Event, EventCallback, ProtocolConfig, Result, TransportManager};
use offline_protocol_core::{AppId, Message, MessageId, MessagePriority, UserId, TTL};
use offline_protocol_reliability::{AckManager, Deduplicator, RetryQueue};
use offline_protocol_router::{PathSelector, RelayManager, TransportSelector};
use std::sync::{Arc, Mutex};

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
            config,
        })
    }

    /// Starts the protocol.
    ///
    /// # Returns
    ///
    /// Returns `Ok(())` if started successfully, `Err` if already started.
    pub fn start(&mut self) -> Result<()> {
        let state = self.shared_state.lock().unwrap();

        if state.state != ProtocolState::Stopped {
            return Err(crate::Error::AlreadyStarted);
        }

        // Start all transports
        drop(state);
        self.transport_manager.start()?;
        let mut state = self.shared_state.lock().unwrap();

        state.state = ProtocolState::Running;

        Ok(())
    }

    /// Stops the protocol gracefully.
    ///
    /// # Returns
    ///
    /// Returns `Ok(())` if stopped successfully, `Err` if not started.
    pub fn stop(&mut self) -> Result<()> {
        let state = self.shared_state.lock().unwrap();

        if state.state == ProtocolState::Stopped {
            return Ok(()); // Already stopped
        }

        // Stop all transports
        drop(state);
        self.transport_manager.stop()?;
        let mut state = self.shared_state.lock().unwrap();

        state.state = ProtocolState::Stopped;

        Ok(())
    }

    /// Pauses the protocol (for background mode).
    pub fn pause(&mut self) -> Result<()> {
        let mut state = self.shared_state.lock().unwrap();

        if state.state != ProtocolState::Running {
            return Err(crate::Error::NotStarted);
        }

        state.state = ProtocolState::Paused;
        Ok(())
    }

    /// Resumes the protocol from pause.
    pub fn resume(&mut self) -> Result<()> {
        let mut state = self.shared_state.lock().unwrap();

        if state.state != ProtocolState::Paused {
            return Err(crate::Error::InvalidConfiguration(
                "Protocol is not paused".to_string(),
            ));
        }

        state.state = ProtocolState::Running;
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
            let state = self.shared_state.lock().unwrap();
            if state.state != ProtocolState::Running {
                return Err(crate::Error::NotStarted);
            }
        }

        // Create message
        let sender = UserId::new(&self.config.user_id)?;
        let recipient = UserId::new(recipient)?;
        let app_id = AppId::new(&self.config.app_id)?;

        let message = Message::builder(sender, recipient, app_id)
            .content(content)
            .priority(priority.unwrap_or(MessagePriority::Medium))
            .ttl(TTL::new(self.config.initial_ttl)?)
            .build();

        let message_id = message.id.clone();

        // Check for duplicates
        if self.deduplicator.is_duplicate(&message_id) {
            return Err(crate::Error::Other("Duplicate message".to_string()));
        }

        // Mark as seen
        self.deduplicator.mark_seen(message_id.clone());

        // Send via transport manager (DORS will select best transport)
        self.transport_manager.send(&message)?;

        // Register for ACK if required
        if message.requires_ack {
            self.ack_manager
                .register_pending_ack(message_id.clone(), None)?;
        }

        // Emit MessageSent event
        {
            let state = self.shared_state.lock().unwrap();
            state.emit_event(Event::message_sent(message_id.clone()));
        }

        Ok(message_id)
    }

    /// Receives the next available message.
    ///
    /// # Returns
    ///
    /// Returns `Some(Message)` if a message is available, `None` otherwise.
    pub fn receive_message(&mut self) -> Option<Message> {
        let mut state = self.shared_state.lock().unwrap();

        // Check if we have any queued messages
        if !state.received_messages.is_empty() {
            return Some(state.received_messages.remove(0));
        }

        drop(state);

        // Try to receive from transport manager
        if let Ok(Some(message)) = self.transport_manager.receive() {
            // Check for duplicates
            if self.deduplicator.is_duplicate(&message.id) {
                return None; // Skip duplicate
            }

            self.deduplicator.mark_seen(message.id.clone());

            // Emit MessageReceived event
            let event = Event::MessageReceived {
                message_id: message.id.as_str(),
                sender: message.sender.as_str().to_string(),
                recipient: message.recipient.as_str().to_string(),
                content: message.content.clone(),
                hop_count: message.hop_count.value(),
                transport: "BLE".to_string(), // Mock for now
                timestamp: message.timestamp.as_millis(),
            };

            let state = self.shared_state.lock().unwrap();
            state.emit_event(event);
            drop(state);

            Some(message)
        } else {
            None
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
        let mut state = self.shared_state.lock().unwrap();
        state.event_handlers.push(Arc::new(handler));
    }

    /// Processes pending operations (retries, timeouts, etc.).
    ///
    /// This should be called periodically to handle background tasks.
    pub fn process(&mut self) -> Result<()> {
        {
            let state = self.shared_state.lock().unwrap();
            if state.state != ProtocolState::Running {
                return Ok(()); // Don't process if not running
            }
        }

        // Check for retry-ready messages
        while let Some(entry) = self.retry_queue.dequeue_ready() {
            // Try to resend
            if self.transport_manager.send(&entry.message).is_err() {
                // Re-enqueue with incremented retry count
                let _ = self
                    .retry_queue
                    .enqueue(entry.message.clone(), entry.retry_count + 1);
            } else {
                // Reset ACK timer
                self.ack_manager.increment_retry_count(&entry.message.id);
            }
        }

        // Check for timed out ACKs
        let timed_out = self.ack_manager.get_timed_out_acks();
        for message_id in timed_out {
            // Try to get the pending ACK info
            if let Some(pending) = self.ack_manager.get_pending_ack(&message_id) {
                // Emit MessageFailed event if max retries exceeded
                if pending.retry_count >= self.config.reliability.retry.max_retries {
                    let state = self.shared_state.lock().unwrap();
                    state.emit_event(Event::message_failed(
                        message_id.clone(),
                        "Max retries exceeded".to_string(),
                        pending.retry_count,
                    ));
                    drop(state);

                    // Remove from ACK manager
                    self.ack_manager.remove_ack(&message_id);
                }
            }
        }

        // Cleanup expired entries
        self.deduplicator.cleanup_expired();
        self.retry_queue.cleanup_expired();

        Ok(())
    }

    /// Gets the current protocol state.
    pub fn state(&self) -> ProtocolState {
        let state = self.shared_state.lock().unwrap();
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
}

#[cfg(test)]
mod tests {
    use super::*;
    use offline_protocol_transport::{mock::MockTransport, Transport, TransportType};

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
        protocol.transport_manager_mut().add_transport(TransportType::BLE, Box::new(mock_transport));
        
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
        
        protocol.transport_manager_mut().add_transport(TransportType::BLE, Box::new(mock_transport));
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
        protocol.transport_manager_mut().add_transport(TransportType::BLE, Box::new(mock_transport));

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
        protocol.transport_manager_mut().add_transport(TransportType::BLE, Box::new(mock_transport));
        
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
    fn test_config_access() {
        let config = create_test_config();
        let protocol = OfflineProtocol::new(config.clone()).unwrap();

        assert_eq!(protocol.config().app_id, config.app_id);
        assert_eq!(protocol.config().user_id, config.user_id);
    }
}
