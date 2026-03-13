//! Wi-Fi Direct transport implementation (Android P2P).
//!
//! This module provides high-bandwidth peer-to-peer connectivity via Wi-Fi Direct.
//! This is primarily for Android devices and offers faster data transfer than BLE.

// Constants are defined but not used in Rust code (used in native Android code)
use crate::{Result, SharedCallback, Transport, TransportMetrics, TransportStatus, TransportType};
use offline_protocol_core::Message;
use std::collections::{HashMap, VecDeque};
use std::sync::{Arc, Mutex};
use std::time::SystemTime;

/// Peer device information for Wi-Fi Direct
#[derive(Debug, Clone)]
pub struct WifiDirectPeer {
    /// Device name
    pub device_name: String,
    /// Device address (MAC address)
    pub device_address: String,
    /// Is this device the group owner?
    pub is_group_owner: bool,
    /// Last seen timestamp
    pub last_seen: SystemTime,
    /// Connection status
    pub connected: bool,
}

/// Wi-Fi Direct transport configuration
#[derive(Debug, Clone)]
pub struct WifiDirectConfig {
    /// Device name to advertise
    pub device_name: String,
    /// Enable autonomous group owner negotiation
    pub auto_accept: bool,
    /// Group owner intent (0-15, higher = more likely to be GO)
    pub group_owner_intent: u8,
}

impl Default for WifiDirectConfig {
    fn default() -> Self {
        Self {
            device_name: "OfflineProtocolDevice".to_string(),
            auto_accept: false,
            group_owner_intent: 7,
        }
    }
}

/// Wi-Fi Direct transport implementation.
///
/// This provides high-bandwidth P2P connectivity via Wi-Fi Direct (Android).
/// Offers much higher throughput than BLE for large data transfers.
pub struct WifiDirectTransport {
    /// Local device ID
    device_id: String,
    /// Configuration
    config: WifiDirectConfig,
    /// Transport status
    status: Arc<Mutex<TransportStatus>>,
    /// Discovered peers
    peers: Arc<Mutex<HashMap<String, WifiDirectPeer>>>,
    /// Received message queue
    receive_queue: Arc<Mutex<VecDeque<Message>>>,
    /// Send queue
    send_queue: Arc<Mutex<VecDeque<(String, Message)>>>,
    /// Transport metrics
    metrics: Arc<Mutex<TransportMetrics>>,
    /// Platform-specific handle (opaque pointer to Android WifiP2pManager)
    platform_handle: Arc<Mutex<Option<usize>>>,
    /// Platform callback invoked when new messages are available to send.
    on_messages_available: SharedCallback,
}

impl WifiDirectTransport {
    /// Creates a new Wi-Fi Direct transport.
    pub fn new(device_id: impl Into<String>) -> Self {
        Self::with_config(device_id, WifiDirectConfig::default())
    }

    /// Creates a new Wi-Fi Direct transport with custom configuration.
    pub fn with_config(device_id: impl Into<String>, config: WifiDirectConfig) -> Self {
        Self {
            device_id: device_id.into(),
            config,
            status: Arc::new(Mutex::new(TransportStatus::Unavailable)),
            peers: Arc::new(Mutex::new(HashMap::new())),
            receive_queue: Arc::new(Mutex::new(VecDeque::new())),
            send_queue: Arc::new(Mutex::new(VecDeque::new())),
            metrics: Arc::new(Mutex::new(TransportMetrics::default())),
            platform_handle: Arc::new(Mutex::new(None)),
            on_messages_available: Arc::new(Mutex::new(None)),
        }
    }

    /// Registers a callback that fires when new outgoing messages become available.
    pub fn set_on_messages_available(&self, callback: Arc<dyn Fn() + Send + Sync>) {
        *self.on_messages_available.lock().unwrap() = Some(callback);
    }

    /// Notifies the platform that messages are ready to send.
    fn notify_messages_available(&self) {
        let callback = self.on_messages_available.lock().unwrap().clone();
        if let Some(cb) = callback {
            cb();
        }
    }

    /// Gets the local device ID.
    pub fn device_id(&self) -> &str {
        &self.device_id
    }

    /// Gets the configuration.
    pub fn config(&self) -> &WifiDirectConfig {
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

    /// Called when a peer device is discovered.
    pub fn on_peer_discovered(&self, peer: WifiDirectPeer) {
        let mut peers = self.peers.lock().unwrap();
        peers.insert(peer.device_address.clone(), peer);
    }

    /// Called when a peer device is lost.
    pub fn on_peer_lost(&self, device_address: &str) {
        let mut peers = self.peers.lock().unwrap();
        peers.remove(device_address);
    }

    /// Called when connection status changes.
    pub fn on_status_changed(&self, status: TransportStatus) {
        *self.status.lock().unwrap() = status;
    }

    /// Called when a message is received from a peer.
    pub fn on_message_received(&self, message: Message) {
        let mut queue = self.receive_queue.lock().unwrap();
        queue.push_back(message);
    }

    /// Gets all discovered peers.
    pub fn get_peers(&self) -> Vec<WifiDirectPeer> {
        let peers = self.peers.lock().unwrap();
        peers.values().cloned().collect()
    }

    /// Gets a specific peer by device address.
    pub fn get_peer(&self, device_address: &str) -> Option<WifiDirectPeer> {
        let peers = self.peers.lock().unwrap();
        peers.get(device_address).cloned()
    }

    /// Updates transport metrics.
    pub fn update_metrics(&self, metrics: TransportMetrics) {
        *self.metrics.lock().unwrap() = metrics;
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
    /// Returns (recipient, serialized_data) or None if no messages to send.
    pub fn get_next_message(&self) -> Result<Option<(String, Vec<u8>)>> {
        let (recipient, message) = {
            let mut queue = self.send_queue.lock().unwrap();
            match queue.pop_front() {
                Some((r, m)) => (r, m),
                None => return Ok(None),
            }
        };

        // Serialize the message
        let data = self.serialize_message(&message)?;
        Ok(Some((recipient, data)))
    }

    /// Checks if there are messages to send.
    pub fn has_pending_sends(&self) -> bool {
        let queue = self.send_queue.lock().unwrap();
        !queue.is_empty()
    }
}

impl Transport for WifiDirectTransport {
    fn as_any(&self) -> &dyn std::any::Any {
        self
    }

    fn transport_type(&self) -> TransportType {
        TransportType::WiFiDirect
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
                "Wi-Fi Direct transport is not available".to_string(),
            ));
        }

        // Determine recipient and add to send queue
        let recipient = message.recipient.as_str().to_string();
        {
            let mut queue = self.send_queue.lock().unwrap();
            queue.push_back((recipient, message.clone()));

            // Update metrics
            let mut metrics = self.metrics.lock().unwrap();
            metrics.queue_depth = queue.len();
            metrics.congestion = ((metrics.queue_depth as f32) / 20.0).clamp(0.0, 1.0);
        }

        // Notify platform that messages are available to send.
        self.notify_messages_available();

        Ok(())
    }

    fn receive(&self) -> Result<Option<Message>> {
        let mut queue = self.receive_queue.lock().unwrap();
        Ok(queue.pop_front())
    }

    fn start(&mut self) -> Result<()> {
        // Status will be updated by platform implementation via on_status_changed()
        Ok(())
    }

    fn stop(&mut self) -> Result<()> {
        *self.status.lock().unwrap() = TransportStatus::Disconnected;
        Ok(())
    }
}

/// Wi-Fi Direct transport builder for configuration.
pub struct WifiDirectTransportBuilder {
    device_id: String,
    config: WifiDirectConfig,
}

impl WifiDirectTransportBuilder {
    /// Creates a new builder.
    pub fn new(device_id: impl Into<String>) -> Self {
        Self {
            device_id: device_id.into(),
            config: WifiDirectConfig::default(),
        }
    }

    /// Sets the device name.
    pub fn device_name(mut self, name: impl Into<String>) -> Self {
        self.config.device_name = name.into();
        self
    }

    /// Enables or disables auto-accept.
    pub fn auto_accept(mut self, enabled: bool) -> Self {
        self.config.auto_accept = enabled;
        self
    }

    /// Sets the group owner intent (0-15).
    pub fn group_owner_intent(mut self, intent: u8) -> Self {
        self.config.group_owner_intent = intent.min(15);
        self
    }

    /// Builds the transport.
    pub fn build(self) -> WifiDirectTransport {
        WifiDirectTransport::with_config(self.device_id, self.config)
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
    fn test_wifi_direct_transport_creation() {
        let transport = WifiDirectTransport::new("test-device");
        assert_eq!(transport.device_id(), "test-device");
        assert_eq!(transport.transport_type(), TransportType::WiFiDirect);
        assert_eq!(transport.status(), TransportStatus::Unavailable);
    }

    #[test]
    fn test_builder() {
        let transport = WifiDirectTransportBuilder::new("test-device")
            .device_name("MyDevice")
            .auto_accept(true)
            .group_owner_intent(10)
            .build();

        assert_eq!(transport.config().device_name, "MyDevice");
        assert!(transport.config().auto_accept);
        assert_eq!(transport.config().group_owner_intent, 10);
    }

    #[test]
    fn test_peer_discovery() {
        let transport = WifiDirectTransport::new("test-device");

        let peer = WifiDirectPeer {
            device_name: "Peer1".to_string(),
            device_address: "00:11:22:33:44:55".to_string(),
            is_group_owner: false,
            last_seen: SystemTime::now(),
            connected: true,
        };

        transport.on_peer_discovered(peer.clone());

        let peers = transport.get_peers();
        assert_eq!(peers.len(), 1);
        assert_eq!(peers[0].device_name, "Peer1");
    }

    #[test]
    fn test_send_receive() {
        let transport = WifiDirectTransport::new("test-device");

        // Mark as available
        transport.on_status_changed(TransportStatus::Available);

        // Send message
        let message = create_test_message();
        assert!(transport.send(&message).is_ok());

        // Should have message in queue
        assert!(transport.has_pending_sends());

        let next = transport.get_next_message().unwrap();
        assert!(next.is_some());
    }

    #[test]
    fn test_serialization() {
        let transport = WifiDirectTransport::new("test-device");
        let message = create_test_message();

        // Serialize
        let data = transport.serialize_message(&message).unwrap();
        assert!(!data.is_empty());

        // Deserialize
        let deserialized = transport.deserialize_message(&data).unwrap();
        assert_eq!(deserialized.id, message.id);
    }

    #[test]
    fn test_send_when_unavailable_fails() {
        let transport = WifiDirectTransport::new("test-device");
        let message = create_test_message();
        let result = transport.send(&message);
        assert!(result.is_err());
        assert!(matches!(
            result.unwrap_err(),
            crate::Error::TransportNotAvailable(_)
        ));
    }

    #[test]
    fn test_receive_when_empty_returns_none() {
        let transport = WifiDirectTransport::new("test-device");
        assert!(transport.receive().unwrap().is_none());
    }

    #[test]
    fn test_on_data_received_invalid_json_drops_ok() {
        let transport = WifiDirectTransport::new("test-device");
        let result = transport.on_data_received(b"not json".to_vec());
        assert!(result.is_ok());
        assert!(transport.receive().unwrap().is_none());
    }

    #[test]
    fn test_on_data_received_valid_queues_message() {
        let transport = WifiDirectTransport::new("test-device");
        let message = create_test_message();
        let data = transport.serialize_message(&message).unwrap();
        transport.on_data_received(data).unwrap();
        let received = transport.receive().unwrap();
        assert!(received.is_some());
        assert_eq!(received.unwrap().id, message.id);
    }

    #[test]
    fn test_on_peer_lost() {
        let transport = WifiDirectTransport::new("test-device");
        let peer = WifiDirectPeer {
            device_name: "P".to_string(),
            device_address: "00:11:22:33:44:55".to_string(),
            is_group_owner: false,
            last_seen: SystemTime::now(),
            connected: true,
        };
        transport.on_peer_discovered(peer);
        assert_eq!(transport.get_peers().len(), 1);
        transport.on_peer_lost("00:11:22:33:44:55");
        assert_eq!(transport.get_peers().len(), 0);
    }

    #[test]
    fn test_get_peer() {
        let transport = WifiDirectTransport::new("test-device");
        let peer = WifiDirectPeer {
            device_name: "P".to_string(),
            device_address: "aa:bb:cc:dd:ee:ff".to_string(),
            is_group_owner: true,
            last_seen: SystemTime::now(),
            connected: false,
        };
        transport.on_peer_discovered(peer.clone());
        let found = transport.get_peer("aa:bb:cc:dd:ee:ff");
        assert!(found.is_some());
        assert_eq!(found.unwrap().device_name, "P");
        assert!(transport.get_peer("other").is_none());
    }

    #[test]
    fn test_update_metrics() {
        let transport = WifiDirectTransport::new("test-device");
        let mut m = TransportMetrics::default();
        m.rssi = Some(-72);
        transport.update_metrics(m);
        assert_eq!(transport.metrics().rssi, Some(-72));
    }

    #[test]
    fn test_platform_handle() {
        let transport = WifiDirectTransport::new("test-device");
        assert_eq!(transport.platform_handle(), None);
        transport.set_platform_handle(99);
        assert_eq!(transport.platform_handle(), Some(99));
    }

    #[test]
    fn test_has_pending_sends_empty() {
        let transport = WifiDirectTransport::new("test-device");
        assert!(!transport.has_pending_sends());
    }

    #[test]
    fn test_stop_sets_disconnected() {
        let mut transport = WifiDirectTransport::new("test-device");
        transport.on_status_changed(TransportStatus::Available);
        transport.stop().unwrap();
        assert_eq!(transport.status(), TransportStatus::Disconnected);
    }

    #[test]
    fn test_set_on_messages_available_callback_invoked_on_send() {
        use std::sync::atomic::{AtomicBool, Ordering};
        let transport = WifiDirectTransport::new("test-device");
        transport.on_status_changed(TransportStatus::Available);
        let called = std::sync::Arc::new(AtomicBool::new(false));
        let c = called.clone();
        transport.set_on_messages_available(std::sync::Arc::new(move || {
            c.store(true, Ordering::SeqCst)
        }));
        let _ = transport.send(&create_test_message());
        assert!(called.load(Ordering::SeqCst));
    }

    #[test]
    fn test_get_next_message_returns_recipient_and_data() {
        let transport = WifiDirectTransport::new("test-device");
        transport.on_status_changed(TransportStatus::Available);
        let message = create_test_message();
        transport.send(&message).unwrap();
        let next = transport.get_next_message().unwrap();
        assert!(next.is_some());
        let (recipient, data) = next.unwrap();
        assert_eq!(recipient, message.recipient.as_str());
        assert!(!data.is_empty());
        let deserialized = transport.deserialize_message(&data).unwrap();
        assert_eq!(deserialized.id, message.id);
    }
}
