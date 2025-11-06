//! Bluetooth Low Energy (BLE) transport implementation.
//!
//! This module provides the BLE transport layer for peer-to-peer communication.
//! It handles:
//! - Device discovery (advertising and scanning)
//! - GATT server/client operations
//! - Message transmission over BLE characteristics
//! - Message fragmentation for large payloads

use crate::{Result, Transport, TransportMetrics, TransportStatus, TransportType};
use offline_protocol_core::Message;
use std::sync::{Arc, Mutex};
use std::collections::{HashMap, VecDeque};
use std::time::SystemTime;

/// UUID for the Offline Protocol GATT service
pub const SERVICE_UUID: &str = "6E400001-B5A3-F393-E0A9-E50E24DCCA9E";

/// UUID for the message characteristic (write/notify)
pub const MESSAGE_CHAR_UUID: &str = "6E400002-B5A3-F393-E0A9-E50E24DCCA9E";

/// UUID for the device ID characteristic (read)
pub const DEVICE_ID_CHAR_UUID: &str = "6E400003-B5A3-F393-E0A9-E50E24DCCA9E";

/// Maximum BLE payload size (MTU - overhead)
/// Typical BLE MTU ranges from 23-251 bytes, we use conservative 185 bytes per fragment
pub const MAX_FRAGMENT_SIZE: usize = 185;

/// Fragment timeout - if fragments aren't all received within 30s, discard
pub const FRAGMENT_TIMEOUT_SECS: u64 = 30;

/// Peer device information
#[derive(Debug, Clone)]
pub struct PeerDevice {
    /// Device ID (user_id)
    pub device_id: String,
    /// BLE address (platform-specific)
    pub address: String,
    /// Signal strength in dBm
    pub rssi: i16,
    /// Last seen timestamp
    pub last_seen: std::time::SystemTime,
    /// Connection status
    pub connected: bool,
}

/// Message fragment for BLE transmission
#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
struct MessageFragment {
    /// Message ID being fragmented
    message_id: String,
    /// Fragment index (0-based)
    fragment_index: u16,
    /// Total number of fragments
    total_fragments: u16,
    /// Fragment payload data
    data: Vec<u8>,
}

/// Reassembly buffer for incoming fragments
#[derive(Debug)]
struct FragmentAssembly {
    /// Total expected fragments
    total_fragments: u16,
    /// Received fragments (index -> data)
    fragments: HashMap<u16, Vec<u8>>,
    /// First fragment received time
    started_at: SystemTime,
}

/// BLE transport implementation.
///
/// This is a platform-agnostic abstraction. The actual BLE operations
/// are delegated to platform-specific implementations via callbacks.
pub struct BleTransport {
    /// Local device ID
    device_id: String,
    /// Transport status
    status: Arc<Mutex<TransportStatus>>,
    /// Discovered peers
    peers: Arc<Mutex<HashMap<String, PeerDevice>>>,
    /// Received message queue
    receive_queue: Arc<Mutex<VecDeque<Message>>>,
    /// Send queue
    send_queue: Arc<Mutex<VecDeque<(String, Message)>>>,
    /// Pending serialized fragments waiting to be delivered
    pending_fragments: Arc<Mutex<VecDeque<(String, Vec<u8>)>>>,
    /// Transport metrics
    metrics: Arc<Mutex<TransportMetrics>>,
    /// Platform-specific handle (opaque pointer)
    platform_handle: Arc<Mutex<Option<usize>>>,
    /// Fragment reassembly buffers
    fragment_buffers: Arc<Mutex<HashMap<String, FragmentAssembly>>>,
    /// Negotiated MTU size (set by platform)
    mtu_size: Arc<Mutex<usize>>,
}

impl BleTransport {
    /// Creates a new BLE transport.
    pub fn new(device_id: impl Into<String>) -> Self {
        Self {
            device_id: device_id.into(),
            status: Arc::new(Mutex::new(TransportStatus::Unavailable)),
            peers: Arc::new(Mutex::new(HashMap::new())),
            receive_queue: Arc::new(Mutex::new(VecDeque::new())),
            send_queue: Arc::new(Mutex::new(VecDeque::new())),
            pending_fragments: Arc::new(Mutex::new(VecDeque::new())),
            metrics: Arc::new(Mutex::new(TransportMetrics::default())),
            platform_handle: Arc::new(Mutex::new(None)),
            fragment_buffers: Arc::new(Mutex::new(HashMap::new())),
            mtu_size: Arc::new(Mutex::new(MAX_FRAGMENT_SIZE)),
        }
    }

    /// Sets the negotiated MTU size (called by platform after BLE MTU negotiation).
    pub fn set_mtu(&self, mtu: usize) {
        *self.mtu_size.lock().unwrap() = mtu.saturating_sub(3); // Reserve 3 bytes for ATT overhead
    }

    /// Gets the current MTU size.
    pub fn mtu(&self) -> usize {
        *self.mtu_size.lock().unwrap()
    }

    /// Sets the platform-specific handle.
    ///
    /// This is called by the platform implementation to store its context.
    pub fn set_platform_handle(&self, handle: usize) {
        *self.platform_handle.lock().unwrap() = Some(handle);
    }

    /// Gets the platform-specific handle.
    pub fn platform_handle(&self) -> Option<usize> {
        *self.platform_handle.lock().unwrap()
    }

    /// Gets the local device ID.
    pub fn device_id(&self) -> &str {
        &self.device_id
    }

    /// Called when a peer device is discovered.
    pub fn on_peer_discovered(&self, peer: PeerDevice) {
        let mut peers = self.peers.lock().unwrap();
        peers.insert(peer.device_id.clone(), peer);
    }

    /// Called when a peer device is lost.
    pub fn on_peer_lost(&self, device_id: &str) {
        let mut peers = self.peers.lock().unwrap();
        peers.remove(device_id);
    }

    /// Called when a message is received from a peer.
    pub fn on_message_received(&self, message: Message) {
        let mut queue = self.receive_queue.lock().unwrap();
        queue.push_back(message);
    }

    /// Called when connection status changes.
    pub fn on_status_changed(&self, status: TransportStatus) {
        *self.status.lock().unwrap() = status;
    }

    /// Gets all discovered peers.
    pub fn get_peers(&self) -> Vec<PeerDevice> {
        let peers = self.peers.lock().unwrap();
        peers.values().cloned().collect()
    }

    /// Gets a specific peer by device ID.
    pub fn get_peer(&self, device_id: &str) -> Option<PeerDevice> {
        let peers = self.peers.lock().unwrap();
        peers.get(device_id).cloned()
    }

    /// Updates transport metrics.
    pub fn update_metrics(&self, metrics: TransportMetrics) {
        *self.metrics.lock().unwrap() = metrics;
    }

    fn update_queue_metric(&self) {
        let send_len = self.send_queue.lock().unwrap().len();
        let fragment_len = self.pending_fragments.lock().unwrap().len();
        let mut metrics = self.metrics.lock().unwrap();
        metrics.queue_depth = send_len + fragment_len;
    }

    /// Records a successful send for metrics tracking.
    pub fn record_send_success(&self) {
        let mut metrics = self.metrics.lock().unwrap();
        metrics.success_count = metrics.success_count.saturating_add(1);
    }

    /// Records a failed send for metrics tracking.
    pub fn record_send_failure(&self) {
        let mut metrics = self.metrics.lock().unwrap();
        metrics.failure_count = metrics.failure_count.saturating_add(1);
    }

    /// Gets current queue depth for metrics.
    pub fn get_queue_depth(&self) -> usize {
        let send_len = self.send_queue.lock().unwrap().len();
        let fragment_len = self.pending_fragments.lock().unwrap().len();
        send_len + fragment_len
    }

    /// Processes the send queue (to be called by platform implementation).
    pub fn dequeue_send(&self) -> Option<(String, Message)> {
        let result = {
            let mut queue = self.send_queue.lock().unwrap();
            queue.pop_front()
        };

        if result.is_some() {
            self.update_queue_metric();
        }

        result
    }

    /// Checks if there are messages to send.
    pub fn has_pending_sends(&self) -> bool {
        let queue = self.send_queue.lock().unwrap();
        !queue.is_empty()
    }

    /// Serializes a message to bytes (JSON).
    pub fn serialize_message(&self, message: &Message) -> Result<Vec<u8>> {
        serde_json::to_vec(message)
            .map_err(|e| crate::Error::SerializationError(format!("Failed to serialize message: {}", e)))
    }

    /// Deserializes a message from bytes (JSON).
    pub fn deserialize_message(&self, data: &[u8]) -> Result<Message> {
        serde_json::from_slice(data)
            .map_err(|e| crate::Error::SerializationError(format!("Failed to deserialize message: {}", e)))
    }

    /// Fragments a message into chunks suitable for BLE transmission.
    ///
    /// Returns a vector of serialized fragments ready to send over BLE.
    pub fn fragment_message(&self, message: &Message) -> Result<Vec<Vec<u8>>> {
        // Serialize the message
        let message_bytes = self.serialize_message(message)?;
        
        // Check if fragmentation is needed
        let mtu = self.mtu();
        if message_bytes.len() <= mtu {
            // No fragmentation needed, send as single fragment
            let fragment = MessageFragment {
                message_id: message.id.as_str(),
                fragment_index: 0,
                total_fragments: 1,
                data: message_bytes,
            };
            let fragment_bytes = serde_json::to_vec(&fragment)
                .map_err(|e| crate::Error::SerializationError(format!("Failed to serialize fragment: {}", e)))?;
            return Ok(vec![fragment_bytes]);
        }

        // Fragment the message
        let total_fragments = (message_bytes.len() + mtu - 1) / mtu;
        if total_fragments > u16::MAX as usize {
            return Err(crate::Error::Other("Message too large to fragment".to_string()));
        }

        let mut fragments = Vec::new();
        for (i, chunk) in message_bytes.chunks(mtu).enumerate() {
            let fragment = MessageFragment {
                message_id: message.id.as_str(),
                fragment_index: i as u16,
                total_fragments: total_fragments as u16,
                data: chunk.to_vec(),
            };
            let fragment_bytes = serde_json::to_vec(&fragment)
                .map_err(|e| crate::Error::SerializationError(format!("Failed to serialize fragment {}: {}", i, e)))?;
            fragments.push(fragment_bytes);
        }

        Ok(fragments)
    }

    /// Processes an incoming fragment and reassembles if complete.
    ///
    /// Returns Ok(Some(Message)) if message is complete, Ok(None) if more fragments needed.
    pub fn process_fragment(&self, fragment_data: &[u8]) -> Result<Option<Message>> {
        // Deserialize fragment
        let fragment: MessageFragment = serde_json::from_slice(fragment_data)
            .map_err(|e| crate::Error::SerializationError(format!("Failed to deserialize fragment: {}", e)))?;

        // If it's a single fragment message, deserialize directly
        if fragment.total_fragments == 1 {
            return Ok(Some(self.deserialize_message(&fragment.data)?));
        }

        // Multi-fragment message - add to reassembly buffer
        let mut buffers = self.fragment_buffers.lock().unwrap();
        
        // Cleanup expired buffers first
        let now = SystemTime::now();
        buffers.retain(|_, assembly| {
            now.duration_since(assembly.started_at)
                .map(|d| d.as_secs() < FRAGMENT_TIMEOUT_SECS)
                .unwrap_or(false)
        });

        // Get or create assembly buffer
        let assembly = buffers.entry(fragment.message_id.clone()).or_insert_with(|| {
            FragmentAssembly {
                total_fragments: fragment.total_fragments,
                fragments: HashMap::new(),
                started_at: now,
            }
        });

        // Validate fragment
        if assembly.total_fragments != fragment.total_fragments {
            return Err(crate::Error::Other(format!(
                "Fragment count mismatch: expected {}, got {}",
                assembly.total_fragments, fragment.total_fragments
            )));
        }

        // Add fragment
        assembly.fragments.insert(fragment.fragment_index, fragment.data);

        // Check if complete
        if assembly.fragments.len() == assembly.total_fragments as usize {
            // Reassemble message
            let mut complete_data = Vec::new();
            for i in 0..assembly.total_fragments {
                if let Some(data) = assembly.fragments.get(&i) {
                    complete_data.extend_from_slice(data);
                } else {
                    return Err(crate::Error::Other(format!("Missing fragment {}", i)));
                }
            }

            // Remove assembly buffer
            buffers.remove(&fragment.message_id);
            
            // Deserialize complete message
            return Ok(Some(self.deserialize_message(&complete_data)?));
        }

        // More fragments needed
        Ok(None)
    }

    /// Called when raw fragment data is received from BLE (platform callback).
    ///
    /// This handles fragmentation reassembly and queues complete messages.
    pub fn on_fragment_received(&self, fragment_data: Vec<u8>) -> Result<()> {
        match self.process_fragment(&fragment_data) {
            Ok(Some(message)) => {
                // Message complete - queue it
                let mut queue = self.receive_queue.lock().unwrap();
                queue.push_back(message);
                Ok(())
            }
            Ok(None) => {
                // More fragments needed
                Ok(())
            }
            Err(e) => {
                // Log error but don't fail - just drop bad fragment
                eprintln!("Error processing fragment: {}", e);
                Ok(())
            }
        }
    }

    /// Gets the next fragment to send (for platform implementation).
    ///
    /// Returns (recipient, fragment_data) or None if no messages to send.
    pub fn get_next_fragment(&self) -> Result<Option<(String, Vec<u8>)>> {
        if let Some(fragment) = {
            let mut pending = self.pending_fragments.lock().unwrap();
            pending.pop_front()
        } {
            self.update_queue_metric();
            return Ok(Some(fragment));
        }

        // No serialized fragments waiting – pull a fresh message from the queue
        let maybe_message = {
            let mut queue = self.send_queue.lock().unwrap();
            queue.pop_front()
        };

        let Some((recipient, message)) = maybe_message else {
            self.update_queue_metric();
            return Ok(None);
        };

        let fragments = self.fragment_message(&message)?;

        if fragments.is_empty() {
            self.update_queue_metric();
            return Ok(None);
        }

        {
            let mut pending = self.pending_fragments.lock().unwrap();
            for fragment in fragments {
                pending.push_back((recipient.clone(), fragment));
            }
        }

        self.update_queue_metric();

        let result = {
            let mut pending = self.pending_fragments.lock().unwrap();
            pending.pop_front()
        };

        self.update_queue_metric();

        Ok(result)
    }

    /// Re-queues a fragment at the front of the pending queue (used when platform send fails).
    pub fn requeue_fragment(&self, recipient: &str, fragment_data: Vec<u8>) {
        {
            let mut pending = self.pending_fragments.lock().unwrap();
            pending.push_front((recipient.to_string(), fragment_data));
        }

        self.update_queue_metric();
    }
}

impl Transport for BleTransport {
    fn transport_type(&self) -> TransportType {
        TransportType::BLE
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
                "BLE transport is not available".to_string()
            ));
        }

        // Determine recipient and add to send queue
        let recipient = message.recipient.as_str().to_string();
        {
            let mut queue = self.send_queue.lock().unwrap();
            queue.push_back((recipient, message.clone()));
        }

        self.update_queue_metric();
        
        Ok(())
    }

    fn receive(&self) -> Result<Option<Message>> {
        let mut queue = self.receive_queue.lock().unwrap();
        Ok(queue.pop_front())
    }

    fn start(&mut self) -> Result<()> {
        // Status will be updated by platform implementation
        // via on_status_changed()
        Ok(())
    }

    fn stop(&mut self) -> Result<()> {
        *self.status.lock().unwrap() = TransportStatus::Disconnected;
        Ok(())
    }
}

/// BLE transport builder for configuration.
pub struct BleTransportBuilder {
    device_id: String,
}

impl BleTransportBuilder {
    /// Creates a new builder.
    pub fn new(device_id: impl Into<String>) -> Self {
        Self {
            device_id: device_id.into(),
        }
    }

    /// Builds the BLE transport.
    pub fn build(self) -> BleTransport {
        BleTransport::new(self.device_id)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_ble_transport_creation() {
        let transport = BleTransport::new("test-device");
        assert_eq!(transport.device_id(), "test-device");
        assert_eq!(transport.status(), TransportStatus::Unavailable);
    }

    #[test]
    fn test_peer_discovery() {
        let transport = BleTransport::new("test-device");
        
        let peer = PeerDevice {
            device_id: "peer-1".to_string(),
            address: "AA:BB:CC:DD:EE:FF".to_string(),
            rssi: -60,
            last_seen: std::time::SystemTime::now(),
            connected: false,
        };

        transport.on_peer_discovered(peer.clone());
        
        let peers = transport.get_peers();
        assert_eq!(peers.len(), 1);
        assert_eq!(peers[0].device_id, "peer-1");
    }

    #[test]
    fn test_peer_lost() {
        let transport = BleTransport::new("test-device");
        
        let peer = PeerDevice {
            device_id: "peer-1".to_string(),
            address: "AA:BB:CC:DD:EE:FF".to_string(),
            rssi: -60,
            last_seen: std::time::SystemTime::now(),
            connected: false,
        };

        transport.on_peer_discovered(peer);
        transport.on_peer_lost("peer-1");
        
        let peers = transport.get_peers();
        assert_eq!(peers.len(), 0);
    }
}

