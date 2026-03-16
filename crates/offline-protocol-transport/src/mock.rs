//! Mock transport for testing.

use crate::{Result, Transport, TransportMetrics, TransportStatus, TransportType};
use offline_protocol_core::Message;
use std::sync::{Arc, Mutex};

/// Mock transport for testing purposes.
///
/// This transport simulates message sending and receiving without actual network operations.
#[derive(Clone)]
pub struct MockTransport {
    transport_type: TransportType,
    status: Arc<Mutex<TransportStatus>>,
    sent_messages: Arc<Mutex<Vec<Message>>>,
    receive_queue: Arc<Mutex<Vec<Message>>>,
    metrics: Arc<Mutex<TransportMetrics>>,
    /// When > 0, next send() calls fail this many times (for escalation/fallback tests).
    fail_next_sends: Arc<Mutex<usize>>,
}

impl std::fmt::Debug for MockTransport {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("MockTransport")
            .field("transport_type", &self.transport_type)
            .finish()
    }
}

impl MockTransport {
    /// Creates a new mock transport.
    pub fn new(transport_type: TransportType) -> Self {
        Self {
            transport_type,
            status: Arc::new(Mutex::new(TransportStatus::Unavailable)),
            sent_messages: Arc::new(Mutex::new(Vec::new())),
            receive_queue: Arc::new(Mutex::new(Vec::new())),
            metrics: Arc::new(Mutex::new(TransportMetrics::default())),
            fail_next_sends: Arc::new(Mutex::new(0)),
        }
    }

    /// Makes the next `n` send attempts return an error (for retry-failure / escalation tests).
    pub fn set_fail_next_sends(&self, n: usize) {
        *self.fail_next_sends.lock().unwrap() = n;
    }

    /// Adds a message to the receive queue for testing.
    pub fn queue_message(&self, message: Message) {
        let mut queue = self.receive_queue.lock().unwrap();
        queue.push(message);
    }

    /// Adds a message to the receive queue with a transport-verified peer identity.
    pub fn queue_message_from(&self, mut message: Message, peer_id: String) {
        message
            .set_transport_peer_id(peer_id)
            .expect("test must provide non-empty peer_id");
        let mut queue = self.receive_queue.lock().unwrap();
        queue.push(message);
    }

    /// Returns all messages that were sent through this transport.
    pub fn sent_messages(&self) -> Vec<Message> {
        self.sent_messages.lock().unwrap().clone()
    }

    /// Clears the sent messages buffer.
    pub fn clear_sent_messages(&self) {
        self.sent_messages.lock().unwrap().clear();
    }

    /// Sets custom metrics for testing.
    pub fn set_metrics(&self, metrics: TransportMetrics) {
        *self.metrics.lock().unwrap() = metrics;
    }
}

impl Transport for MockTransport {
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
        self.metrics.lock().unwrap().clone()
    }

    fn send(&self, message: &Message) -> Result<()> {
        {
            let mut fail = self.fail_next_sends.lock().unwrap();
            if *fail > 0 {
                *fail = fail.saturating_sub(1);
                let mut metrics = self.metrics.lock().unwrap();
                metrics.failure_count += 1;
                let total = metrics.success_count + metrics.failure_count;
                if total > 0 {
                    metrics.delivery_ratio =
                        Some((metrics.success_count as f32 / total as f32).clamp(0.0, 1.0));
                    metrics.drop_rate =
                        Some((1.0 - metrics.delivery_ratio.unwrap()).clamp(0.0, 1.0));
                }
                return Err(crate::Error::SendFailed("mock fail_next_sends".to_string()));
            }
        }

        let mut sent = self.sent_messages.lock().unwrap();
        sent.push(message.clone());

        // Update metrics
        let mut metrics = self.metrics.lock().unwrap();
        metrics.success_count += 1;
        metrics.queue_depth = metrics.queue_depth.saturating_sub(1);
        metrics.congestion = (metrics.queue_depth as f32 / 10.0).clamp(0.0, 1.0);
        let total = metrics.success_count + metrics.failure_count;
        if total > 0 {
            let ratio = metrics.success_count as f32 / total as f32;
            metrics.delivery_ratio = Some(ratio.clamp(0.0, 1.0));
            metrics.drop_rate = Some((1.0 - ratio).clamp(0.0, 1.0));
        }

        Ok(())
    }

    fn receive(&self) -> Result<Option<Message>> {
        let mut queue = self.receive_queue.lock().unwrap();
        Ok(queue.pop())
    }

    fn start(&mut self) -> Result<()> {
        *self.status.lock().unwrap() = TransportStatus::Available;
        Ok(())
    }

    fn stop(&mut self) -> Result<()> {
        *self.status.lock().unwrap() = TransportStatus::Disconnected;
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use offline_protocol_core::{AppId, UserId};

    #[test]
    fn test_mock_transport_send_receive() {
        let mut transport = MockTransport::new(TransportType::BLE);
        transport.start().unwrap();

        assert_eq!(transport.status(), TransportStatus::Available);

        let message = Message::new(
            UserId::new("alice").unwrap(),
            UserId::new("bob").unwrap(),
            AppId::new("test").unwrap(),
            "Test message",
        );

        // Send a message
        transport.send(&message).unwrap();
        assert_eq!(transport.sent_messages().len(), 1);

        // Queue a message for receiving
        transport.queue_message(message.clone());
        let received = transport.receive().unwrap();
        assert!(received.is_some());
    }
}
