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
use std::collections::{HashMap, VecDeque};
use std::convert::TryInto;
use std::sync::{Arc, Mutex};
use std::time::{Duration as StdDuration, SystemTime};

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

/// Maximum number of concurrent fragment assemblies we buffer before evicting the oldest.
pub const MAX_FRAGMENT_ASSEMBLIES: usize = 64;

/// Maximum allowed fragments for a single BLE message to avoid memory blow-up.
pub const MAX_FRAGMENT_COUNT: usize = 512;

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

const FRAGMENT_MAGIC: [u8; 2] = *b"OP"; // Offline Protocol
const FRAGMENT_VERSION: u8 = 1;
const FRAGMENT_HEADER_FIXED: usize = 2 /*magic*/ + 1 /*version*/ + 1 /*id_len*/ + 2 /*index*/ + 2 /*total*/ + 2 /*data_len*/;

#[derive(Debug, Clone)]
struct DecodedFragment {
    message_id: String,
    fragment_index: u16,
    total_fragments: u16,
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
        let heuristic_capacity = 50_f32;
        metrics.congestion = ((metrics.queue_depth as f32) / heuristic_capacity).clamp(0.0, 1.0);
    }

    /// Records a successful send for metrics tracking.
    pub fn record_send_success(&self) {
        let mut metrics = self.metrics.lock().unwrap();
        metrics.success_count = metrics.success_count.saturating_add(1);
        let total = metrics.success_count + metrics.failure_count;
        if total > 0 {
            let ratio = metrics.success_count as f32 / total as f32;
            metrics.delivery_ratio = Some(ratio);
            metrics.drop_rate = Some((1.0 - ratio).clamp(0.0, 1.0));
        }
    }

    /// Records a failed send for metrics tracking.
    pub fn record_send_failure(&self) {
        let mut metrics = self.metrics.lock().unwrap();
        metrics.failure_count = metrics.failure_count.saturating_add(1);
        let total = metrics.success_count + metrics.failure_count;
        if total > 0 {
            let drop_ratio = metrics.failure_count as f32 / total as f32;
            metrics.drop_rate = Some(drop_ratio.clamp(0.0, 1.0));
            metrics.delivery_ratio = Some((1.0 - drop_ratio).clamp(0.0, 1.0));
        }
    }

    fn record_latency(&self, latency_ms: u128) {
        let value = latency_ms.min(u128::from(u32::MAX)) as u32;
        let mut metrics = self.metrics.lock().unwrap();
        metrics.latency_ms = Some(match metrics.latency_ms {
            Some(existing) => {
                let ema = (existing as f32 * 0.7) + (value as f32 * 0.3);
                ema as u32
            }
            None => value,
        });
    }

    fn cleanup_fragment_buffers(&self) {
        let mut buffers = self.fragment_buffers.lock().unwrap();
        let now = SystemTime::now();
        let mut expired = Vec::new();

        for (message_id, assembly) in buffers.iter() {
            if now
                .duration_since(assembly.started_at)
                .unwrap_or_else(|_| StdDuration::from_secs(0))
                > StdDuration::from_secs(FRAGMENT_TIMEOUT_SECS)
            {
                expired.push(message_id.clone());
            }
        }

        for message_id in expired {
            buffers.remove(&message_id);
            self.record_send_failure();
            eprintln!(
                "⏱️ Dropped expired BLE fragment assembly for message {}",
                message_id
            );
        }
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
        serde_json::to_vec(message).map_err(|e| {
            crate::Error::SerializationError(format!("Failed to serialize message: {}", e))
        })
    }

    /// Deserializes a message from bytes (JSON).
    pub fn deserialize_message(&self, data: &[u8]) -> Result<Message> {
        serde_json::from_slice(data).map_err(|e| {
            crate::Error::SerializationError(format!("Failed to deserialize message: {}", e))
        })
    }

    /// Fragments a message into chunks suitable for BLE transmission.
    ///
    /// Returns a vector of serialized fragments ready to send over BLE.
    pub fn fragment_message(&self, message: &Message) -> Result<Vec<Vec<u8>>> {
        let message_bytes = self.serialize_message(message)?;

        let mtu = self.mtu();
        let message_id = message.id.as_str();
        let message_id_bytes = message_id.as_bytes();
        if message_id_bytes.len() > u8::MAX as usize {
            return Err(crate::Error::Other("Message ID too long".to_string()));
        }

        // Ensure fragment payload fits within MTU once headers are applied
        let header_overhead = FRAGMENT_HEADER_FIXED + message_id_bytes.len();
        if header_overhead >= mtu {
            return Err(crate::Error::Other(
                "MTU too small for fragment header".to_string(),
            ));
        }

        let max_fragment_payload = mtu - header_overhead;
        let total_fragments =
            (message_bytes.len() + max_fragment_payload - 1) / max_fragment_payload;
        if total_fragments == 0 {
            return Err(crate::Error::Other("Empty message".to_string()));
        }
        if total_fragments > MAX_FRAGMENT_COUNT {
            return Err(crate::Error::Other(
                "Message would require too many BLE fragments".to_string(),
            ));
        }

        if total_fragments > u16::MAX as usize {
            return Err(crate::Error::Other(
                "Message too large to fragment".to_string(),
            ));
        }

        let mut fragments = Vec::with_capacity(total_fragments);
        for (i, chunk) in message_bytes.chunks(max_fragment_payload).enumerate() {
            let encoded =
                encode_fragment(message_id_bytes, i as u16, total_fragments as u16, chunk)?;
            fragments.push(encoded);
        }

        Ok(fragments)
    }

    /// Processes an incoming fragment and reassembles if complete.
    ///
    /// Returns Ok(Some(Message)) if message is complete, Ok(None) if more fragments needed.
    pub fn process_fragment(&self, fragment_data: &[u8]) -> Result<Option<Message>> {
        self.cleanup_fragment_buffers();

        // Decode fragment from binary format
        let fragment = decode_fragment(fragment_data)?;

        if fragment.total_fragments == 1 {
            return Ok(Some(self.deserialize_message(&fragment.data)?));
        }

        let mut completed_payload: Option<Vec<u8>> = None;
        let mut assembly_started_at: Option<SystemTime> = None;
        let mut evicted = false;

        {
            // Multi-fragment message - add to reassembly buffer
            let mut buffers = self.fragment_buffers.lock().unwrap();
            let now = SystemTime::now();

            if !buffers.contains_key(&fragment.message_id)
                && buffers.len() >= MAX_FRAGMENT_ASSEMBLIES
            {
                if let Some(oldest_id) = buffers
                    .iter()
                    .min_by_key(|(_, assembly)| assembly.started_at)
                    .map(|(id, _)| id.clone())
                {
                    buffers.remove(&oldest_id);
                    evicted = true;
                }
            }

            // Get or create assembly buffer
            let assembly = buffers
                .entry(fragment.message_id.clone())
                .or_insert_with(|| FragmentAssembly {
                    total_fragments: fragment.total_fragments,
                    fragments: HashMap::new(),
                    started_at: now,
                });

            // Validate fragment
            if assembly.total_fragments != fragment.total_fragments {
                return Err(crate::Error::Other(format!(
                    "Fragment count mismatch: expected {}, got {}",
                    assembly.total_fragments, fragment.total_fragments
                )));
            }

            assembly
                .fragments
                .insert(fragment.fragment_index, fragment.data);

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

                assembly_started_at = Some(assembly.started_at);
                buffers.remove(&fragment.message_id);
                completed_payload = Some(complete_data);
            }
        }

        if evicted {
            self.record_send_failure();
        }

        if let Some(payload) = completed_payload {
            let start = assembly_started_at.unwrap_or_else(SystemTime::now);
            let latency = SystemTime::now()
                .duration_since(start)
                .unwrap_or_else(|_| StdDuration::from_millis(0))
                .as_millis();
            self.record_latency(latency);

            // Deserialize complete message
            let message = self.deserialize_message(&payload)?;
            return Ok(Some(message));
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
                queue.push_back(message.clone());
                eprintln!(
                    "🎉 COMPLETE MESSAGE ASSEMBLED: {} from {} to {}",
                    message.id.as_str(),
                    message.sender.as_str(),
                    message.recipient.as_str()
                );
                eprintln!("📬 Message content: {}", message.content);
                Ok(())
            }
            Ok(None) => {
                // More fragments needed
                eprintln!("📦 Fragment received, more needed for complete message");
                Ok(())
            }
            Err(e) => {
                // Log error but don't fail - just drop bad fragment
                eprintln!("❌ Error processing fragment: {}", e);
                Ok(())
            }
        }
    }

    /// Gets the next fragment to send (for platform implementation).
    ///
    /// Returns (recipient, fragment_data) or None if no messages to send.
    pub fn get_next_fragment(&self) -> Result<Option<(String, Vec<u8>)>> {
        // Check for pending fragments first
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

fn encode_fragment(
    message_id: &[u8],
    fragment_index: u16,
    total_fragments: u16,
    data: &[u8],
) -> Result<Vec<u8>> {
    if data.len() > u16::MAX as usize {
        return Err(crate::Error::Other(
            "Fragment payload too large".to_string(),
        ));
    }

    let mut encoded = Vec::with_capacity(FRAGMENT_HEADER_FIXED + message_id.len() + data.len());
    encoded.extend_from_slice(&FRAGMENT_MAGIC);
    encoded.push(FRAGMENT_VERSION);
    encoded.push(message_id.len() as u8);
    encoded.extend_from_slice(message_id);
    encoded.extend_from_slice(&fragment_index.to_le_bytes());
    encoded.extend_from_slice(&total_fragments.to_le_bytes());
    encoded.extend_from_slice(&(data.len() as u16).to_le_bytes());
    encoded.extend_from_slice(data);
    Ok(encoded)
}

fn decode_fragment(fragment_data: &[u8]) -> Result<DecodedFragment> {
    if fragment_data.len() < FRAGMENT_HEADER_FIXED {
        return Err(crate::Error::Other("Fragment too short".to_string()));
    }

    if &fragment_data[0..2] != FRAGMENT_MAGIC {
        return Err(crate::Error::Other("Invalid fragment magic".to_string()));
    }

    let version = fragment_data[2];
    if version != FRAGMENT_VERSION {
        return Err(crate::Error::Other(format!(
            "Unsupported fragment version {}",
            version
        )));
    }

    let id_len = fragment_data[3] as usize;
    let header_len = FRAGMENT_HEADER_FIXED + id_len;
    if fragment_data.len() < header_len {
        return Err(crate::Error::Other("Fragment truncated (id)".to_string()));
    }

    let mut offset = 4;
    let message_id_bytes = &fragment_data[offset..offset + id_len];
    offset += id_len;

    let message_id = String::from_utf8(message_id_bytes.to_vec())
        .map_err(|_| crate::Error::Other("Invalid UTF-8 in message ID".to_string()))?;

    if fragment_data.len() < offset + 6 {
        return Err(crate::Error::Other(
            "Fragment truncated (header)".to_string(),
        ));
    }

    let fragment_index = u16::from_le_bytes(
        fragment_data[offset..offset + 2]
            .try_into()
            .map_err(|_| crate::Error::Other("Fragment truncated (index)".to_string()))?,
    );
    offset += 2;
    let total_fragments = u16::from_le_bytes(
        fragment_data[offset..offset + 2]
            .try_into()
            .map_err(|_| crate::Error::Other("Fragment truncated (total)".to_string()))?,
    );
    offset += 2;
    let data_len = u16::from_le_bytes(
        fragment_data[offset..offset + 2]
            .try_into()
            .map_err(|_| crate::Error::Other("Fragment truncated (length)".to_string()))?,
    ) as usize;
    offset += 2;

    if fragment_data.len() < offset + data_len {
        return Err(crate::Error::Other("Fragment truncated (data)".to_string()));
    }

    let data = fragment_data[offset..offset + data_len].to_vec();

    Ok(DecodedFragment {
        message_id,
        fragment_index,
        total_fragments,
        data,
    })
}

impl Transport for BleTransport {
    fn as_any(&self) -> &dyn std::any::Any {
        self
    }

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
        let status = self.status();

        // Check status
        if status != TransportStatus::Available {
            return Err(crate::Error::TransportNotAvailable(format!(
                "BLE transport is not available (status: {:?})",
                status
            )));
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
        // Set status to Available when starting
        // Platform can still override this via on_status_changed() if BLE is not available
        *self.status.lock().unwrap() = TransportStatus::Available;
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

    #[test]
    fn test_fragment_roundtrip() {
        use offline_protocol_core::{AppId, Message, MessagePriority, UserId, TTL};

        let transport = BleTransport::new("test-device");

        let sender = UserId::new("alice").unwrap();
        let recipient = UserId::new("bob").unwrap();
        let app_id = AppId::new("app").unwrap();
        let content = "x".repeat(512);

        let message = Message::builder(sender, recipient, app_id)
            .content(content.clone())
            .priority(MessagePriority::High)
            .ttl(TTL::new(8).unwrap())
            .build();

        let fragments = transport.fragment_message(&message).unwrap();
        assert!(fragments.len() > 1); // should fragment
        for fragment in &fragments {
            assert!(fragment.len() <= MAX_FRAGMENT_SIZE);
        }

        let mut reconstructed = None;
        for fragment in fragments {
            if let Some(msg) = transport.process_fragment(&fragment).unwrap() {
                reconstructed = Some(msg);
            }
        }

        let reconstructed = reconstructed.expect("Expected complete message");
        assert_eq!(reconstructed.content, content);
    }
}
