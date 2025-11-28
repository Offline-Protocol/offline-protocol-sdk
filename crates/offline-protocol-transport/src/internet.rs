//! Internet transport implementation (TCP/WebSocket).
//!
//! This module provides connectivity via standard internet protocols.
//! It supports both direct TCP connections and WebSocket for web compatibility.

use crate::constants::{
    INTERNET_CONNECTION_TIMEOUT_SECS, INTERNET_DEFAULT_SERVER_ADDRESS,
    INTERNET_HEARTBEAT_INTERVAL_SECS,
};
use crate::{Result, Transport, TransportMetrics, TransportStatus, TransportType};
use offline_protocol_core::Message;
use std::collections::VecDeque;
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

/// Internet transport configuration
#[derive(Debug, Clone)]
pub struct InternetConfig {
    /// Server address (WebSocket URL or TCP address)
    pub server_address: String,
    /// Connection timeout
    pub connection_timeout: Duration,
    /// Enable automatic reconnection
    pub auto_reconnect: bool,
    /// Reconnection delay
    pub reconnect_delay: Duration,
    /// Maximum reconnection attempts (0 = infinite)
    pub max_reconnect_attempts: u32,
}

impl Default for InternetConfig {
    fn default() -> Self {
        Self {
            server_address: INTERNET_DEFAULT_SERVER_ADDRESS.to_string(),
            connection_timeout: Duration::from_secs(INTERNET_CONNECTION_TIMEOUT_SECS),
            auto_reconnect: true,
            reconnect_delay: Duration::from_secs(5),
            max_reconnect_attempts: 0,
        }
    }
}

/// Internet transport implementation.
///
/// This provides connectivity via TCP/WebSocket to a central relay server.
/// Useful for hybrid online/offline scenarios.
pub struct InternetTransport {
    /// Local device ID
    device_id: String,
    /// Configuration
    config: InternetConfig,
    /// Transport status
    status: Arc<Mutex<TransportStatus>>,
    /// Received message queue
    receive_queue: Arc<Mutex<VecDeque<Message>>>,
    /// Send queue
    send_queue: Arc<Mutex<VecDeque<Message>>>,
    /// Transport metrics
    metrics: Arc<Mutex<TransportMetrics>>,
    /// Last heartbeat time
    last_heartbeat: Arc<Mutex<Option<Instant>>>,
    /// Reconnection attempts counter
    reconnect_attempts: Arc<Mutex<u32>>,
    /// Platform-specific handle (opaque pointer to actual connection)
    platform_handle: Arc<Mutex<Option<usize>>>,
}

impl InternetTransport {
    /// Creates a new internet transport.
    pub fn new(device_id: impl Into<String>) -> Self {
        Self::with_config(device_id, InternetConfig::default())
    }

    /// Creates a new internet transport with custom configuration.
    pub fn with_config(device_id: impl Into<String>, config: InternetConfig) -> Self {
        Self {
            device_id: device_id.into(),
            config,
            status: Arc::new(Mutex::new(TransportStatus::Unavailable)),
            receive_queue: Arc::new(Mutex::new(VecDeque::new())),
            send_queue: Arc::new(Mutex::new(VecDeque::new())),
            metrics: Arc::new(Mutex::new(TransportMetrics::default())),
            last_heartbeat: Arc::new(Mutex::new(None)),
            reconnect_attempts: Arc::new(Mutex::new(0)),
            platform_handle: Arc::new(Mutex::new(None)),
        }
    }

    /// Gets the local device ID.
    pub fn device_id(&self) -> &str {
        &self.device_id
    }

    /// Gets the configuration.
    pub fn config(&self) -> &InternetConfig {
        &self.config
    }

    /// Sets the platform-specific handle.
    pub fn set_platform_handle(&self, handle: usize) {
        *self.platform_handle.lock().unwrap() = Some(handle);
    }

    /// Gets the platform-specific handle.
    pub fn platform_handle(&self) -> Option<usize> {
        *self.platform_handle.lock().unwrap()
    }

    /// Called when connection status changes.
    /// 
    /// EDGE CASE HANDLING:
    /// - Messages in send_queue are preserved during disconnection
    /// - They will be sent when transport becomes available again
    /// - Reconnect counter is reset on successful connection
    pub fn on_status_changed(&self, status: TransportStatus) {
        let previous_status = *self.status.lock().unwrap();
        *self.status.lock().unwrap() = status;

        // Reset reconnect counter on successful connection
        if status == TransportStatus::Available {
            *self.reconnect_attempts.lock().unwrap() = 0;
            
            // Log if we have pending messages to send after reconnection
            let queue_len = self.send_queue.lock().unwrap().len();
            if queue_len > 0 {
                tracing::info!(
                    pending_messages = queue_len,
                    "Internet transport available, {} messages pending in queue",
                    queue_len
                );
            }
        } else if previous_status == TransportStatus::Available && status != TransportStatus::Available {
            // Log disconnection with pending messages
            let queue_len = self.send_queue.lock().unwrap().len();
            if queue_len > 0 {
                tracing::warn!(
                    pending_messages = queue_len,
                    new_status = ?status,
                    "Internet transport disconnected with {} messages in queue (will retry)",
                    queue_len
                );
            }
        }
    }

    /// Called when a message is received from the server.
    pub fn on_message_received(&self, message: Message) {
        let mut queue = self.receive_queue.lock().unwrap();
        queue.push_back(message);
    }

    /// Serializes a message to JSON bytes.
    pub fn serialize_message(&self, message: &Message) -> Result<Vec<u8>> {
        serde_json::to_vec(message).map_err(|e| {
            crate::Error::SerializationError(format!("Failed to serialize message: {}", e))
        })
    }

    /// Deserializes a message from JSON bytes.
    pub fn deserialize_message(&self, data: &[u8]) -> Result<Message> {
        serde_json::from_slice(data).map_err(|e| {
            crate::Error::SerializationError(format!("Failed to deserialize message: {}", e))
        })
    }

    /// Called when raw data is received (platform callback).
    ///
    /// This deserializes the message and queues it.
    pub fn on_data_received(&self, data: Vec<u8>) -> Result<()> {
        match self.deserialize_message(&data) {
            Ok(message) => {
                let mut queue = self.receive_queue.lock().unwrap();
                queue.push_back(message);
                Ok(())
            }
            Err(e) => {
                tracing::warn!(error = %e, "Error deserializing message, dropping bad data");
                Ok(()) // Don't fail - just drop bad data
            }
        }
    }

    /// Gets the next message to send (for platform implementation).
    ///
    /// Returns serialized message bytes or None if no messages to send.
    pub fn get_next_message(&self) -> Result<Option<Vec<u8>>> {
        let message = {
            let mut queue = self.send_queue.lock().unwrap();
            match queue.pop_front() {
                Some(m) => m,
                None => return Ok(None),
            }
        };

        // Serialize the message
        let data = self.serialize_message(&message)?;
        Ok(Some(data))
    }

    /// Checks if reconnection should be attempted.
    pub fn should_reconnect(&self) -> bool {
        if !self.config.auto_reconnect {
            return false;
        }

        let attempts = *self.reconnect_attempts.lock().unwrap();
        self.config.max_reconnect_attempts == 0 || attempts < self.config.max_reconnect_attempts
    }

    /// Increments reconnection attempt counter.
    pub fn increment_reconnect_attempts(&self) {
        *self.reconnect_attempts.lock().unwrap() += 1;
    }

    /// Updates heartbeat timestamp.
    pub fn update_heartbeat(&self) {
        *self.last_heartbeat.lock().unwrap() = Some(Instant::now());
    }

    /// Checks if heartbeat is needed.
    pub fn needs_heartbeat(&self) -> bool {
        let last = *self.last_heartbeat.lock().unwrap();
        match last {
            Some(instant) => instant.elapsed() >= Duration::from_secs(INTERNET_HEARTBEAT_INTERVAL_SECS),
            None => true,
        }
    }

    /// Updates transport metrics.
    pub fn update_metrics(&self, metrics: TransportMetrics) {
        *self.metrics.lock().unwrap() = metrics;
    }
}

impl Transport for InternetTransport {
    fn as_any(&self) -> &dyn std::any::Any {
        self
    }

    fn transport_type(&self) -> TransportType {
        TransportType::Internet
    }

    fn status(&self) -> TransportStatus {
        *self.status.lock().unwrap()
    }

    fn metrics(&self) -> TransportMetrics {
        self.metrics.lock().unwrap().clone()
    }

    fn send(&self, message: &Message) -> Result<()> {
        // Check status
        if self.status() != TransportStatus::Available {
            return Err(crate::Error::TransportNotAvailable(
                "Internet transport is not available".to_string(),
            ));
        }

        // Add to send queue
        let mut queue = self.send_queue.lock().unwrap();
        queue.push_back(message.clone());

        // Update metrics
        let mut metrics = self.metrics.lock().unwrap();
        metrics.queue_depth = queue.len();
        metrics.congestion = ((metrics.queue_depth as f32) / 25.0).clamp(0.0, 1.0);

        Ok(())
    }

    fn receive(&self) -> Result<Option<Message>> {
        let mut queue = self.receive_queue.lock().unwrap();
        Ok(queue.pop_front())
    }

    fn start(&mut self) -> Result<()> {
        // Status will be updated by platform implementation via on_status_changed()
        // Platform code should establish connection and call on_status_changed()
        Ok(())
    }

    fn stop(&mut self) -> Result<()> {
        *self.status.lock().unwrap() = TransportStatus::Disconnected;
        Ok(())
    }
}

/// Internet transport builder for configuration.
pub struct InternetTransportBuilder {
    device_id: String,
    config: InternetConfig,
}

impl InternetTransportBuilder {
    /// Creates a new builder.
    pub fn new(device_id: impl Into<String>) -> Self {
        Self {
            device_id: device_id.into(),
            config: InternetConfig::default(),
        }
    }

    /// Sets the server address.
    pub fn server_address(mut self, address: impl Into<String>) -> Self {
        self.config.server_address = address.into();
        self
    }

    /// Sets the connection timeout.
    pub fn connection_timeout(mut self, timeout: Duration) -> Self {
        self.config.connection_timeout = timeout;
        self
    }

    /// Enables or disables automatic reconnection.
    pub fn auto_reconnect(mut self, enabled: bool) -> Self {
        self.config.auto_reconnect = enabled;
        self
    }

    /// Sets the reconnection delay.
    pub fn reconnect_delay(mut self, delay: Duration) -> Self {
        self.config.reconnect_delay = delay;
        self
    }

    /// Sets the maximum reconnection attempts.
    pub fn max_reconnect_attempts(mut self, attempts: u32) -> Self {
        self.config.max_reconnect_attempts = attempts;
        self
    }

    /// Builds the transport.
    pub fn build(self) -> InternetTransport {
        InternetTransport::with_config(self.device_id, self.config)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use offline_protocol_core::{AppId, UserId};

    fn create_test_message() -> Message {
        Message::new(
            UserId::new("alice").unwrap(),
            UserId::new("bob").unwrap(),
            AppId::new("test").unwrap(),
            "Test message",
        )
    }

    #[test]
    fn test_internet_transport_creation() {
        let transport = InternetTransport::new("test-device");
        assert_eq!(transport.device_id(), "test-device");
        assert_eq!(transport.transport_type(), TransportType::Internet);
        assert_eq!(transport.status(), TransportStatus::Unavailable);
    }

    #[test]
    fn test_builder() {
        let transport = InternetTransportBuilder::new("test-device")
            .server_address("ws://example.com:8080")
            .connection_timeout(Duration::from_secs(10))
            .auto_reconnect(false)
            .build();

        assert_eq!(transport.config().server_address, "ws://example.com:8080");
        assert_eq!(
            transport.config().connection_timeout,
            Duration::from_secs(10)
        );
        assert!(!transport.config().auto_reconnect);
    }

    #[test]
    fn test_send_receive() {
        let transport = InternetTransport::new("test-device");

        // Mark as available
        transport.on_status_changed(TransportStatus::Available);

        // Send message
        let message = create_test_message();
        assert!(transport.send(&message).is_ok());

        // Should have message in queue
        let next = transport.get_next_message().unwrap();
        assert!(next.is_some());
    }

    #[test]
    fn test_serialization() {
        let transport = InternetTransport::new("test-device");
        let message = create_test_message();

        // Serialize
        let data = transport.serialize_message(&message).unwrap();
        assert!(!data.is_empty());

        // Deserialize
        let deserialized = transport.deserialize_message(&data).unwrap();
        assert_eq!(deserialized.id, message.id);
    }

    #[test]
    fn test_reconnect_logic() {
        let transport = InternetTransportBuilder::new("test-device")
            .auto_reconnect(true)
            .max_reconnect_attempts(3)
            .build();

        assert!(transport.should_reconnect());

        transport.increment_reconnect_attempts();
        transport.increment_reconnect_attempts();
        transport.increment_reconnect_attempts();

        assert!(!transport.should_reconnect()); // Max attempts reached
    }

    #[test]
    fn test_heartbeat() {
        let transport = InternetTransport::new("test-device");

        assert!(transport.needs_heartbeat());

        transport.update_heartbeat();
        assert!(!transport.needs_heartbeat());
    }
}
