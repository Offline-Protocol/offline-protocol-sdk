//! Mock transport for testing.

use crate::{PeerLink, Result, Transport, TransportMetrics, TransportStatus, TransportType};
use offline_protocol_core::Message;
use std::collections::VecDeque;
use std::sync::{Arc, Mutex};

/// Mock transport for testing purposes.
///
/// This transport simulates message sending and receiving without actual network operations.
#[derive(Clone)]
pub struct MockTransport {
    transport_type: TransportType,
    status: Arc<Mutex<TransportStatus>>,
    sent_messages: Arc<Mutex<Vec<Message>>>,
    receive_queue: Arc<Mutex<VecDeque<Message>>>,
    metrics: Arc<Mutex<TransportMetrics>>,
    /// When > 0, next send() calls fail this many times (for escalation/fallback tests).
    fail_next_sends: Arc<Mutex<usize>>,
    /// Simulated live links, in the order they were registered.
    connected_peers: Arc<Mutex<Vec<PeerLink>>>,
    /// When set, `send` refuses a recipient with no live link, the way a
    /// radio-backed transport does. Off by default so tests that only care
    /// about what was sent need not declare a topology.
    reject_unknown_recipients: Arc<Mutex<bool>>,
    /// Every `send_to_peer` call, as `(peer_id, message)`, so forwarding tests
    /// can assert *which neighbor* a frame crossed and how many times it was
    /// transmitted — the counts that distinguish a controlled flood from a
    /// storm.
    peer_sends: Arc<Mutex<Vec<(String, Message)>>>,
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
            receive_queue: Arc::new(Mutex::new(VecDeque::new())),
            metrics: Arc::new(Mutex::new(TransportMetrics::default())),
            fail_next_sends: Arc::new(Mutex::new(0)),
            connected_peers: Arc::new(Mutex::new(Vec::new())),
            peer_sends: Arc::new(Mutex::new(Vec::new())),
            reject_unknown_recipients: Arc::new(Mutex::new(false)),
        }
    }

    /// Makes `send` behave like a radio-backed transport: a recipient with no
    /// live link is refused rather than silently accepted.
    ///
    /// This is what lets a test exercise the path a message takes when its
    /// recipient is out of range — the case mesh forwarding exists for.
    pub fn set_reject_unknown_recipients(&self, reject: bool) {
        *self.reject_unknown_recipients.lock().unwrap() = reject;
    }

    /// Declares the simulated set of directly connected peers.
    pub fn set_connected_peers(&self, peers: Vec<PeerLink>) {
        *self.connected_peers.lock().unwrap() = peers;
    }

    /// Adds one simulated live link.
    pub fn add_connected_peer(&self, peer_id: impl Into<String>, rssi: i16) {
        self.connected_peers
            .lock()
            .unwrap()
            .push(PeerLink::with_rssi(peer_id, rssi));
    }

    /// Drops one simulated live link (peer churn).
    pub fn remove_connected_peer(&self, peer_id: &str) {
        self.connected_peers
            .lock()
            .unwrap()
            .retain(|link| link.peer_id != peer_id);
    }

    /// Returns every `(peer_id, message)` handed to `send_to_peer`.
    pub fn peer_sends(&self) -> Vec<(String, Message)> {
        self.peer_sends.lock().unwrap().clone()
    }

    /// Number of times `message_id` was transmitted to any peer — the
    /// per-node transmission count a storm test asserts against.
    pub fn peer_send_count_for(&self, message_id: &str) -> usize {
        self.peer_sends
            .lock()
            .unwrap()
            .iter()
            .filter(|(_, message)| message.id.as_str() == message_id)
            .count()
    }

    /// Clears the recorded per-peer sends.
    pub fn clear_peer_sends(&self) {
        self.peer_sends.lock().unwrap().clear();
    }

    /// Makes the next `n` send attempts return an error (for retry-failure / escalation tests).
    pub fn set_fail_next_sends(&self, n: usize) {
        *self.fail_next_sends.lock().unwrap() = n;
    }

    /// Adds a message to the receive queue for testing.
    pub fn queue_message(&self, message: Message) {
        let mut queue = self.receive_queue.lock().unwrap();
        queue.push_back(message);
    }

    /// Adds a message to the receive queue with a transport-verified peer identity.
    pub fn queue_message_from(&self, mut message: Message, peer_id: String) {
        message
            .set_transport_peer_id(peer_id)
            .expect("test must provide non-empty peer_id");
        let mut queue = self.receive_queue.lock().unwrap();
        queue.push_back(message);
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

    /// Overrides the reported status, bypassing `start()`/`stop()`. Used by
    /// tests that drive status transitions directly (e.g. the telemetry
    /// aggregator diff).
    pub fn set_status(&self, status: TransportStatus) {
        *self.status.lock().unwrap() = status;
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
        if *self.reject_unknown_recipients.lock().unwrap() {
            let recipient = message.recipient.as_str();
            let peers = self.connected_peers.lock().unwrap();
            if !peers.iter().any(|link| link.peer_id == recipient) {
                return Err(crate::Error::PeerNotReachable(format!(
                    "mock: no connected peer for recipient {}",
                    recipient
                )));
            }
        }

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

    fn send_to_peer(&self, peer_id: &str, message: &Message) -> Result<()> {
        if self.status() != TransportStatus::Available {
            return Err(crate::Error::TransportNotAvailable(
                "mock transport is not available".to_string(),
            ));
        }

        {
            let peers = self.connected_peers.lock().unwrap();
            if !peers.iter().any(|link| link.peer_id == peer_id) {
                return Err(crate::Error::PeerNotReachable(format!(
                    "mock: {} is not a connected peer",
                    peer_id
                )));
            }
        }

        {
            let mut fail = self.fail_next_sends.lock().unwrap();
            if *fail > 0 {
                *fail = fail.saturating_sub(1);
                return Err(crate::Error::SendFailed("mock fail_next_sends".to_string()));
            }
        }

        self.peer_sends
            .lock()
            .unwrap()
            .push((peer_id.to_string(), message.clone()));
        Ok(())
    }

    fn connected_peers(&self) -> Vec<PeerLink> {
        if self.status() != TransportStatus::Available {
            return Vec::new();
        }
        self.connected_peers.lock().unwrap().clone()
    }

    fn receive(&self) -> Result<Option<Message>> {
        let mut queue = self.receive_queue.lock().unwrap();
        Ok(queue.pop_front())
    }

    fn start(&self) -> Result<()> {
        *self.status.lock().unwrap() = TransportStatus::Available;
        Ok(())
    }

    fn stop(&self) -> Result<()> {
        *self.status.lock().unwrap() = TransportStatus::Disconnected;
        Ok(())
    }

    fn on_status_changed(&self, status: TransportStatus) {
        self.set_status(status);
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use offline_protocol_core::{AppId, UserId};

    fn test_message() -> Message {
        Message::new(
            UserId::new("alice").unwrap(),
            UserId::new("bob").unwrap(),
            AppId::new("test").unwrap(),
            "Test message",
        )
    }

    #[test]
    fn send_to_peer_records_the_hop_not_the_recipient() {
        let transport = MockTransport::new(TransportType::BLE);
        transport.start().unwrap();
        transport.add_connected_peer("carol", -60);

        // A frame addressed to bob, handed across the link to carol.
        let message = test_message();
        transport.send_to_peer("carol", &message).unwrap();

        let sends = transport.peer_sends();
        assert_eq!(sends.len(), 1);
        assert_eq!(sends[0].0, "carol");
        assert_eq!(sends[0].1.recipient.as_str(), "bob");
        assert_eq!(transport.peer_send_count_for(&message.id.as_str()), 1);
    }

    #[test]
    fn send_to_peer_refuses_a_peer_without_a_live_link() {
        let transport = MockTransport::new(TransportType::BLE);
        transport.start().unwrap();
        transport.add_connected_peer("carol", -60);

        // The failure must be synchronous so a forwarding caller can pick a
        // different neighbor instead of assuming the frame is on its way.
        assert!(transport.send_to_peer("dave", &test_message()).is_err());
        assert!(transport.peer_sends().is_empty());
    }

    #[test]
    fn connected_peers_follows_link_state() {
        let transport = MockTransport::new(TransportType::BLE);
        transport.add_connected_peer("carol", -60);

        // Links are only real while the transport itself is available.
        assert!(transport.connected_peers().is_empty());

        transport.start().unwrap();
        assert_eq!(transport.connected_peers().len(), 1);

        transport.remove_connected_peer("carol");
        assert!(transport.connected_peers().is_empty());
    }

    #[test]
    fn test_mock_transport_send_receive() {
        let transport = MockTransport::new(TransportType::BLE);
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
