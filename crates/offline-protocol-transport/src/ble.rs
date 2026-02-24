//! Bluetooth Low Energy (BLE) transport implementation.
//!
//! This module provides the BLE transport layer for peer-to-peer communication.
//! It handles:
//! - Device discovery (advertising and scanning)
//! - GATT server/client operations
//! - Message transmission over BLE characteristics
//! - Message fragmentation for large payloads

use crate::constants::{
    BLE_FRAGMENT_TIMEOUT_SECS, BLE_MAX_FRAGMENT_ASSEMBLIES, BLE_MAX_FRAGMENT_COUNT,
    BLE_MAX_FRAGMENT_SIZE, FRAGMENT_HEADER_FIXED, FRAGMENT_MAGIC, FRAGMENT_VERSION,
};
use crate::{Result, Transport, TransportMetrics, TransportStatus, TransportType};
use offline_protocol_core::Message;
use std::collections::{HashMap, VecDeque};
use std::convert::TryInto;
use std::sync::{Arc, Mutex};
use std::time::{Duration as StdDuration, SystemTime};

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

impl FragmentAssembly {
    /// Returns the completion ratio (0.0 to 1.0) of this assembly.
    fn completion_ratio(&self) -> f32 {
        if self.total_fragments == 0 {
            return 0.0;
        }
        self.fragments.len() as f32 / self.total_fragments as f32
    }

    /// Returns a priority score for eviction (lower = more likely to be evicted).
    /// Prioritizes keeping near-complete assemblies.
    fn eviction_priority(&self, now: SystemTime) -> f32 {
        let completion = self.completion_ratio();

        // Age factor: older assemblies are slightly less valuable
        let age_secs = now
            .duration_since(self.started_at)
            .unwrap_or(StdDuration::from_secs(0))
            .as_secs_f32();
        let age_penalty = (age_secs / 60.0).min(1.0) * 0.2; // Max 20% penalty for age

        // Priority = completion ratio (0-1) minus age penalty
        // Higher value = more valuable = less likely to evict
        completion - age_penalty
    }
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
    #[allow(clippy::type_complexity)]
    pending_fragments: Arc<Mutex<VecDeque<(String, Vec<u8>)>>>,
    /// Transport metrics
    metrics: Arc<Mutex<TransportMetrics>>,
    /// Platform-specific handle (opaque pointer)
    platform_handle: Arc<Mutex<Option<usize>>>,
    /// Fragment reassembly buffers
    fragment_buffers: Arc<Mutex<HashMap<String, FragmentAssembly>>>,
    /// Negotiated MTU size (set by platform)
    mtu_size: Arc<Mutex<usize>>,
    /// Platform callback invoked when new fragments are available to send.
    /// Called from `send()` after enqueueing — the platform layer should
    /// respond by calling `get_next_fragment()` and performing the BLE write.
    on_fragments_available: Arc<Mutex<Option<Arc<dyn Fn() + Send + Sync>>>>,
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
            mtu_size: Arc::new(Mutex::new(BLE_MAX_FRAGMENT_SIZE)),
            on_fragments_available: Arc::new(Mutex::new(None)),
        }
    }

    /// Registers a callback that fires when new outgoing fragments become available.
    ///
    /// The platform layer (Swift/Kotlin) implements this to wake up and call
    /// `get_next_fragment()` instead of polling on a timer.
    pub fn set_on_fragments_available(&self, callback: Arc<dyn Fn() + Send + Sync>) {
        *self.on_fragments_available.lock().unwrap() = Some(callback);
    }

    /// Notifies the platform that fragments are ready to send.
    fn notify_fragments_available(&self) {
        let callback = self.on_fragments_available.lock().unwrap().clone();
        if let Some(cb) = callback {
            cb();
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
                > StdDuration::from_secs(BLE_FRAGMENT_TIMEOUT_SECS)
            {
                expired.push(message_id.clone());
            }
        }

        for message_id in expired {
            buffers.remove(&message_id);
            self.record_send_failure();
            tracing::debug!(
                message_id = %message_id,
                "Dropped expired BLE fragment assembly"
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
        if total_fragments > BLE_MAX_FRAGMENT_COUNT {
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
                && buffers.len() >= BLE_MAX_FRAGMENT_ASSEMBLIES
            {
                // Priority-based eviction: prefer evicting assemblies with less progress
                // rather than just the oldest. This preserves near-complete assemblies.
                if let Some(evict_id) = buffers
                    .iter()
                    .min_by(|(_, a), (_, b)| {
                        let priority_a = a.eviction_priority(now);
                        let priority_b = b.eviction_priority(now);
                        priority_a
                            .partial_cmp(&priority_b)
                            .unwrap_or(std::cmp::Ordering::Equal)
                    })
                    .map(|(id, _)| id.clone())
                {
                    tracing::debug!(
                        message_id = %evict_id,
                        "Evicting fragment assembly to make room (priority-based)"
                    );
                    buffers.remove(&evict_id);
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
                // Note: sender/recipient are intentionally not logged to protect user privacy
                tracing::debug!(
                    message_id = %message.id,
                    "Complete message assembled from fragments"
                );
                Ok(())
            }
            Ok(None) => {
                // More fragments needed
                tracing::debug!("Fragment received, more needed for complete message");
                Ok(())
            }
            Err(e) => {
                // Log error but don't fail - just drop bad fragment
                tracing::warn!(error = %e, "Error processing fragment, dropping bad fragment");
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

    if fragment_data[0..2] != FRAGMENT_MAGIC {
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

        // Notify platform that fragments are available to send.
        // This replaces timer-based polling — the platform will call
        // get_next_fragment() in response to this callback.
        self.notify_fragments_available();

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
    use offline_protocol_core::{AppId, Message, MessagePriority, UserId, TTL};

    fn peer_device(id: &str) -> PeerDevice {
        PeerDevice {
            device_id: id.to_string(),
            address: "AA:BB:CC:DD:EE:FF".to_string(),
            rssi: -60,
            last_seen: std::time::SystemTime::now(),
            connected: false,
        }
    }

    fn small_message() -> Message {
        Message::new(
            UserId::new("alice").unwrap(),
            UserId::new("bob").unwrap(),
            AppId::new("app").unwrap(),
            "hi",
        )
    }

    #[test]
    fn test_ble_transport_creation() {
        let transport = BleTransport::new("test-device");
        assert_eq!(transport.device_id(), "test-device");
        assert_eq!(transport.status(), TransportStatus::Unavailable);
    }

    #[test]
    fn test_ble_send_when_unavailable_fails() {
        let transport = BleTransport::new("test-device");
        let msg = small_message();
        let result = transport.send(&msg);
        assert!(result.is_err());
        assert!(matches!(result.unwrap_err(), crate::Error::TransportNotAvailable(_)));
    }

    #[test]
    fn test_ble_start_stop() {
        let mut transport = BleTransport::new("test-device");
        transport.start().unwrap();
        assert_eq!(transport.status(), TransportStatus::Available);
        transport.stop().unwrap();
        assert_eq!(transport.status(), TransportStatus::Disconnected);
    }

    #[test]
    fn test_ble_on_status_changed() {
        let transport = BleTransport::new("test-device");
        transport.on_status_changed(TransportStatus::Available);
        assert_eq!(transport.status(), TransportStatus::Available);
        transport.on_status_changed(TransportStatus::Error);
        assert_eq!(transport.status(), TransportStatus::Error);
    }

    #[test]
    fn test_ble_set_mtu_and_mtu() {
        let transport = BleTransport::new("test-device");
        assert_eq!(transport.mtu(), BLE_MAX_FRAGMENT_SIZE);
        transport.set_mtu(100);
        assert_eq!(transport.mtu(), 97); // 100 - 3 ATT overhead
    }

    #[test]
    fn test_ble_platform_handle() {
        let transport = BleTransport::new("test-device");
        assert_eq!(transport.platform_handle(), None);
        transport.set_platform_handle(42);
        assert_eq!(transport.platform_handle(), Some(42));
    }

    #[test]
    fn test_ble_update_metrics() {
        let transport = BleTransport::new("test-device");
        let mut m = TransportMetrics::default();
        m.rssi = Some(-70);
        transport.update_metrics(m);
        assert_eq!(transport.metrics().rssi, Some(-70));
    }

    #[test]
    fn test_ble_record_send_success_failure() {
        let transport = BleTransport::new("test-device");
        transport.record_send_success();
        transport.record_send_success();
        transport.record_send_failure();
        let metrics = transport.metrics();
        assert_eq!(metrics.success_count, 2);
        assert_eq!(metrics.failure_count, 1);
    }

    #[test]
    fn test_ble_peer_discovery() {
        let transport = BleTransport::new("test-device");
        transport.on_peer_discovered(peer_device("peer-1"));
        let peers = transport.get_peers();
        assert_eq!(peers.len(), 1);
        assert_eq!(peers[0].device_id, "peer-1");
        assert!(transport.get_peer("peer-1").is_some());
        assert!(transport.get_peer("other").is_none());
    }

    #[test]
    fn test_ble_peer_lost() {
        let transport = BleTransport::new("test-device");
        transport.on_peer_discovered(peer_device("peer-1"));
        transport.on_peer_lost("peer-1");
        assert_eq!(transport.get_peers().len(), 0);
    }

    #[test]
    fn test_ble_has_pending_sends_dequeue_send_get_queue_depth() {
        let mut transport = BleTransport::new("test-device");
        transport.start().unwrap();
        let msg = small_message();
        transport.send(&msg).unwrap();
        assert!(transport.has_pending_sends());
        assert_eq!(transport.get_queue_depth(), 1);
        let dequeued = transport.dequeue_send();
        assert!(dequeued.is_some());
        assert!(!transport.has_pending_sends());
        assert_eq!(transport.get_queue_depth(), 0);
        assert!(transport.dequeue_send().is_none());
    }

    #[test]
    fn test_ble_serialize_deserialize_message() {
        let transport = BleTransport::new("test-device");
        let msg = small_message();
        let data = transport.serialize_message(&msg).unwrap();
        let back = transport.deserialize_message(&data).unwrap();
        assert_eq!(back.id, msg.id);
        assert_eq!(back.content, msg.content);
    }

    #[test]
    fn test_ble_deserialize_invalid_json() {
        let transport = BleTransport::new("test-device");
        let result = transport.deserialize_message(b"not json");
        assert!(result.is_err());
        assert!(matches!(result.unwrap_err(), crate::Error::SerializationError(_)));
    }

    #[test]
    fn test_ble_single_fragment_roundtrip() {
        let transport = BleTransport::new("test-device");
        transport.set_mtu(512);
        let msg = small_message();
        let fragments = transport.fragment_message(&msg).unwrap();
        assert_eq!(fragments.len(), 1, "small message with large MTU should fit in one fragment");
        let reconstructed = transport.process_fragment(&fragments[0]).unwrap();
        assert!(reconstructed.is_some());
        assert_eq!(reconstructed.unwrap().content, msg.content);
    }

    #[test]
    fn test_ble_fragment_roundtrip() {
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
        assert!(fragments.len() > 1);
        for fragment in &fragments {
            assert!(fragment.len() <= BLE_MAX_FRAGMENT_SIZE);
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

    #[test]
    fn test_ble_process_fragment_invalid_magic() {
        let transport = BleTransport::new("test-device");
        let mut bad = vec![0x00, 0x00, 1, 0, 0, 0, 0, 0, 0, 0]; // wrong magic
        bad.extend_from_slice(b"{}");
        let result = transport.process_fragment(&bad);
        assert!(result.is_err());
        assert!(matches!(result.unwrap_err(), crate::Error::Other(s) if s.contains("magic")));
    }

    #[test]
    fn test_ble_process_fragment_too_short() {
        let transport = BleTransport::new("test-device");
        let result = transport.process_fragment(&[0x4f, 0x50]); // "OP" only
        assert!(result.is_err());
        assert!(matches!(result.unwrap_err(), crate::Error::Other(s) if s.contains("short") || s.contains("truncat")));
    }

    #[test]
    fn test_ble_process_fragment_wrong_version() {
        let transport = BleTransport::new("test-device");
        // Minimal header with wrong version: magic(2) + version(1)=99 + id_len(1)=0 + index(2) + total(2) + data_len(2)
        let bad = [
            b'O', b'P', 99u8, 0u8,
            0u8, 0u8, 1u8, 0u8, 0u8, 0u8,
        ];
        let result = transport.process_fragment(&bad);
        assert!(result.is_err());
        assert!(matches!(result.unwrap_err(), crate::Error::Other(s) if s.contains("version")));
    }

    #[test]
    fn test_ble_on_fragment_received_complete_queues_message() {
        let transport = BleTransport::new("test-device");
        transport.set_mtu(512);
        let msg = small_message();
        let fragments = transport.fragment_message(&msg).unwrap();
        for fragment in &fragments {
            transport.on_fragment_received(fragment.clone()).unwrap();
        }
        let received = transport.receive().unwrap();
        assert!(received.is_some());
        assert_eq!(received.unwrap().content, msg.content);
    }

    #[test]
    fn test_ble_on_fragment_received_bad_data_drops_ok() {
        let transport = BleTransport::new("test-device");
        let result = transport.on_fragment_received(vec![0u8; 5]);
        assert!(result.is_ok()); // drops bad fragment, doesn't propagate error
    }

    #[test]
    fn test_ble_get_next_fragment_requeue() {
        let mut transport = BleTransport::new("test-device");
        transport.start().unwrap();
        let msg = small_message();
        transport.send(&msg).unwrap();
        let first = transport.get_next_fragment().unwrap();
        assert!(first.is_some());
        let (recipient, data) = first.unwrap();
        transport.requeue_fragment(&recipient, data.clone());
        let again = transport.get_next_fragment().unwrap();
        assert!(again.is_some());
        assert_eq!(again.unwrap().1, data);
    }

    #[test]
    fn test_ble_get_next_fragment_none_when_empty() {
        let transport = BleTransport::new("test-device");
        assert!(transport.get_next_fragment().unwrap().is_none());
    }

    #[test]
    fn test_ble_on_message_received() {
        let transport = BleTransport::new("test-device");
        let msg = small_message();
        transport.on_message_received(msg.clone());
        let received = transport.receive().unwrap();
        assert!(received.is_some());
        assert_eq!(received.unwrap().id, msg.id);
    }

    #[test]
    fn test_ble_transport_builder() {
        let transport = BleTransportBuilder::new("my-device").build();
        assert_eq!(transport.device_id(), "my-device");
        assert_eq!(transport.transport_type(), TransportType::BLE);
    }
}
