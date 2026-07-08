//! Reticulum mesh transport queue engine.
//!
//! Long-range, low-bandwidth, resilient mesh networking via the Reticulum
//! network stack (LoRa, TCP, UDP, serial, I2P, and other mediums). No
//! Reticulum link is opened here: the platform side bridges to a running
//! Reticulum daemon (sidecar, TCP gateway, or embedded Python); the Rust
//! side manages queues, metrics, and the confirmation loop.
//!
//! The bridge contract: the platform reports daemon connectivity via
//! [`ReticulumTransport::on_status_changed`], drains outbound wire bytes
//! with [`ReticulumTransport::get_next_message`] (woken by the
//! [`ReticulumTransport::set_on_messages_available`] callback), reports
//! outcomes via [`ReticulumTransport::confirm_sent`] /
//! [`ReticulumTransport::report_send_failure`], and injects inbound bytes
//! via [`ReticulumTransport::on_data_received`].

use crate::constants::{
    RETICULUM_CONNECTION_TIMEOUT_SECS, RETICULUM_PENDING_CONFIRMATION_TIMEOUT_SECS,
};
use crate::{Result, SharedCallback, Transport, TransportMetrics, TransportStatus, TransportType};
use offline_protocol_core::{Message, MutexExt};
use std::collections::{HashMap, VecDeque};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use crate::common::recalculate_delivery_ratios;

/// Reticulum transport configuration.
#[derive(Debug, Clone)]
pub struct ReticulumConfig {
    /// Connection timeout for reaching the Reticulum daemon.
    pub connection_timeout: Duration,
    /// Enable automatic reconnection to the Reticulum daemon.
    pub auto_reconnect: bool,
    /// Reconnection delay.
    pub reconnect_delay: Duration,
    /// Maximum reconnection attempts (0 = infinite).
    pub max_reconnect_attempts: u32,
}

impl Default for ReticulumConfig {
    fn default() -> Self {
        Self {
            connection_timeout: Duration::from_secs(RETICULUM_CONNECTION_TIMEOUT_SECS),
            auto_reconnect: true,
            reconnect_delay: Duration::from_secs(5),
            max_reconnect_attempts: 0,
        }
    }
}

/// Reticulum mesh transport implementation.
///
/// Provides connectivity via the Reticulum network for long-range,
/// low-bandwidth, resilient mesh networking. The platform bridges to a
/// Reticulum daemon (sidecar, TCP gateway, or embedded Python).
///
/// ## Lock ordering
///
/// When acquiring more than one lock in a single scope, follow this order:
///
/// 1. `status`
/// 2. `pending_confirmation`
/// 3. `send_queue`
/// 4. `metrics`
/// 5. `receive_queue`
/// 6. `reconnect_attempts` / `platform_handle`
pub struct ReticulumTransport {
    device_id: String,
    config: ReticulumConfig,
    status: Arc<Mutex<TransportStatus>>,
    receive_queue: Arc<Mutex<VecDeque<Message>>>,
    send_queue: Arc<Mutex<VecDeque<Message>>>,
    /// Messages dequeued by the platform but not yet confirmed as sent.
    pending_confirmation: Arc<Mutex<HashMap<String, Instant>>>,
    metrics: Arc<Mutex<TransportMetrics>>,
    reconnect_attempts: Arc<Mutex<u32>>,
    platform_handle: Arc<Mutex<Option<usize>>>,
    on_messages_available: SharedCallback,
}

impl ReticulumTransport {
    /// Creates a new Reticulum transport with default configuration.
    pub fn new(device_id: impl Into<String>) -> Self {
        Self::with_config(device_id, ReticulumConfig::default())
    }

    /// Creates a new Reticulum transport with custom configuration.
    pub fn with_config(device_id: impl Into<String>, config: ReticulumConfig) -> Self {
        Self {
            device_id: device_id.into(),
            config,
            status: Arc::new(Mutex::new(TransportStatus::Unavailable)),
            receive_queue: Arc::new(Mutex::new(VecDeque::new())),
            send_queue: Arc::new(Mutex::new(VecDeque::new())),
            pending_confirmation: Arc::new(Mutex::new(HashMap::new())),
            metrics: Arc::new(Mutex::new(TransportMetrics::default())),
            reconnect_attempts: Arc::new(Mutex::new(0)),
            platform_handle: Arc::new(Mutex::new(None)),
            on_messages_available: Arc::new(Mutex::new(None)),
        }
    }

    /// Gets the local device ID.
    pub fn device_id(&self) -> &str {
        &self.device_id
    }

    /// Gets the configuration.
    pub fn config(&self) -> &ReticulumConfig {
        &self.config
    }

    /// Sets the platform-specific handle.
    pub fn set_platform_handle(&self, handle: usize) {
        crate::common::set_platform_handle(&self.platform_handle, handle);
    }

    /// Gets the platform-specific handle.
    pub fn platform_handle(&self) -> Option<usize> {
        crate::common::platform_handle(&self.platform_handle)
    }

    /// Notifies the platform that messages are ready to send.
    ///
    /// The callback Arc is cloned out of the mutex and the guard dropped
    /// before the call, so a callback that re-enters the transport (e.g.
    /// another `send()`) cannot self-deadlock on the callback mutex.
    fn notify_messages_available(&self) {
        let callback = self.on_messages_available.lock_or_recover().clone();
        if let Some(cb) = callback {
            cb();
        }
    }

    /// Called when a message is received.
    pub fn on_message_received(&self, message: Message) {
        crate::common::on_message_received(&self.receive_queue, message);
    }

    /// Like [`on_message_received`](Self::on_message_received), but attaches a
    /// transport-verified `peer_id` to the message.
    pub fn on_message_received_from(&self, message: Message, peer_id: String) {
        crate::common::on_message_received_from(&self.receive_queue, message, peer_id);
    }

    /// Serializes a message to JSON bytes.
    pub fn serialize_message(&self, message: &Message) -> Result<Vec<u8>> {
        crate::common::serialize_message(message)
    }

    /// Whether the transport should attempt reconnection.
    pub fn should_reconnect(&self) -> bool {
        if !self.config.auto_reconnect {
            return false;
        }
        if self.config.max_reconnect_attempts == 0 {
            return true;
        }
        *self.reconnect_attempts.lock_or_recover() < self.config.max_reconnect_attempts
    }

    /// Increments the reconnection attempt counter.
    pub fn increment_reconnect_attempts(&self) {
        let mut attempts = self.reconnect_attempts.lock_or_recover();
        *attempts = attempts.saturating_add(1);
    }

    /// Updates transport metrics, preserving confirmation-loop counts.
    pub fn update_metrics(&self, incoming: TransportMetrics) {
        let mut metrics = self.metrics.lock_or_recover();
        let prev_success = metrics.success_count;
        let prev_failure = metrics.failure_count;
        *metrics = incoming;
        metrics.success_count = prev_success;
        metrics.failure_count = prev_failure;
        recalculate_delivery_ratios(&mut metrics);
    }

    /// Checks if there are messages waiting to be sent.
    pub fn has_pending_sends(&self) -> bool {
        !self.send_queue.lock_or_recover().is_empty()
    }

    /// Fails all pending confirmations and records them as failures.
    fn fail_all_pending(&self) {
        let pending = {
            let mut map = self.pending_confirmation.lock_or_recover();
            let count = map.len();
            map.clear();
            count
        };
        if pending > 0 {
            let mut metrics = self.metrics.lock_or_recover();
            metrics.failure_count = metrics.failure_count.saturating_add(pending as u32);
            recalculate_delivery_ratios(&mut metrics);
        }
    }

    /// Expires pending confirmations that have exceeded the timeout.
    fn drain_expired_pending(&self) {
        let timeout = Duration::from_secs(RETICULUM_PENDING_CONFIRMATION_TIMEOUT_SECS);
        let now = Instant::now();
        let mut expired_count = 0u32;

        {
            let mut pending = self.pending_confirmation.lock_or_recover();
            pending.retain(|_, enqueued_at| {
                if now.duration_since(*enqueued_at) > timeout {
                    expired_count += 1;
                    false
                } else {
                    true
                }
            });
        }

        if expired_count > 0 {
            let mut metrics = self.metrics.lock_or_recover();
            metrics.failure_count = metrics.failure_count.saturating_add(expired_count);
            recalculate_delivery_ratios(&mut metrics);
        }
    }
}

impl Transport for ReticulumTransport {
    fn as_any(&self) -> &dyn std::any::Any {
        self
    }

    fn transport_type(&self) -> TransportType {
        TransportType::Reticulum
    }

    fn status(&self) -> TransportStatus {
        *self.status.lock_or_recover()
    }

    fn metrics(&self) -> TransportMetrics {
        self.metrics.lock_or_recover().clone()
    }

    fn send(&self, message: &Message) -> Result<()> {
        let status = *self.status.lock_or_recover();
        if status != TransportStatus::Available {
            return Err(crate::Error::TransportNotAvailable(format!(
                "Reticulum transport is {:?}",
                status
            )));
        }

        let queue_len = {
            let mut queue = self.send_queue.lock_or_recover();
            queue.push_back(message.clone());
            queue.len()
        };

        let mut metrics = self.metrics.lock_or_recover();
        metrics.queue_depth = queue_len;
        metrics.congestion = ((queue_len as f32) / 50.0).clamp(0.0, 1.0);
        drop(metrics);

        self.notify_messages_available();

        Ok(())
    }

    fn receive(&self) -> Result<Option<Message>> {
        let mut queue = self.receive_queue.lock_or_recover();
        Ok(queue.pop_front())
    }

    fn start(&mut self) -> Result<()> {
        // Actual connection is managed by the platform.
        // Status is updated via on_status_changed().
        Ok(())
    }

    fn stop(&mut self) -> Result<()> {
        *self.status.lock_or_recover() = TransportStatus::Disconnected;
        self.fail_all_pending();
        self.send_queue.lock_or_recover().clear();
        self.receive_queue.lock_or_recover().clear();
        Ok(())
    }

    /// Called when connection status changes.
    ///
    /// Resets reconnect counter on successful connection.
    /// Fails all pending confirmations on disconnect.
    fn on_status_changed(&self, status: TransportStatus) {
        let previous_status = {
            let mut guard = self.status.lock_or_recover();
            let prev = *guard;
            *guard = status;
            prev
        };

        if status == TransportStatus::Available {
            let queue_len = self.send_queue.lock_or_recover().len();
            *self.reconnect_attempts.lock_or_recover() = 0;

            if queue_len > 0 {
                tracing::info!(
                    pending_messages = queue_len,
                    "Reticulum transport available, {} messages pending in queue",
                    queue_len
                );
            }
        } else if previous_status == TransportStatus::Available
            && status != TransportStatus::Available
        {
            self.fail_all_pending();

            let queue_len = self.send_queue.lock_or_recover().len();
            if queue_len > 0 {
                tracing::warn!(
                    pending_messages = queue_len,
                    new_status = ?status,
                    "Reticulum transport disconnected with {} messages in queue (will retry)",
                    queue_len
                );
            }
        }
    }

    fn on_data_received(&self, data: Vec<u8>) -> Result<()> {
        crate::common::on_data_received(&self.receive_queue, data)
    }

    /// Like [`Transport::on_data_received`], but attaches a
    /// transport-verified `peer_id` to the deserialized message.
    fn on_data_received_from(&self, data: Vec<u8>, peer_id: String) -> Result<()> {
        crate::common::on_data_received_from(&self.receive_queue, data, peer_id)
    }

    /// Gets the next message to send (for platform implementation).
    ///
    /// Returns `(message_id, serialized_bytes)` or `None` if no messages.
    /// The message enters the pending-confirmation state until the platform
    /// calls [`Transport::confirm_sent`] or [`Transport::report_send_failure`].
    fn get_next_message(&self) -> Result<Option<(String, Vec<u8>)>> {
        self.drain_expired_pending();

        let message = {
            let mut queue = self.send_queue.lock_or_recover();
            match queue.pop_front() {
                Some(m) => m,
                None => return Ok(None),
            }
        };

        let message_id = message.id.to_string();
        let data = self.serialize_message(&message)?;

        self.pending_confirmation
            .lock_or_recover()
            .insert(message_id.clone(), Instant::now());

        Ok(Some((message_id, data)))
    }

    /// Sets the callback invoked when outgoing messages are queued.
    fn set_on_messages_available(&self, callback: Arc<dyn Fn() + Send + Sync>) {
        *self.on_messages_available.lock_or_recover() = Some(callback);
    }

    /// Platform confirms a message was sent successfully.
    fn confirm_sent(&self, message_id: &str) {
        let removed = self
            .pending_confirmation
            .lock_or_recover()
            .remove(message_id);

        if removed.is_some() {
            let mut metrics = self.metrics.lock_or_recover();
            metrics.success_count = metrics.success_count.saturating_add(1);
            recalculate_delivery_ratios(&mut metrics);
        }
    }

    /// Platform reports a send failure.
    fn report_send_failure(&self, message_id: &str) {
        let removed = self
            .pending_confirmation
            .lock_or_recover()
            .remove(message_id);

        if removed.is_some() {
            let mut metrics = self.metrics.lock_or_recover();
            metrics.failure_count = metrics.failure_count.saturating_add(1);
            recalculate_delivery_ratios(&mut metrics);
        }
    }
}

/// Builder for [`ReticulumTransport`].
pub struct ReticulumTransportBuilder {
    device_id: String,
    config: ReticulumConfig,
}

impl ReticulumTransportBuilder {
    /// Creates a new builder.
    pub fn new(device_id: impl Into<String>) -> Self {
        Self {
            device_id: device_id.into(),
            config: ReticulumConfig::default(),
        }
    }

    /// Sets the connection timeout.
    pub fn connection_timeout(mut self, timeout: Duration) -> Self {
        self.config.connection_timeout = timeout;
        self
    }

    /// Sets whether to auto-reconnect.
    pub fn auto_reconnect(mut self, auto_reconnect: bool) -> Self {
        self.config.auto_reconnect = auto_reconnect;
        self
    }

    /// Sets the reconnection delay.
    pub fn reconnect_delay(mut self, delay: Duration) -> Self {
        self.config.reconnect_delay = delay;
        self
    }

    /// Sets the maximum reconnection attempts.
    pub fn max_reconnect_attempts(mut self, max: u32) -> Self {
        self.config.max_reconnect_attempts = max;
        self
    }

    /// Builds the transport.
    pub fn build(self) -> ReticulumTransport {
        ReticulumTransport::with_config(self.device_id, self.config)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::constants::DEFAULT_MAX_MESSAGE_SIZE;
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
    fn test_reticulum_transport_creation() {
        let transport = ReticulumTransport::new("device1");
        assert_eq!(transport.device_id(), "device1");
        assert_eq!(transport.transport_type(), TransportType::Reticulum);
        assert_eq!(transport.status(), TransportStatus::Unavailable);
    }

    #[test]
    fn test_builder() {
        let transport = ReticulumTransportBuilder::new("device1")
            .connection_timeout(Duration::from_secs(30))
            .auto_reconnect(false)
            .reconnect_delay(Duration::from_secs(10))
            .max_reconnect_attempts(5)
            .build();
        assert_eq!(
            transport.config().connection_timeout,
            Duration::from_secs(30)
        );
        assert!(!transport.config().auto_reconnect);
        assert_eq!(transport.config().reconnect_delay, Duration::from_secs(10));
        assert_eq!(transport.config().max_reconnect_attempts, 5);
    }

    #[test]
    fn test_send_receive() {
        let mut transport = ReticulumTransport::new("device1");
        transport.start().unwrap();
        transport.on_status_changed(TransportStatus::Available);

        let msg = create_test_message();
        transport.send(&msg).unwrap();

        let (msg_id, data) = transport.get_next_message().unwrap().unwrap();
        assert!(!msg_id.is_empty());
        assert!(!data.is_empty());

        let deserialized = transport.deserialize_message(&data).unwrap();
        assert_eq!(deserialized.id, msg.id);
    }

    #[test]
    fn test_send_when_unavailable_fails() {
        let transport = ReticulumTransport::new("device1");
        let msg = create_test_message();
        assert!(transport.send(&msg).is_err());
    }

    #[test]
    fn test_receive_when_empty_returns_none() {
        let transport = ReticulumTransport::new("device1");
        assert!(transport.receive().unwrap().is_none());
    }

    #[test]
    fn test_confirmation_loop() {
        let mut transport = ReticulumTransport::new("device1");
        transport.start().unwrap();
        transport.on_status_changed(TransportStatus::Available);

        let msg = create_test_message();
        transport.send(&msg).unwrap();

        let (msg_id, _) = transport.get_next_message().unwrap().unwrap();
        transport.confirm_sent(&msg_id);

        let metrics = transport.metrics();
        assert_eq!(metrics.success_count, 1);
        assert_eq!(metrics.failure_count, 0);
    }

    #[test]
    fn test_send_failure_reporting() {
        let mut transport = ReticulumTransport::new("device1");
        transport.start().unwrap();
        transport.on_status_changed(TransportStatus::Available);

        let msg = create_test_message();
        transport.send(&msg).unwrap();

        let (msg_id, _) = transport.get_next_message().unwrap().unwrap();
        transport.report_send_failure(&msg_id);

        let metrics = transport.metrics();
        assert_eq!(metrics.success_count, 0);
        assert_eq!(metrics.failure_count, 1);
    }

    #[test]
    fn test_fail_all_pending_on_disconnect() {
        let mut transport = ReticulumTransport::new("device1");
        transport.start().unwrap();
        transport.on_status_changed(TransportStatus::Available);

        let msg = create_test_message();
        transport.send(&msg).unwrap();
        let _ = transport.get_next_message().unwrap();

        transport.on_status_changed(TransportStatus::Disconnected);

        let metrics = transport.metrics();
        assert_eq!(metrics.failure_count, 1);
    }

    #[test]
    fn test_stop_fails_pending() {
        let mut transport = ReticulumTransport::new("device1");
        transport.start().unwrap();
        transport.on_status_changed(TransportStatus::Available);

        let msg = create_test_message();
        transport.send(&msg).unwrap();
        let _ = transport.get_next_message().unwrap();

        transport.stop().unwrap();

        let metrics = transport.metrics();
        assert_eq!(metrics.failure_count, 1);
    }

    #[test]
    fn test_serialization() {
        let transport = ReticulumTransport::new("device1");
        let msg = create_test_message();
        let data = transport.serialize_message(&msg).unwrap();
        let deserialized = transport.deserialize_message(&data).unwrap();
        assert_eq!(deserialized.id, msg.id);
    }

    #[test]
    fn test_reconnect_logic() {
        let transport = ReticulumTransportBuilder::new("device1")
            .max_reconnect_attempts(3)
            .build();

        assert!(transport.should_reconnect());
        transport.increment_reconnect_attempts();
        transport.increment_reconnect_attempts();
        assert!(transport.should_reconnect());
        transport.increment_reconnect_attempts();
        assert!(!transport.should_reconnect());
    }

    #[test]
    fn test_on_data_received_invalid_json_drops_ok() {
        let transport = ReticulumTransport::new("device1");
        let result = transport.on_data_received(b"not json".to_vec());
        assert!(result.is_ok());
        assert!(transport.receive().unwrap().is_none());
    }

    #[test]
    fn test_on_data_received_rejects_oversized_payload() {
        let transport = ReticulumTransport::new("device1");
        let oversized = vec![0u8; DEFAULT_MAX_MESSAGE_SIZE + 1];
        let result = transport.on_data_received(oversized);
        assert!(result.is_err());
    }

    #[test]
    fn test_on_messages_available_callback() {
        let mut transport = ReticulumTransport::new("device1");
        transport.start().unwrap();
        transport.on_status_changed(TransportStatus::Available);

        let called = Arc::new(Mutex::new(false));
        let called_clone = Arc::clone(&called);
        transport.set_on_messages_available(Arc::new(move || {
            *called_clone.lock().unwrap() = true;
        }));

        let msg = create_test_message();
        transport.send(&msg).unwrap();
        assert!(*called.lock().unwrap());
    }

    #[test]
    fn test_messages_available_callback_reentrant_send() {
        use std::sync::atomic::{AtomicBool, Ordering};

        let transport = Arc::new(ReticulumTransport::new("device1"));
        transport.on_status_changed(TransportStatus::Available);

        let reentered = Arc::new(AtomicBool::new(false));
        let reentered_clone = Arc::clone(&reentered);
        let transport_clone = Arc::clone(&transport);
        transport.set_on_messages_available(Arc::new(move || {
            // Re-enters send() from inside the callback. If send() held the
            // callback mutex across this call, the inner send would
            // self-deadlock re-locking it.
            if !reentered_clone.swap(true, Ordering::SeqCst) {
                transport_clone.send(&create_test_message()).unwrap();
            }
        }));

        transport.send(&create_test_message()).unwrap();

        assert!(reentered.load(Ordering::SeqCst));
        assert_eq!(transport.send_queue.lock().unwrap().len(), 2);
    }

    #[test]
    fn test_update_metrics_preserves_confirmation_counts() {
        let mut transport = ReticulumTransport::new("device1");
        transport.start().unwrap();
        transport.on_status_changed(TransportStatus::Available);

        let msg = create_test_message();
        transport.send(&msg).unwrap();
        let (msg_id, _) = transport.get_next_message().unwrap().unwrap();
        transport.confirm_sent(&msg_id);

        let mut new_metrics = TransportMetrics::default();
        new_metrics.rssi = Some(-70);
        transport.update_metrics(new_metrics);

        let metrics = transport.metrics();
        assert_eq!(metrics.success_count, 1);
        assert_eq!(metrics.rssi, Some(-70));
    }

    #[test]
    fn test_platform_handle() {
        let transport = ReticulumTransport::new("device1");
        assert!(transport.platform_handle().is_none());
        transport.set_platform_handle(42);
        assert_eq!(transport.platform_handle(), Some(42));
    }

    #[test]
    fn test_drain_expired_pending_expires_old_entries() {
        let mut transport = ReticulumTransport::new("device1");
        transport.start().unwrap();
        transport.on_status_changed(TransportStatus::Available);

        // Insert a pending entry that is already past the timeout by backdating it.
        let timeout_secs = RETICULUM_PENDING_CONFIRMATION_TIMEOUT_SECS;
        let expired_at = Instant::now() - Duration::from_secs(timeout_secs + 1);
        transport
            .pending_confirmation
            .lock()
            .unwrap()
            .insert("expired-msg".to_string(), expired_at);

        // Insert a recent pending entry that should survive.
        transport
            .pending_confirmation
            .lock()
            .unwrap()
            .insert("recent-msg".to_string(), Instant::now());

        transport.drain_expired_pending();

        let pending = transport.pending_confirmation.lock().unwrap();
        assert!(
            !pending.contains_key("expired-msg"),
            "Expired entry should have been drained"
        );
        assert!(
            pending.contains_key("recent-msg"),
            "Recent entry should be retained"
        );
        drop(pending);

        let metrics = transport.metrics();
        assert_eq!(
            metrics.failure_count, 1,
            "Expired entry should be counted as a failure"
        );
    }

    #[test]
    fn test_has_pending_sends() {
        let mut transport = ReticulumTransport::new("device1");
        transport.start().unwrap();
        transport.on_status_changed(TransportStatus::Available);

        assert!(!transport.has_pending_sends());

        let msg = create_test_message();
        transport.send(&msg).unwrap();
        assert!(transport.has_pending_sends());
    }
}
