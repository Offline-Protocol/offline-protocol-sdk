//! Internet transport implementation (TCP/WebSocket).
//!
//! This module provides connectivity via standard internet protocols.
//! It supports both direct TCP connections and WebSocket for web compatibility.

use crate::constants::{
    INTERNET_CONNECTION_TIMEOUT_SECS, INTERNET_DEFAULT_SERVER_ADDRESS,
    INTERNET_HEARTBEAT_INTERVAL_SECS, INTERNET_PENDING_CONFIRMATION_TIMEOUT_SECS,
};
use crate::{Result, Transport, TransportMetrics, TransportStatus, TransportType};
use offline_protocol_core::Message;
use std::collections::{HashMap, VecDeque};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

/// Recalculates `delivery_ratio` and `drop_rate` from the current success/failure counts.
fn recalculate_delivery_ratios(metrics: &mut TransportMetrics) {
    let total = metrics.success_count + metrics.failure_count;
    if total > 0 {
        let ratio = metrics.success_count as f32 / total as f32;
        metrics.delivery_ratio = Some(ratio.clamp(0.0, 1.0));
        metrics.drop_rate = Some((1.0 - ratio).clamp(0.0, 1.0));
    }
}

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
    /// Send queue — messages waiting to be polled by the platform via `get_next_message()`
    send_queue: Arc<Mutex<VecDeque<Message>>>,
    /// Messages dequeued by the platform but not yet confirmed as sent.
    /// Key: message_id string, Value: when the message was dequeued.
    pending_confirmation: Arc<Mutex<HashMap<String, Instant>>>,
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
            pending_confirmation: Arc::new(Mutex::new(HashMap::new())),
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
        } else if previous_status == TransportStatus::Available
            && status != TransportStatus::Available
        {
            // Fail all pending confirmations — the connection is gone so the
            // platform can no longer report outcomes for in-flight messages.
            self.fail_all_pending();

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
    /// Returns the message_id and serialized bytes, or None if no messages to send.
    /// The message enters the pending-confirmation state until the platform calls
    /// `confirm_sent()` or `report_send_failure()`.
    ///
    /// Also drains any pending confirmations that have exceeded the timeout,
    /// recording them as failures to keep DORS metrics accurate.
    pub fn get_next_message(&self) -> Result<Option<(String, Vec<u8>)>> {
        // Expire stale pending confirmations before processing the next message.
        // This is the only call site — it runs each time the platform polls, which
        // is frequent enough to bound the pending map and keep metrics fresh.
        self.drain_expired_pending();

        let (message, queue_depth) = {
            let mut queue = self.send_queue.lock().unwrap();
            match queue.pop_front() {
                Some(m) => {
                    let depth = queue.len();
                    (m, depth)
                }
                None => return Ok(None),
            }
        };

        let message_id = message.id.as_str().to_string();

        // Serialize before inserting into pending_confirmation so a
        // serialization failure does not orphan an entry that the platform
        // can never confirm or fail.
        let data = self.serialize_message(&message)?;

        {
            let mut pending = self.pending_confirmation.lock().unwrap();
            pending.insert(message_id.clone(), Instant::now());
        }

        {
            let mut metrics = self.metrics.lock().unwrap();
            metrics.queue_depth = queue_depth;
        }

        Ok(Some((message_id, data)))
    }

    /// Platform confirms that a message was successfully sent over the wire (e.g., WebSocket).
    ///
    /// This updates transport metrics to reflect real delivery outcomes,
    /// enabling DORS to make accurate routing decisions.
    pub fn confirm_sent(&self, message_id: &str) {
        let was_pending = {
            let mut pending = self.pending_confirmation.lock().unwrap();
            pending.remove(message_id).is_some()
        };

        if was_pending {
            let mut metrics = self.metrics.lock().unwrap();
            metrics.success_count = metrics.success_count.saturating_add(1);
            recalculate_delivery_ratios(&mut metrics);
        } else {
            tracing::debug!(
                message_id = message_id,
                "confirm_sent: unknown or already-resolved message"
            );
        }
    }

    /// Platform reports that a message failed to send (e.g., WebSocket error).
    pub fn report_send_failure(&self, message_id: &str) {
        let was_pending = {
            let mut pending = self.pending_confirmation.lock().unwrap();
            pending.remove(message_id).is_some()
        };

        if was_pending {
            let mut metrics = self.metrics.lock().unwrap();
            metrics.failure_count = metrics.failure_count.saturating_add(1);
            recalculate_delivery_ratios(&mut metrics);
        } else {
            tracing::debug!(
                message_id = message_id,
                "report_send_failure: unknown or already-resolved message"
            );
        }
    }

    /// Drains messages that have been pending confirmation longer than the timeout,
    /// recording each as a failure.
    fn drain_expired_pending(&self) {
        let timeout = Duration::from_secs(INTERNET_PENDING_CONFIRMATION_TIMEOUT_SECS);
        let now = Instant::now();
        let mut expired_count: u32 = 0;

        {
            let mut pending = self.pending_confirmation.lock().unwrap();
            pending.retain(|_id, enqueued_at| {
                if now.duration_since(*enqueued_at) >= timeout {
                    expired_count += 1;
                    false
                } else {
                    true
                }
            });
        }

        if expired_count > 0 {
            tracing::warn!(
                expired_count = expired_count,
                "Pending confirmations expired, recorded as failures"
            );
            let mut metrics = self.metrics.lock().unwrap();
            metrics.failure_count = metrics.failure_count.saturating_add(expired_count);
            recalculate_delivery_ratios(&mut metrics);
        }
    }

    /// Fails all pending confirmations immediately.
    ///
    /// Called when the transport disconnects so that metrics reflect the loss
    /// right away rather than waiting for the per-message expiry timeout.
    fn fail_all_pending(&self) {
        let count = {
            let mut pending = self.pending_confirmation.lock().unwrap();
            let count = u32::try_from(pending.len()).unwrap_or(u32::MAX);
            pending.clear();
            count
        };

        if count > 0 {
            tracing::warn!(
                count = count,
                "Failing all pending confirmations due to transport disconnect"
            );
            let mut metrics = self.metrics.lock().unwrap();
            metrics.failure_count = metrics.failure_count.saturating_add(count);
            recalculate_delivery_ratios(&mut metrics);
        }
    }

    /// Returns the count of messages currently awaiting confirmation from the platform.
    pub fn pending_confirmation_count(&self) -> usize {
        self.pending_confirmation.lock().unwrap().len()
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
    /// Uses saturating addition to prevent overflow.
    pub fn increment_reconnect_attempts(&self) {
        let mut attempts = self.reconnect_attempts.lock().unwrap();
        *attempts = attempts.saturating_add(1);
    }

    /// Updates heartbeat timestamp.
    pub fn update_heartbeat(&self) {
        *self.last_heartbeat.lock().unwrap() = Some(Instant::now());
    }

    /// Checks if heartbeat is needed.
    pub fn needs_heartbeat(&self) -> bool {
        let last = *self.last_heartbeat.lock().unwrap();
        match last {
            Some(instant) => {
                instant.elapsed() >= Duration::from_secs(INTERNET_HEARTBEAT_INTERVAL_SECS)
            }
            None => true,
        }
    }

    /// Updates transport metrics while preserving confirmation-loop delivery counts.
    ///
    /// The confirmation loop (`confirm_sent` / `report_send_failure`) owns
    /// `success_count`, `failure_count`, `delivery_ratio`, and `drop_rate`.
    /// This method only copies externally-managed fields from `incoming`,
    /// so adding a new confirmation-loop field doesn't require updating this
    /// function — it's preserved by default.
    pub fn update_metrics(&self, incoming: TransportMetrics) {
        let mut current = self.metrics.lock().unwrap();
        current.rssi = incoming.rssi;
        current.latency_ms = incoming.latency_ms;
        current.bandwidth_bps = incoming.bandwidth_bps;
        current.congestion = incoming.congestion;
        current.queue_depth = incoming.queue_depth;
        current.battery_level = incoming.battery_level;
        current.is_charging = incoming.is_charging;
        current.relay_connection_count = incoming.relay_connection_count;
        current.is_active_relay = incoming.is_active_relay;
        current.average_hop_count = incoming.average_hop_count;
        current.energy_cost = incoming.energy_cost;
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
        let message_id = message.id.as_str().to_string();
        assert!(transport.send(&message).is_ok());

        // Should have message in queue; get_next_message returns (id, bytes)
        let next = transport.get_next_message().unwrap();
        assert!(next.is_some());
        let (returned_id, data) = next.unwrap();
        assert_eq!(returned_id, message_id);
        assert!(!data.is_empty());

        // Message should now be in pending confirmation
        assert_eq!(transport.pending_confirmation_count(), 1);

        // Confirm sent — should update success metrics
        transport.confirm_sent(&returned_id);
        assert_eq!(transport.pending_confirmation_count(), 0);
        let metrics = transport.metrics();
        assert_eq!(metrics.success_count, 1);
    }

    #[test]
    fn test_send_failure_tracking() {
        let transport = InternetTransport::new("test-device");
        transport.on_status_changed(TransportStatus::Available);

        let message = create_test_message();
        assert!(transport.send(&message).is_ok());

        let (msg_id, _data) = transport.get_next_message().unwrap().unwrap();
        assert_eq!(transport.pending_confirmation_count(), 1);

        // Report failure
        transport.report_send_failure(&msg_id);
        assert_eq!(transport.pending_confirmation_count(), 0);
        let metrics = transport.metrics();
        assert_eq!(metrics.failure_count, 1);
        assert_eq!(metrics.success_count, 0);
    }

    #[test]
    fn test_pending_expiry_on_drain() {
        let transport = InternetTransport::new("test-device");
        transport.on_status_changed(TransportStatus::Available);

        let message = create_test_message();
        assert!(transport.send(&message).is_ok());

        let (msg_id, _data) = transport.get_next_message().unwrap().unwrap();
        assert_eq!(transport.pending_confirmation_count(), 1);

        // Backdate the pending entry so it appears expired.
        {
            let mut pending = transport.pending_confirmation.lock().unwrap();
            let expired_time = Instant::now()
                - Duration::from_secs(INTERNET_PENDING_CONFIRMATION_TIMEOUT_SECS + 1);
            pending.insert(msg_id.clone(), expired_time);
        }

        // Calling get_next_message (even with an empty queue) triggers drain.
        let next = transport.get_next_message().unwrap();
        assert!(next.is_none());

        // The expired entry should have been drained and counted as a failure.
        assert_eq!(transport.pending_confirmation_count(), 0);
        let metrics = transport.metrics();
        assert_eq!(metrics.failure_count, 1);
        assert_eq!(metrics.success_count, 0);
    }

    #[test]
    fn test_fail_all_pending_on_disconnect() {
        let transport = InternetTransport::new("test-device");
        transport.on_status_changed(TransportStatus::Available);

        // Enqueue and dequeue two messages to put them in pending state.
        let msg1 = create_test_message();
        let msg2 = create_test_message();
        assert!(transport.send(&msg1).is_ok());
        assert!(transport.send(&msg2).is_ok());

        let _ = transport.get_next_message().unwrap().unwrap();
        let _ = transport.get_next_message().unwrap().unwrap();
        assert_eq!(transport.pending_confirmation_count(), 2);

        // Simulate disconnect — should fail all pending.
        transport.on_status_changed(TransportStatus::Disconnected);
        assert_eq!(transport.pending_confirmation_count(), 0);
        let metrics = transport.metrics();
        assert_eq!(metrics.failure_count, 2);
        assert_eq!(metrics.success_count, 0);
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
