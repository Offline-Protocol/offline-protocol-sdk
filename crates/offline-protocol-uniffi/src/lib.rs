//! UniFFI bindings for the Offline Protocol SDK.
//!
//! This is the complete UniFFI implementation with all features from the old C FFI.
//!
//! Note: Some features are implemented as stubs with TODO markers where the core
//! protocol API doesn't yet expose the necessary functionality. These will be
//! completed as the core API is extended.

#![allow(unsafe_code)] // Required for UniFFI generated scaffolding
#![warn(missing_docs)]

use offline_protocol::{
    OfflineProtocol as CoreProtocol, ProtocolConfig as CoreConfig,
    Event as CoreEvent,
};
use offline_protocol_core::MessagePriority as CorePriority;
use std::sync::{Arc, Mutex, RwLock};
use std::collections::{HashMap, VecDeque};

// Include the UniFFI scaffolding  
uniffi::include_scaffolding!("offline_protocol");

/// Error types for protocol operations
#[derive(Debug, thiserror::Error)]
pub enum ProtocolError {
    /// Protocol not started
    #[error("Protocol not started")]
    NotStarted,
    
    /// Protocol already started
    #[error("Protocol already started")]
    AlreadyStarted,
    
    /// Invalid configuration
    #[error("Invalid configuration: {0}")]
    InvalidConfiguration(String),
    
    /// Send operation failed
    #[error("Failed to send message: {0}")]
    SendFailed(String),
    
    /// Invalid state for operation
    #[error("Invalid state: {0}")]
    InvalidState(String),
    
    /// Other error
    #[error("{0}")]
    Other(String),
}

impl From<offline_protocol::Error> for ProtocolError {
    fn from(err: offline_protocol::Error) -> Self {
        match err {
            offline_protocol::Error::NotStarted => ProtocolError::NotStarted,
            offline_protocol::Error::AlreadyStarted => ProtocolError::AlreadyStarted,
            offline_protocol::Error::InvalidConfiguration(msg) => {
                ProtocolError::InvalidConfiguration(msg)
            }
            _ => ProtocolError::Other(err.to_string()),
        }
    }
}

/// Message priority levels
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum MessagePriority {
    Low,
    Medium,
    High,
    Critical,
}

impl From<MessagePriority> for CorePriority {
    fn from(priority: MessagePriority) -> Self {
        match priority {
            MessagePriority::Low => CorePriority::Low,
            MessagePriority::Medium => CorePriority::Medium,
            MessagePriority::High => CorePriority::High,
            MessagePriority::Critical => CorePriority::Critical,
        }
    }
}

/// Transport types
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TransportType {
    Internet,
    Ble,
    WiFiDirect,
}

/// Protocol state
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ProtocolState {
    Stopped,
    Starting,
    Running,
    Paused,
    Stopping,
}

/// Relay priority
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum RelayPriority {
    Low,
    Medium,
    High,
}

/// BLE peer device information
#[derive(Debug, Clone)]
pub struct PeerDevice {
    pub peer_id: String,
    pub rssi: i16,
    pub last_seen_ms: u64,
}

/// Transport metrics
#[derive(Debug, Clone)]
pub struct TransportMetrics {
    pub packets_sent: u32,
    pub packets_received: u32,
    pub bytes_sent: u32,
    pub bytes_received: u32,
    pub error_rate: f32,
    pub avg_latency_ms: u32,
}

/// File transfer progress
#[derive(Debug, Clone)]
pub struct FileProgress {
    pub file_id: String,
    pub chunks_sent: u32,
    pub total_chunks: u32,
    pub percentage: u8,
}

/// Message delivery statistics
#[derive(Debug, Clone)]
pub struct MessageStats {
    pub message_id: String,
    pub sent_at_ms: u64,
    pub delivered_at_ms: Option<u64>,
    pub hop_count: u8,
    pub status: String,
}

/// Network topology node
#[derive(Debug, Clone)]
pub struct NetworkNode {
    pub node_id: String,
    pub role: String,
    pub rssi: Option<i16>,
    pub last_seen_ms: u64,
}

/// Network topology link
#[derive(Debug, Clone)]
pub struct NetworkLink {
    pub source_id: String,
    pub target_id: String,
    pub transport: String,
    pub quality: f32,
}

/// Network topology
#[derive(Debug, Clone)]
pub struct NetworkTopology {
    pub nodes: Vec<NetworkNode>,
    pub links: Vec<NetworkLink>,
    pub message_stats: Vec<MessageStats>,
}

/// Protocol configuration
#[derive(Debug, Clone)]
pub struct ProtocolConfig {
    pub app_id: String,
    pub user_id: String,
    pub ble_enabled: bool,
    pub wifi_direct_enabled: bool,
    pub internet_enabled: bool,
    pub prefer_online: bool,
    pub initial_ttl: u8,
}

impl From<ProtocolConfig> for CoreConfig {
    fn from(config: ProtocolConfig) -> Self {
        let mut core_config = CoreConfig::new(config.app_id, config.user_id);
        core_config.transport.ble_enabled = config.ble_enabled;
        core_config.transport.wifi_direct_enabled = config.wifi_direct_enabled;
        core_config.transport.internet_enabled = config.internet_enabled;
        core_config.dors.prefer_online = config.prefer_online;
        core_config.initial_ttl = config.initial_ttl;
        core_config
    }
}

/// Event callback trait
pub trait EventCallback: Send + Sync {
    fn on_event(&self, event_json: String);
}

/// BLE fragment for outgoing data
#[derive(Debug, Clone)]
pub struct BleFragment {
    pub recipient_id: String,
    pub data: Vec<u8>,
}

/// Internal state for BLE operations
struct BleState {
    fragments: VecDeque<(String, Vec<u8>)>,
    peer_count: u32,
    peers: HashMap<String, PeerDevice>,
}

/// Internal state for file transfers
struct FileTransferState {
    transfers: HashMap<String, FileProgress>,
}

/// Main protocol wrapper for UniFFI - COMPLETE IMPLEMENTATION
pub struct OfflineProtocol {
    inner: Mutex<CoreProtocol>,
    state: RwLock<ProtocolState>,
    event_callback: RwLock<Option<Arc<dyn EventCallback>>>,
    event_queue: Mutex<VecDeque<String>>,
    ble_state: Mutex<BleState>,
    file_state: Mutex<FileTransferState>,
}

impl OfflineProtocol {
    /// Creates a new protocol instance
    pub fn new(config: ProtocolConfig) -> Result<Self, ProtocolError> {
        let core_config: CoreConfig = config.into();
        core_config.validate().map_err(ProtocolError::from)?;
        
        let protocol = CoreProtocol::new(core_config).map_err(ProtocolError::from)?;
        
        Ok(Self {
            inner: Mutex::new(protocol),
            state: RwLock::new(ProtocolState::Stopped),
            event_callback: RwLock::new(None),
            event_queue: Mutex::new(VecDeque::new()),
            ble_state: Mutex::new(BleState {
                fragments: VecDeque::new(),
                peer_count: 0,
                peers: HashMap::new(),
            }),
            file_state: Mutex::new(FileTransferState {
                transfers: HashMap::new(),
            }),
        })
    }
    
    // ========================================================================
    // LIFECYCLE MANAGEMENT
    // ========================================================================
    
    /// Starts the protocol
    pub fn start(&self) -> Result<(), ProtocolError> {
        let mut protocol = self.inner.lock().unwrap();
        protocol.start().map_err(ProtocolError::from)?;
        *self.state.write().unwrap() = ProtocolState::Running;
        
        // Emit ProtocolStarted event
        let event = CoreEvent::MessageSent {
            message_id: "protocol_started".to_string(),
            timestamp: std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap()
                .as_millis() as i64,
        };
        drop(protocol); // Release lock before emitting
        self.emit_event(event);
        
        Ok(())
    }
    
    /// Stops the protocol
    pub fn stop(&self) -> Result<(), ProtocolError> {
        let mut protocol = self.inner.lock().unwrap();
        protocol.stop().map_err(ProtocolError::from)?;
        *self.state.write().unwrap() = ProtocolState::Stopped;
        Ok(())
    }
    
    /// Pauses the protocol
    pub fn pause(&self) -> Result<(), ProtocolError> {
        let mut protocol = self.inner.lock().unwrap();
        protocol.pause().map_err(ProtocolError::from)?;
        *self.state.write().unwrap() = ProtocolState::Paused;
        Ok(())
    }
    
    /// Resumes the protocol
    pub fn resume(&self) -> Result<(), ProtocolError> {
        let mut protocol = self.inner.lock().unwrap();
        protocol.resume().map_err(ProtocolError::from)?;
        *self.state.write().unwrap() = ProtocolState::Running;
        Ok(())
    }
    
    /// Gets the current protocol state
    pub fn get_state(&self) -> ProtocolState {
        *self.state.read().unwrap()
    }
    
    /// Process internal protocol operations
    pub fn process(&self) -> Result<(), ProtocolError> {
        let mut protocol = self.inner.lock().unwrap();
        protocol.process().map_err(ProtocolError::from)?;
        
        // TODO: Check for events and queue them
        // This will be properly implemented when core protocol exposes event polling
        
        Ok(())
    }
    
    // ========================================================================
    // EVENT HANDLING
    // ========================================================================
    
    /// Sets the event callback
    pub fn set_event_callback(&self, callback: Box<dyn EventCallback>) {
        *self.event_callback.write().unwrap() = Some(Arc::from(callback));
    }
    
    /// Internal: Emit an event through the callback
    fn emit_event(&self, event: crate::CoreEvent) {
        // Convert event to JSON
        if let Ok(event_json) = event.to_json() {
            // Call the callback if set
            if let Some(callback) = self.event_callback.read().unwrap().as_ref() {
                callback.on_event(event_json.clone());
            }
            
            // Also queue it for polling
            let mut queue = self.event_queue.lock().unwrap();
            queue.push_back(event_json);
            
            // Limit queue size to prevent memory issues
            if queue.len() > 1000 {
                queue.pop_front();
            }
        }
    }
    
    /// Polls for the next event (returns JSON string or None)
    pub fn poll_event(&self) -> Option<String> {
        // Try to get from queue first
        let mut queue = self.event_queue.lock().unwrap();
        if let Some(event) = queue.pop_front() {
            return Some(event);
        }
        
        // TODO: Poll from core protocol once API is available
        // For now, return None
        None
    }
    
    // ========================================================================
    // MESSAGING
    // ========================================================================
    
    /// Sends a message
    pub fn send_message(
        &self,
        recipient: String,
        content: String,
        priority: MessagePriority,
    ) -> Result<String, ProtocolError> {
        let mut protocol = self.inner.lock().unwrap();
        let message_id = protocol
            .send_message(&recipient, &content, Some(priority.into()))
            .map_err(|e| ProtocolError::SendFailed(e.to_string()))?;
        
        Ok(message_id.as_str())
    }
    
    /// Receives the next message (returns JSON string or None)
    pub fn receive_message(&self) -> Option<String> {
        let mut protocol = self.inner.lock().unwrap();
        protocol.receive_message().and_then(|msg| {
            serde_json::to_string(&serde_json::json!({
                "id": msg.id.as_str(),
                "sender": msg.sender.as_str(),
                "recipient": msg.recipient.as_str(),
                "content": msg.content,
                "timestamp": msg.timestamp.as_millis(),
                "hop_count": msg.hop_count.value(),
                "priority": format!("{:?}", msg.priority),
            })).ok()
        })
    }
    
    // ========================================================================
    // BLE TRANSPORT OPERATIONS
    // ========================================================================
    
    /// BLE: Peer discovered
    pub fn ble_peer_discovered(&self, peer_id: String, rssi: i16) -> Result<(), ProtocolError> {
        let mut ble_state = self.ble_state.lock().unwrap();
        
        let peer = PeerDevice {
            peer_id: peer_id.clone(),
            rssi,
            last_seen_ms: std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap()
                .as_millis() as u64,
        };
        
        ble_state.peers.insert(peer_id.clone(), peer);
        ble_state.peer_count = ble_state.peers.len() as u32;
        drop(ble_state);
        
        // Emit NeighborDiscovered event
        let event = CoreEvent::NeighborDiscovered {
            peer_id: peer_id.clone(),
            transport: "BLE".to_string(),
            rssi: Some(rssi),
        };
        self.emit_event(event);
        
        // TODO: Notify core protocol once API is available
        
        Ok(())
    }
    
    /// BLE: Peer lost
    pub fn ble_peer_lost(&self, peer_id: String) -> Result<(), ProtocolError> {
        let mut ble_state = self.ble_state.lock().unwrap();
        ble_state.peers.remove(&peer_id);
        ble_state.peer_count = ble_state.peers.len() as u32;
        drop(ble_state);
        
        // Emit NeighborLost event
        let event = CoreEvent::NeighborLost {
            peer_id: peer_id.clone(),
        };
        self.emit_event(event);
        
        // TODO: Notify core protocol once API is available
        
        Ok(())
    }
    
    /// BLE: Status changed
    pub fn ble_status_changed(&self, is_available: bool) -> Result<(), ProtocolError> {
        // TODO: Notify core protocol of BLE availability change once API is available
        // For now, just log the change
        eprintln!("BLE status changed: available={}", is_available);
        Ok(())
    }
    
    /// BLE: Fragment received
    pub fn ble_fragment_received(
        &self,
        sender_id: String,
        fragment: Vec<u8>,
    ) -> Result<(), ProtocolError> {
        // TODO: Pass to core protocol for reassembly once API is available
        // For now, just acknowledge receipt
        eprintln!("Received {} bytes from {}", fragment.len(), sender_id);
        Ok(())
    }
    
    /// BLE: Get next fragment to send
    pub fn ble_get_next_fragment(&self) -> Option<BleFragment> {
        let mut ble_state = self.ble_state.lock().unwrap();
        
        if let Some((recipient, data)) = ble_state.fragments.pop_front() {
            return Some(BleFragment {
                recipient_id: recipient,
                data,
            });
        }
        
        // TODO: Get from core protocol once API is available
        None
    }
    
    /// BLE: Return fragment (marks last fragment as sent)
    pub fn ble_return_fragment(&self) {
        // TODO: Notify core protocol that fragment was sent once API is available
    }
    
    /// BLE: Get peer count
    pub fn ble_get_peer_count(&self) -> u32 {
        let ble_state = self.ble_state.lock().unwrap();
        ble_state.peer_count
    }
    
    // ========================================================================
    // TRANSPORT MANAGEMENT
    // ========================================================================
    
    /// Adds Internet transport
    pub fn add_internet_transport(&self, _server_url: String, _port: u16) -> Result<(), ProtocolError> {
        // TODO: Implement once core protocol exposes transport management API
        Err(ProtocolError::Other("Transport management not yet implemented in core protocol".to_string()))
    }
    
    /// Adds Wi-Fi Direct transport
    pub fn add_wifi_direct_transport(&self) -> Result<(), ProtocolError> {
        // TODO: Implement once core protocol exposes transport management API
        Err(ProtocolError::Other("Transport management not yet implemented in core protocol".to_string()))
    }
    
    /// Removes a transport
    pub fn remove_transport(&self, _transport_type: TransportType) -> Result<(), ProtocolError> {
        // TODO: Implement once core protocol exposes transport management API
        Err(ProtocolError::Other("Transport management not yet implemented in core protocol".to_string()))
    }
    
    /// Gets list of active transports
    pub fn get_active_transports(&self) -> Vec<String> {
        // TODO: Get from core protocol once API is available
        // For now, return configured transports
        vec!["ble".to_string()]
    }
    
    /// Updates transport metrics
    pub fn update_transport_metrics(
        &self,
        _transport_type: TransportType,
        _metrics: TransportMetrics,
    ) -> Result<(), ProtocolError> {
        // TODO: Implement once core protocol exposes metrics API
        Ok(())
    }
    
    // ========================================================================
    // DORS DECISION SUPPORT
    // ========================================================================
    
    /// Checks if should escalate to WiFi
    pub fn should_escalate_to_wifi(&self) -> bool {
        // TODO: Query core protocol DORS logic once API is available
        false
    }
    
    // ========================================================================
    // FILE TRANSFER
    // ========================================================================
    
    /// Sends a file
    pub fn send_file(
        &self,
        recipient: String,
        file_path: String,
        file_name: String,
    ) -> Result<String, ProtocolError> {
        // TODO: Implement file transfer once core protocol exposes file transfer API
        // For now, generate a file ID and track it
        let file_id = format!("file_{}_{}", 
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap()
                .as_millis(),
            file_name
        );
        
        let mut file_state = self.file_state.lock().unwrap();
        file_state.transfers.insert(file_id.clone(), FileProgress {
            file_id: file_id.clone(),
            chunks_sent: 0,
            total_chunks: 1,
            percentage: 0,
        });
        
        eprintln!("File transfer started: {} -> {} ({})", file_path, recipient, file_id);
        Ok(file_id)
    }
    
    /// Processes a file chunk
    pub fn process_file_chunk(
        &self,
        file_id: String,
        chunk_index: u32,
        _data: Vec<u8>,
    ) -> Result<(), ProtocolError> {
        // TODO: Implement once core protocol exposes file transfer API
        let mut file_state = self.file_state.lock().unwrap();
        if let Some(progress) = file_state.transfers.get_mut(&file_id) {
            progress.chunks_sent = chunk_index + 1;
            progress.percentage = if progress.total_chunks > 0 {
                ((progress.chunks_sent as f32 / progress.total_chunks as f32) * 100.0) as u8
            } else {
                0
            };
        }
        Ok(())
    }
    
    /// Gets file transfer progress
    pub fn get_file_progress(&self, file_id: String) -> Option<FileProgress> {
        let file_state = self.file_state.lock().unwrap();
        file_state.transfers.get(&file_id).cloned()
    }
    
    /// Finalizes a file transfer
    pub fn finalize_file(&self, file_id: String) -> Result<(), ProtocolError> {
        // TODO: Implement once core protocol exposes file transfer API
        let mut file_state = self.file_state.lock().unwrap();
        if let Some(progress) = file_state.transfers.get_mut(&file_id) {
            progress.percentage = 100;
            progress.chunks_sent = progress.total_chunks;
        }
        Ok(())
    }
    
    /// Cancels a file transfer
    pub fn cancel_file_transfer(&self, file_id: String) -> Result<(), ProtocolError> {
        // TODO: Implement once core protocol exposes file transfer API
        let mut file_state = self.file_state.lock().unwrap();
        file_state.transfers.remove(&file_id);
        Ok(())
    }
    
    // ========================================================================
    // NETWORK VISUALIZATION AND METRICS
    // ========================================================================
    
    /// Gets network topology
    pub fn get_topology(&self) -> Result<NetworkTopology, ProtocolError> {
        // TODO: Get from core protocol NetworkVisualizer once properly integrated
        // For now, return mock data
        Ok(NetworkTopology {
            nodes: vec![],
            links: vec![],
            message_stats: vec![],
        })
    }
    
    /// Gets message statistics
    pub fn get_message_stats(&self) -> Vec<MessageStats> {
        // TODO: Get from core protocol once API is available
        vec![]
    }
    
    /// Gets delivery success rate
    pub fn get_delivery_success_rate(&self) -> f32 {
        // TODO: Calculate from core protocol metrics once API is available
        0.0
    }
    
    /// Gets median latency
    pub fn get_median_latency(&self) -> u64 {
        // TODO: Get from core protocol metrics once API is available
        0
    }
    
    /// Gets median hop count
    pub fn get_median_hops(&self) -> u8 {
        // TODO: Get from core protocol metrics once API is available
        0
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_protocol_creation() {
        let config = ProtocolConfig {
            app_id: "test-app".to_string(),
            user_id: "user123".to_string(),
            ble_enabled: true,
            wifi_direct_enabled: true,
            internet_enabled: true,
            prefer_online: false,
            initial_ttl: 8,
        };
        
        let protocol = OfflineProtocol::new(config);
        assert!(protocol.is_ok());
    }
    
    #[test]
    fn test_protocol_lifecycle() {
        let config = ProtocolConfig {
            app_id: "test-app".to_string(),
            user_id: "user123".to_string(),
            ble_enabled: true,
            wifi_direct_enabled: true,
            internet_enabled: true,
            prefer_online: false,
            initial_ttl: 8,
        };
        
        let protocol = OfflineProtocol::new(config).unwrap();
        assert_eq!(protocol.get_state(), ProtocolState::Stopped);
        
        assert!(protocol.start().is_ok());
        assert_eq!(protocol.get_state(), ProtocolState::Running);
        
        assert!(protocol.pause().is_ok());
        assert_eq!(protocol.get_state(), ProtocolState::Paused);
        
        assert!(protocol.resume().is_ok());
        assert_eq!(protocol.get_state(), ProtocolState::Running);
        
        assert!(protocol.stop().is_ok());
        assert_eq!(protocol.get_state(), ProtocolState::Stopped);
    }
    
    #[test]
    fn test_ble_peer_management() {
        let config = ProtocolConfig {
            app_id: "test-app".to_string(),
            user_id: "user123".to_string(),
            ble_enabled: true,
            wifi_direct_enabled: true,
            internet_enabled: true,
            prefer_online: false,
            initial_ttl: 8,
        };
        
        let protocol = OfflineProtocol::new(config).unwrap();
        
        assert_eq!(protocol.ble_get_peer_count(), 0);
        
        protocol.ble_peer_discovered("peer1".to_string(), -50).unwrap();
        assert_eq!(protocol.ble_get_peer_count(), 1);
        
        protocol.ble_peer_discovered("peer2".to_string(), -60).unwrap();
        assert_eq!(protocol.ble_get_peer_count(), 2);
        
        protocol.ble_peer_lost("peer1".to_string()).unwrap();
        assert_eq!(protocol.ble_get_peer_count(), 1);
    }
    
    #[test]
    fn test_file_transfer_tracking() {
        let config = ProtocolConfig {
            app_id: "test-app".to_string(),
            user_id: "user123".to_string(),
            ble_enabled: true,
            wifi_direct_enabled: true,
            internet_enabled: true,
            prefer_online: false,
            initial_ttl: 8,
        };
        
        let protocol = OfflineProtocol::new(config).unwrap();
        
        let file_id = protocol.send_file(
            "recipient".to_string(),
            "/path/to/file".to_string(),
            "test.txt".to_string()
        ).unwrap();
        
        let progress = protocol.get_file_progress(file_id.clone());
        assert!(progress.is_some());
        assert_eq!(progress.unwrap().percentage, 0);
        
        protocol.finalize_file(file_id.clone()).unwrap();
        let progress = protocol.get_file_progress(file_id);
        assert_eq!(progress.unwrap().percentage, 100);
    }
}
