//! The peer-stream transport: the queue engine behind the `wifi_direct` slot.
//!
//! A peer stream is a byte stream the platform established to exactly one
//! other device: a Wi-Fi Direct group socket on Android, a Multipeer session
//! on iOS, a TCP connection over a LAN or a routed mesh on a host. To the
//! engine they are one transport, and this is it. The name is historical and
//! recorded as debt in `docs/spec/stream-framing.md`; renaming the slot is a
//! separate breaking change.
//!
//! No socket is touched here. The platform owns discovery, the connections,
//! the framing (`u32` big-endian length plus body) and the preamble exchange;
//! the Rust side holds the send and receive queues, the set of live links,
//! and metrics. The Rust reference for the framing and the preamble position
//! is [`crate::stream_framing`], for a host that owns its own sockets.
//!
//! # The link contract
//!
//! A link exists only under an address a preamble proved. The platform calls
//! [`WifiDirectTransport::on_peer_connected`] with the address
//! `verify_identity_assertion` derived from the peer's first frame, and
//! nothing else: not a socket address, not a device name, not a MAC, and not
//! an address it only read from a discovery record. Every inbound body is
//! then attributed to that address through
//! [`Transport::on_data_received_from`], which is what the receiving core
//! matches `Message.sender` against.
//!
//! The same rule bounds the outbound side. [`Transport::send`] refuses a
//! recipient no stream has proved with [`crate::Error::PeerNotReachable`],
//! which the transport manager treats as "not this carrier" and falls through
//! to the next one. Without that refusal a Wi-Fi Direct group that has formed
//! but exchanged no preamble would be `Available` with nothing to address,
//! and the selector, which weights bandwidth most heavily, would route direct
//! messages into a queue whose frames the far side must drop: a message lost
//! on the carrier that scored best, with BLE and the relay never tried.
//!
//! The bridge contract, in full: the platform reports connectivity via
//! [`Transport::on_status_changed`], proved links via
//! [`WifiDirectTransport::on_peer_connected`] and
//! [`WifiDirectTransport::on_peer_disconnected`], drains outbound bodies
//! with [`Transport::get_next_message`] (woken by the
//! [`Transport::set_on_messages_available`] callback), and injects inbound
//! bodies via [`Transport::on_data_received_from`]. The discovery view
//! ([`WifiDirectTransport::on_peer_discovered`], keyed by hardware address)
//! is informational and never routed by.

use crate::{Result, SharedCallback, Transport, TransportMetrics, TransportStatus, TransportType};
use offline_protocol_core::{Message, MutexExt};
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

/// The peer-stream transport: queue engine for platform-established streams
/// to one peer each, registered in the `wifi_direct` slot.
///
/// See the module documentation for the link contract. Higher throughput
/// than BLE, whole messages in one write, and every link is one proved
/// address.
pub struct WifiDirectTransport {
    /// Local device ID
    device_id: String,
    /// Configuration
    config: WifiDirectConfig,
    /// Transport status
    status: Arc<Mutex<TransportStatus>>,
    /// Discovered peers, keyed by device address (MAC).
    ///
    /// This is the *discovery* view reported by `WifiP2pManager`, so its key
    /// is a hardware address. It deliberately stays separate from
    /// [`Self::connected_links`], which is keyed by user-level id — the two
    /// answer different questions ("what did we see?" vs "who can we address
    /// right now?") and only the latter is safe to route by.
    peers: Arc<Mutex<HashMap<String, WifiDirectPeer>>>,
    /// Live links, keyed by the peer's user-level id.
    ///
    /// Populated from the platform's connect/disconnect callbacks, which are
    /// the only signals that carry a user id. Drained whenever the transport
    /// leaves `Available`, so it can never hand out a link that has gone away.
    connected_links: Arc<Mutex<HashMap<String, SystemTime>>>,
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
            connected_links: Arc::new(Mutex::new(HashMap::new())),
            receive_queue: Arc::new(Mutex::new(VecDeque::new())),
            send_queue: Arc::new(Mutex::new(VecDeque::new())),
            metrics: Arc::new(Mutex::new(TransportMetrics::default())),
            platform_handle: Arc::new(Mutex::new(None)),
            on_messages_available: Arc::new(Mutex::new(None)),
        }
    }

    /// Notifies the platform that messages are ready to send.
    fn notify_messages_available(&self) {
        let callback = self.on_messages_available.lock_or_recover().clone();
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
        crate::common::set_platform_handle(&self.platform_handle, handle);
    }

    /// Gets the platform-specific handle.
    pub fn platform_handle(&self) -> Option<usize> {
        crate::common::platform_handle(&self.platform_handle)
    }

    /// Called when a peer device is discovered.
    pub fn on_peer_discovered(&self, peer: WifiDirectPeer) {
        let mut peers = self.peers.lock_or_recover();
        peers.insert(peer.device_address.clone(), peer);
    }

    /// Called when a peer device is lost.
    pub fn on_peer_lost(&self, device_address: &str) {
        let mut peers = self.peers.lock_or_recover();
        peers.remove(device_address);
    }

    /// Records a live link to `peer_id` (platform connect callback).
    ///
    /// Takes the address the peer's preamble proved, which is what the send
    /// path and [`Transport::connected_peers`] are keyed by, unlike
    /// [`Self::on_peer_discovered`], whose key is a hardware address. Links
    /// are keyed by address, so a second stream proving an address already
    /// held is not a second link; the chapter's one-announce-one-loss rule
    /// for that case is the platform's to keep.
    pub fn on_peer_connected(&self, peer_id: impl Into<String>) {
        let peer_id = peer_id.into();
        if peer_id.is_empty() {
            return;
        }
        self.connected_links
            .lock_or_recover()
            .insert(peer_id, SystemTime::now());
    }

    /// Drops the live link to `peer_id` (platform disconnect callback).
    pub fn on_peer_disconnected(&self, peer_id: &str) {
        self.connected_links.lock_or_recover().remove(peer_id);
    }

    /// Called when a message is received from a peer.
    pub fn on_message_received(&self, message: Message) {
        crate::common::on_message_received(&self.receive_queue, message);
    }

    /// Like [`on_message_received`](Self::on_message_received), but attaches a
    /// transport-verified `peer_id` to the message.
    ///
    /// # Parameters
    ///
    /// * `peer_id` — The **user-level identifier** (i.e., the remote peer's
    ///   `UserId` string) that the transport layer has authenticated for this
    ///   connection. This is **not** the raw transport address (MAC, IP, etc.).
    ///   The protocol layer uses this value to verify that `message.sender`
    ///   matches the physical peer that delivered it.
    pub fn on_message_received_from(&self, message: Message, peer_id: String) {
        crate::common::on_message_received_from(&self.receive_queue, message, peer_id);
    }

    /// Gets all discovered peers.
    pub fn get_peers(&self) -> Vec<WifiDirectPeer> {
        let peers = self.peers.lock_or_recover();
        peers.values().cloned().collect()
    }

    /// Gets a specific peer by device address.
    pub fn get_peer(&self, device_address: &str) -> Option<WifiDirectPeer> {
        let peers = self.peers.lock_or_recover();
        peers.get(device_address).cloned()
    }

    /// Updates transport metrics.
    pub fn update_metrics(&self, metrics: TransportMetrics) {
        *self.metrics.lock_or_recover() = metrics;
    }

    /// Serializes a message to JSON bytes.
    pub fn serialize_message(&self, message: &Message) -> Result<Vec<u8>> {
        crate::common::serialize_message_with(message)
    }

    /// Checks if there are messages to send.
    pub fn has_pending_sends(&self) -> bool {
        let queue = self.send_queue.lock_or_recover();
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
        *self.status.lock_or_recover()
    }

    fn metrics(&self) -> TransportMetrics {
        self.metrics.lock_or_recover().clone()
    }

    /// Queues `message` for its recipient, if a stream has proved that
    /// recipient.
    ///
    /// Refuses with [`crate::Error::PeerNotReachable`] otherwise, exactly as
    /// BLE does for a recipient that is not a connected peer: a stream
    /// carries one peer, and the platform drains this queue by recipient,
    /// so a body for an address no stream proved has nowhere to go. The
    /// transport manager treats the refusal as "try the next carrier", not
    /// as a failure of this one (see the module documentation for what
    /// happens without it).
    fn send(&self, message: &Message) -> Result<()> {
        // Check status
        if self.status() != TransportStatus::Available {
            return Err(crate::Error::TransportNotAvailable(
                "Wi-Fi Direct transport is not available".to_string(),
            ));
        }

        // Determine recipient and add to send queue
        let recipient = message.recipient.as_str().to_string();
        if !self
            .connected_links
            .lock_or_recover()
            .contains_key(&recipient)
        {
            return Err(crate::Error::PeerNotReachable(format!(
                "peer stream: no stream has proved {}",
                recipient
            )));
        }
        {
            let mut queue = self.send_queue.lock_or_recover();
            queue.push_back((recipient, message.clone()));

            // Update metrics
            let mut metrics = self.metrics.lock_or_recover();
            metrics.queue_depth = queue.len();
            metrics.congestion = ((metrics.queue_depth as f32) / 20.0).clamp(0.0, 1.0);
        }

        // Notify platform that messages are available to send.
        self.notify_messages_available();

        Ok(())
    }

    /// Queues `message` for a specific connected peer.
    ///
    /// Unlike [`Transport::send`], which accepts any recipient and lets the
    /// platform discover it cannot be reached, this refuses a peer with no
    /// live link — a forwarding caller needs the failure synchronously so it
    /// can pick another neighbor instead.
    fn send_to_peer(&self, peer_id: &str, message: &Message) -> Result<()> {
        if self.status() != TransportStatus::Available {
            return Err(crate::Error::TransportNotAvailable(
                "Wi-Fi Direct transport is not available".to_string(),
            ));
        }

        if !self.connected_links.lock_or_recover().contains_key(peer_id) {
            return Err(crate::Error::PeerNotReachable(format!(
                "Wi-Fi Direct: {} is not a connected peer",
                peer_id
            )));
        }

        {
            let mut queue = self.send_queue.lock_or_recover();
            queue.push_back((peer_id.to_string(), message.clone()));

            let mut metrics = self.metrics.lock_or_recover();
            metrics.queue_depth = queue.len();
            metrics.congestion = ((metrics.queue_depth as f32) / 20.0).clamp(0.0, 1.0);
        }

        self.notify_messages_available();

        Ok(())
    }

    fn connected_peers(&self) -> Vec<crate::PeerLink> {
        if self.status() != TransportStatus::Available {
            return Vec::new();
        }
        self.connected_links
            .lock_or_recover()
            .keys()
            .map(crate::PeerLink::new)
            .collect()
    }

    fn receive(&self) -> Result<Option<Message>> {
        let mut queue = self.receive_queue.lock_or_recover();
        Ok(queue.pop_front())
    }

    fn start(&self) -> Result<()> {
        // Status will be updated by platform implementation via on_status_changed()
        Ok(())
    }

    fn stop(&self) -> Result<()> {
        *self.status.lock_or_recover() = TransportStatus::Disconnected;
        self.peers.lock_or_recover().clear();
        self.connected_links.lock_or_recover().clear();
        self.send_queue.lock_or_recover().clear();
        self.receive_queue.lock_or_recover().clear();
        let mut metrics = self.metrics.lock_or_recover();
        metrics.queue_depth = 0;
        metrics.congestion = 0.0;
        Ok(())
    }

    /// Called when connection status changes.
    ///
    /// When transitioning away from [`TransportStatus::Available`], peers
    /// and queues are drained so a subsequent reconnect starts clean.
    fn on_status_changed(&self, status: TransportStatus) {
        let previous = {
            let mut guard = self.status.lock_or_recover();
            let prev = *guard;
            *guard = status;
            prev
        };

        if previous == TransportStatus::Available && status != TransportStatus::Available {
            self.peers.lock_or_recover().clear();
            self.connected_links.lock_or_recover().clear();
            self.send_queue.lock_or_recover().clear();
            self.receive_queue.lock_or_recover().clear();
            let mut metrics = self.metrics.lock_or_recover();
            metrics.queue_depth = 0;
            metrics.congestion = 0.0;
        }
    }

    fn on_data_received(&self, data: Vec<u8>) -> Result<()> {
        crate::common::on_data_received(&self.receive_queue, data)
    }

    /// Like [`Transport::on_data_received`], but attaches a
    /// transport-verified `peer_id` to the deserialized message.
    ///
    /// # Parameters
    ///
    /// * `peer_id` — The **user-level identifier** (i.e., the remote peer's
    ///   `UserId` string) that the transport layer has authenticated for this
    ///   connection. This is **not** the raw transport address (MAC, IP, etc.).
    ///   The protocol layer uses this value to verify that `message.sender`
    ///   matches the physical peer that delivered it.
    fn on_data_received_from(&self, data: Vec<u8>, peer_id: String) -> Result<()> {
        crate::common::on_data_received_from(&self.receive_queue, data, peer_id)
    }

    /// Gets the next message to send (for platform implementation).
    ///
    /// Returns (recipient, serialized_data) or None if no messages to send.
    fn get_next_message(&self) -> Result<Option<(String, Vec<u8>)>> {
        let (recipient, message) = {
            let mut queue = self.send_queue.lock_or_recover();
            match queue.pop_front() {
                Some((r, m)) => (r, m),
                None => return Ok(None),
            }
        };

        // Serialize the message
        let data = self.serialize_message(&message)?;
        Ok(Some((recipient, data)))
    }

    /// Registers a callback that fires when new outgoing messages become available.
    fn set_on_messages_available(&self, callback: Arc<dyn Fn() + Send + Sync>) {
        *self.on_messages_available.lock_or_recover() = Some(callback);
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

        // Mark as available, with a proved link to the recipient
        transport.on_status_changed(TransportStatus::Available);
        transport.on_peer_connected("bob");

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

    /// A recipient no stream has proved is refused as "not reachable here",
    /// the refusal the transport manager falls through on. Without it an
    /// `Available` transport with no links would take every direct message
    /// the selector's bandwidth weight sends its way and hand them to a
    /// platform that has no stream to write them to.
    #[test]
    fn send_to_a_recipient_no_stream_proved_is_peer_not_reachable() {
        let transport = WifiDirectTransport::new("test-device");
        transport.on_status_changed(TransportStatus::Available);

        let result = transport.send(&create_test_message());
        assert!(
            matches!(result, Err(crate::Error::PeerNotReachable(_))),
            "got {result:?}"
        );
        assert!(!transport.has_pending_sends(), "nothing may be queued");

        // A link to someone else does not make this recipient reachable.
        transport.on_peer_connected("carol");
        assert!(matches!(
            transport.send(&create_test_message()),
            Err(crate::Error::PeerNotReachable(_))
        ));

        // The proved link is what makes the difference.
        transport.on_peer_connected("bob");
        assert!(transport.send(&create_test_message()).is_ok());
        assert!(transport.has_pending_sends());

        // And its loss takes the reachability with it.
        transport.on_peer_disconnected("bob");
        assert!(matches!(
            transport.send(&create_test_message()),
            Err(crate::Error::PeerNotReachable(_))
        ));
    }

    /// Leaving `Available` clears the links, so a reconnect starts with
    /// nothing proved and nothing addressable.
    #[test]
    fn a_status_drop_clears_the_proved_links() {
        let transport = WifiDirectTransport::new("test-device");
        transport.on_status_changed(TransportStatus::Available);
        transport.on_peer_connected("bob");
        assert_eq!(transport.connected_peers().len(), 1);

        transport.on_status_changed(TransportStatus::Disconnected);
        transport.on_status_changed(TransportStatus::Available);
        assert!(transport.connected_peers().is_empty());
        assert!(matches!(
            transport.send(&create_test_message()),
            Err(crate::Error::PeerNotReachable(_))
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
        let transport = WifiDirectTransport::new("test-device");
        transport.on_status_changed(TransportStatus::Available);
        transport.stop().unwrap();
        assert_eq!(transport.status(), TransportStatus::Disconnected);
    }

    #[test]
    fn test_stop_clears_session_state() {
        let transport = WifiDirectTransport::new("test-device");
        transport.on_status_changed(TransportStatus::Available);
        transport.on_peer_connected("bob");

        let peer = WifiDirectPeer {
            device_name: "P".to_string(),
            device_address: "00:11:22:33:44:55".to_string(),
            is_group_owner: false,
            last_seen: SystemTime::now(),
            connected: true,
        };
        transport.on_peer_discovered(peer);
        transport.send(&create_test_message()).unwrap();

        transport.stop().unwrap();

        assert!(transport.peers.lock().unwrap().is_empty());
        assert!(transport.send_queue.lock().unwrap().is_empty());
        assert!(transport.receive_queue.lock().unwrap().is_empty());
        assert_eq!(transport.metrics.lock().unwrap().queue_depth, 0);
        assert_eq!(transport.metrics.lock().unwrap().congestion, 0.0);
    }

    #[test]
    fn test_wifi_direct_on_status_changed_clears_state() {
        let transport = WifiDirectTransport::new("test-device");
        transport.on_status_changed(TransportStatus::Available);
        transport.on_peer_connected("bob");

        let peer = WifiDirectPeer {
            device_name: "P".to_string(),
            device_address: "00:11:22:33:44:55".to_string(),
            is_group_owner: false,
            last_seen: SystemTime::now(),
            connected: true,
        };
        transport.on_peer_discovered(peer);
        transport.send(&create_test_message()).unwrap();

        transport.on_status_changed(TransportStatus::Disconnected);

        assert!(transport.peers.lock().unwrap().is_empty());
        assert!(transport.send_queue.lock().unwrap().is_empty());
        assert!(transport.receive_queue.lock().unwrap().is_empty());
        assert_eq!(transport.metrics.lock().unwrap().queue_depth, 0);
    }

    #[test]
    fn test_wifi_direct_on_status_changed_available_preserves_state() {
        let transport = WifiDirectTransport::new("test-device");
        transport.on_status_changed(TransportStatus::Available);
        transport.on_peer_connected("bob");

        let peer = WifiDirectPeer {
            device_name: "P".to_string(),
            device_address: "00:11:22:33:44:55".to_string(),
            is_group_owner: false,
            last_seen: SystemTime::now(),
            connected: true,
        };
        transport.on_peer_discovered(peer);
        transport.send(&create_test_message()).unwrap();

        // Available → Available must not clear anything.
        transport.on_status_changed(TransportStatus::Available);

        assert_eq!(transport.peers.lock().unwrap().len(), 1);
        assert!(!transport.send_queue.lock().unwrap().is_empty());
    }

    #[test]
    fn test_set_on_messages_available_callback_invoked_on_send() {
        use std::sync::atomic::{AtomicBool, Ordering};
        let transport = WifiDirectTransport::new("test-device");
        transport.on_status_changed(TransportStatus::Available);
        transport.on_peer_connected("bob");
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
        transport.on_peer_connected("bob");
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

    #[test]
    fn test_on_data_received_rejects_oversized_payload() {
        let transport = WifiDirectTransport::new("test-device");
        let oversized = vec![0u8; DEFAULT_MAX_MESSAGE_SIZE + 1];
        let result = transport.on_data_received(oversized);
        assert!(result.is_err());
        assert!(matches!(
            result.unwrap_err(),
            crate::Error::MessageTooLarge(_, _)
        ));
    }

    #[test]
    fn test_on_data_received_from_rejects_oversized_payload() {
        let transport = WifiDirectTransport::new("test-device");
        let oversized = vec![0u8; DEFAULT_MAX_MESSAGE_SIZE + 1];
        let result = transport.on_data_received_from(oversized, "peer1".to_string());
        assert!(result.is_err());
        assert!(matches!(
            result.unwrap_err(),
            crate::Error::MessageTooLarge(_, _)
        ));
    }
}
