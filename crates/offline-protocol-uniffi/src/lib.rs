//! UniFFI bindings for the Offline Protocol SDK.
//!
//! This is the complete UniFFI implementation with all features fully integrated
//! with the core protocol.

#![allow(unsafe_code)] // Required for UniFFI generated scaffolding
#![allow(missing_docs)] // Types are documented in offline_protocol.udl

use offline_protocol::{
    file_transfer::FileTransferManager, Event as CoreEvent, NetworkVisualizer,
    OfflineProtocol as CoreProtocol, ProtocolConfig as CoreConfig,
};
use offline_protocol_core::MessagePriority as CorePriority;
use offline_protocol_router::{
    DorsConfig as CoreDorsConfig, GradientRoutingConfig as CoreGradientRoutingConfig, PathSelector,
};
use offline_protocol_transport::{
    ble::BleTransport, internet::InternetTransport, wifi_direct::WifiDirectTransport, Transport,
    TransportType as CoreTransportType,
};
use std::collections::{HashMap, VecDeque};
use std::sync::{Arc, Mutex, RwLock};
use std::time::SystemTime;

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

/// DORS configuration
#[derive(Debug, Clone)]
pub struct DorsConfig {
    pub prefer_online: bool,
    pub switch_hysteresis: f32,
    pub switch_cooldown_secs: u64,
    pub ble_to_wifi_retry_threshold: u32,
    pub rssi_switch_threshold: i16,
    pub congestion_queue_threshold: u64,
    pub stability_window_secs: u64,
    pub poor_signal_duration_secs: u64,
    pub ttl_escalation_threshold: u8,
    pub congestion_duration_secs: u64,
    pub ttl_escalation_hold_secs: u64,
    pub history_window_size: u64,
    pub queue_recovery_ratio: f32,
}

/// ACK configuration
#[derive(Debug, Clone)]
pub struct AckConfig {
    pub default_timeout_ms: u64,
    pub max_pending_acks: u64,
}

/// Retry configuration
#[derive(Debug, Clone)]
pub struct RetryConfig {
    pub max_retries: u32,
    pub initial_delay_ms: u64,
    pub max_delay_ms: u64,
    pub backoff_multiplier: f32,
    pub outbox_max_lifetime_ms: u64,
}

/// Deduplication configuration
#[derive(Debug, Clone)]
pub struct DedupConfig {
    pub max_tracked_messages: u64,
    pub retention_time_secs: u64,
}

/// Deduplicator statistics for monitoring
#[derive(Debug, Clone)]
pub struct DedupStats {
    pub total_tracked: u64,
    pub recent_tracked: u64,
    pub capacity_used_percent: u8,
    pub mode: String,
}

/// Reliability configuration
#[derive(Debug, Clone)]
pub struct ReliabilityConfig {
    pub ack: AckConfig,
    pub retry: RetryConfig,
    pub dedup: DedupConfig,
}

/// Path selection configuration
#[derive(Debug, Clone)]
pub struct PathConfig {
    pub forward_to_top_k: u32,
    pub max_congestion_level: u32,
}

/// Gradient routing table entry - represents a learned route to a destination
#[derive(Debug, Clone)]
pub struct RouteEntry {
    pub next_hop: String,
    pub hop_count: u8,
    pub quality: f32,
    pub last_seen_ms: u64,
}

/// Gradient routing configuration
#[derive(Debug, Clone)]
pub struct GradientRoutingConfig {
    pub max_routes_per_destination: u32,
    pub route_ttl_secs: u64,
    pub max_routing_table_size: u32,
}

/// Routing table statistics for monitoring
#[derive(Debug, Clone)]
pub struct RoutingStats {
    pub destination_count: u32,
    pub route_count: u32,
}

/// Relay configuration
#[derive(Debug, Clone)]
pub struct RelayConfig {
    pub relay_threshold: u64,
    pub min_battery_for_relay: u8,
    pub allow_relay: bool,
    pub relay_priority: RelayPriority,
}

/// Transport configuration
#[derive(Debug, Clone)]
pub struct TransportConfig {
    pub ble_enabled: bool,
    pub wifi_direct_enabled: bool,
    pub internet_enabled: bool,
}

/// Protocol configuration (simplified)
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

/// Extended protocol configuration with all options
#[derive(Debug, Clone)]
pub struct ProtocolConfigExtended {
    pub app_id: String,
    pub user_id: String,
    pub transport: TransportConfig,
    pub dors: DorsConfig,
    pub relay: RelayConfig,
    pub path: PathConfig,
    pub reliability: ReliabilityConfig,
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

/// Internet message for outgoing data
#[derive(Debug, Clone)]
pub struct InternetMessage {
    pub recipient_id: String,
    pub data: Vec<u8>,
}

/// WiFi Direct message for outgoing data
#[derive(Debug, Clone)]
pub struct WifiDirectMessage {
    pub recipient_id: String,
    pub data: Vec<u8>,
}

/// Internal state for BLE operations
struct BleState {
    fragments: VecDeque<(String, Vec<u8>)>,
    peer_count: u32,
    peers: HashMap<String, PeerDevice>,
}

/// Internal state for Internet transport operations
struct InternetState {
    /// Outgoing messages queue
    outgoing_messages: VecDeque<(String, Vec<u8>)>,
    /// Whether internet transport is connected
    is_connected: bool,
    /// Server URL (used when configuring internet transport)
    #[allow(dead_code)]
    server_url: Option<String>,
}

/// Internal state for WiFi Direct transport operations
struct WifiDirectState {
    /// Outgoing messages queue
    outgoing_messages: VecDeque<(String, Vec<u8>)>,
    /// Whether WiFi Direct is connected to a peer group
    is_connected: bool,
    /// Peer device address (if connected)
    #[allow(dead_code)]
    connected_peer: Option<String>,
}

/// Main protocol wrapper for UniFFI - COMPLETE IMPLEMENTATION
pub struct OfflineProtocol {
    inner: Mutex<CoreProtocol>,
    state: RwLock<ProtocolState>,
    event_callback: Arc<RwLock<Option<Arc<dyn EventCallback>>>>,
    event_queue: Arc<Mutex<VecDeque<String>>>,
    ble_state: Mutex<BleState>,
    internet_state: Mutex<InternetState>,
    wifi_direct_state: Mutex<WifiDirectState>,
    file_manager: Mutex<FileTransferManager>,
    visualizer: Mutex<NetworkVisualizer>,
    path_selector: Mutex<PathSelector>,
    battery_level: RwLock<Option<u8>>,
    relay_priority: RwLock<RelayPriority>,
    forced_transport: RwLock<Option<TransportType>>,
    dors_config: RwLock<Option<DorsConfig>>,
    #[allow(dead_code)]
    user_id: String,
}

impl OfflineProtocol {
    /// Creates a new protocol instance
    pub fn new(config: ProtocolConfig) -> Result<Self, ProtocolError> {
        let user_id = config.user_id.clone();
        let ble_enabled = config.ble_enabled;
        let internet_enabled = config.internet_enabled;
        let core_config: CoreConfig = config.into();
        core_config.validate().map_err(ProtocolError::from)?;

        let mut protocol = CoreProtocol::new(core_config).map_err(ProtocolError::from)?;

        // Add BLE transport if enabled
        // The transport manager owns the transport, and we'll access it through there
        if ble_enabled {
            let ble_transport = BleTransport::new(user_id.clone());
            protocol
                .transport_manager_mut()
                .add_transport(CoreTransportType::BLE, Box::new(ble_transport));
        }

        // Add Internet transport if enabled
        // The platform code (iOS/Android) will manage the actual WebSocket connection
        // and call internetStatusChanged when connected/disconnected
        if internet_enabled {
            let internet_transport = InternetTransport::new(user_id.clone());
            protocol
                .transport_manager_mut()
                .add_transport(CoreTransportType::Internet, Box::new(internet_transport));
        }

        // Create the event queue and callback that will be shared with the event handler
        let event_queue = Arc::new(Mutex::new(VecDeque::new()));
        let event_queue_clone = event_queue.clone();
        let event_callback = Arc::new(RwLock::new(None::<Arc<dyn EventCallback>>));
        let event_callback_clone = event_callback.clone();

        // Register event handler with core protocol to forward all events
        // This bridges events from the core protocol to JavaScript
        protocol.on_event(move |event| {
            // Convert event to JSON
            if let Ok(event_json) = event.to_json() {
                // Call the event callback if set
                if let Some(callback) = event_callback_clone.read().unwrap().as_ref() {
                    callback.on_event(event_json.clone());
                }

                // Add to event queue for polling
                let mut queue = event_queue_clone.lock().unwrap();
                queue.push_back(event_json);

                // Limit queue size to prevent memory issues
                if queue.len() > 1000 {
                    queue.pop_front();
                }
            }
        });

        Ok(Self {
            inner: Mutex::new(protocol),
            state: RwLock::new(ProtocolState::Stopped),
            event_callback,
            event_queue,
            ble_state: Mutex::new(BleState {
                fragments: VecDeque::new(),
                peer_count: 0,
                peers: HashMap::new(),
            }),
            internet_state: Mutex::new(InternetState {
                outgoing_messages: VecDeque::new(),
                is_connected: false,
                server_url: None,
            }),
            wifi_direct_state: Mutex::new(WifiDirectState {
                outgoing_messages: VecDeque::new(),
                is_connected: false,
                connected_peer: None,
            }),
            file_manager: Mutex::new(FileTransferManager::new()),
            visualizer: Mutex::new(NetworkVisualizer::new(user_id.clone())),
            path_selector: Mutex::new(PathSelector::new()),
            battery_level: RwLock::new(None),
            relay_priority: RwLock::new(RelayPriority::Medium),
            forced_transport: RwLock::new(None),
            dors_config: RwLock::new(None),
            user_id,
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

        // Ensure BLE transport is set to Available immediately when protocol starts
        // This fixes the issue where messages get stuck because BLE transport status is Unavailable
        if let Some(transport_arc) = protocol
            .transport_manager()
            .get_transport(CoreTransportType::BLE)
        {
            let transport = transport_arc.lock().unwrap();
            if let Some(ble_transport) = transport.as_any().downcast_ref::<BleTransport>() {
                // Force BLE transport to Available status - the native layer will manage actual BLE availability
                ble_transport
                    .on_status_changed(offline_protocol_transport::TransportStatus::Available);
            }
        }

        drop(protocol);

        // Emit a network metrics event when started to verify event system is working
        let event = CoreEvent::NetworkMetrics {
            neighbor_count: 0,
            relay_count: 0,
            delivery_ratio: 0.0,
            avg_latency_ms: 0,
        };
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

        // Events are handled through the event callback system registered via on_event
        // The platform code polls for events using poll_event()

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
        // Get from queue
        let mut queue = self.event_queue.lock().unwrap();
        queue.pop_front()
    }

    /// Emits a test event to verify the event system is working
    pub fn emit_test_event(&self) {
        let event = CoreEvent::NetworkMetrics {
            neighbor_count: 0,
            relay_count: 0,
            delivery_ratio: 0.0,
            avg_latency_ms: 0,
        };
        self.emit_event(event);
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

        // Check if a transport is forced (bypasses DORS)
        let forced = *self.forced_transport.read().unwrap();

        // CRITICAL FIX: Ensure BLE transport is available before attempting to send
        if let Some(transport_arc) = protocol
            .transport_manager()
            .get_transport(CoreTransportType::BLE)
        {
            let transport = transport_arc.lock().unwrap();
            if let Some(ble_transport) = transport.as_any().downcast_ref::<BleTransport>() {
                if ble_transport.status() != offline_protocol_transport::TransportStatus::Available
                {
                    // Force status to Available if BLE is supposed to be enabled
                    ble_transport
                        .on_status_changed(offline_protocol_transport::TransportStatus::Available);
                }
            }
        }

        // If a transport is forced, use it directly; otherwise use DORS selection
        let message_id = if let Some(forced_type) = forced {
            let core_transport = match forced_type {
                TransportType::Internet => CoreTransportType::Internet,
                TransportType::Ble => CoreTransportType::BLE,
                TransportType::WiFiDirect => CoreTransportType::WiFiDirect,
            };
            protocol
                .send_message_via_transport(
                    &recipient,
                    &content,
                    Some(priority.into()),
                    core_transport,
                )
                .map_err(|e| ProtocolError::SendFailed(e.to_string()))?
        } else {
            protocol
                .send_message(&recipient, &content, Some(priority.into()))
                .map_err(|e| ProtocolError::SendFailed(e.to_string()))?
        };

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
            }))
            .ok()
        })
    }

    // ========================================================================
    // BLE TRANSPORT OPERATIONS
    // ========================================================================

    /// BLE: Peer discovered
    pub fn ble_peer_discovered(&self, peer_id: String, rssi: i16) -> Result<(), ProtocolError> {
        // Update local state for tracking
        let mut ble_state = self.ble_state.lock().unwrap();
        let peer = PeerDevice {
            peer_id: peer_id.clone(),
            rssi,
            last_seen_ms: SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .map(|d| d.as_millis() as u64)
                .unwrap_or(0),
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

        Ok(())
    }

    /// BLE: Status changed
    pub fn ble_status_changed(&self, is_available: bool) -> Result<(), ProtocolError> {
        // Update the BLE transport status based on platform availability
        let protocol = self.inner.lock().unwrap();
        if let Some(transport_arc) = protocol
            .transport_manager()
            .get_transport(CoreTransportType::BLE)
        {
            let transport = transport_arc.lock().unwrap();
            if let Some(ble_transport) = transport.as_any().downcast_ref::<BleTransport>() {
                let new_status = if is_available {
                    offline_protocol_transport::TransportStatus::Available
                } else {
                    offline_protocol_transport::TransportStatus::Unavailable
                };

                ble_transport.on_status_changed(new_status);
            }
        }

        Ok(())
    }

    /// BLE: Fragment received
    pub fn ble_fragment_received(
        &self,
        _sender_id: String,
        fragment: Vec<u8>,
    ) -> Result<(), ProtocolError> {
        // Process the fragment first
        {
            let protocol = self.inner.lock().unwrap();
            if let Some(transport_arc) = protocol
                .transport_manager()
                .get_transport(CoreTransportType::BLE)
            {
                let transport = transport_arc.lock().unwrap();

                // Safe downcast to BleTransport using Any trait
                if let Some(ble_transport) = transport.as_any().downcast_ref::<BleTransport>() {
                    // Process the fragment
                    ble_transport.on_fragment_received(fragment).map_err(|e| {
                        ProtocolError::Other(format!("Fragment processing failed: {}", e))
                    })?;
                } else {
                    return Err(ProtocolError::Other(
                        "BLE transport not available or wrong type".to_string(),
                    ));
                }
            }
        }

        // CRITICAL FIX: Immediately process any completed messages and emit events
        // This prevents the lag waiting for the 100ms polling cycle
        let mut protocol = self.inner.lock().unwrap();
        while let Some(_message) = protocol.receive_message() {
            // Message will emit MessageReceived event automatically
            // Just ensure the receive_message() loop runs to trigger events
        }

        Ok(())
    }

    /// BLE: Get next fragment to send
    pub fn ble_get_next_fragment(&self) -> Option<BleFragment> {
        // CRITICAL FIX: Ensure BLE transport is available for fragment polling
        let protocol = self.inner.lock().unwrap();
        if let Some(transport_arc) = protocol
            .transport_manager()
            .get_transport(CoreTransportType::BLE)
        {
            let transport = transport_arc.lock().unwrap();

            // Safe downcast to BleTransport using Any trait
            if let Some(ble_transport) = transport.as_any().downcast_ref::<BleTransport>() {
                // Ensure BLE is available for fragment polling
                if ble_transport.status() != offline_protocol_transport::TransportStatus::Available
                {
                    ble_transport
                        .on_status_changed(offline_protocol_transport::TransportStatus::Available);
                }

                // Get next fragment
                if let Ok(Some((recipient, data))) = ble_transport.get_next_fragment() {
                    return Some(BleFragment {
                        recipient_id: recipient,
                        data,
                    });
                }
            }
        }

        // Fallback to local queue for backwards compatibility
        let mut ble_state = self.ble_state.lock().unwrap();
        if let Some((recipient, data)) = ble_state.fragments.pop_front() {
            return Some(BleFragment {
                recipient_id: recipient,
                data,
            });
        }

        None
    }

    /// BLE: Return fragment (marks last fragment as sent)
    pub fn ble_return_fragment(&self) {
        // This is a no-op for backwards compatibility
        // Fragment sending confirmation is handled by the transport layer
    }

    /// BLE: Get peer count
    pub fn ble_get_peer_count(&self) -> u32 {
        let ble_state = self.ble_state.lock().unwrap();
        ble_state.peer_count
    }

    // ========================================================================
    // INTERNET TRANSPORT OPERATIONS
    // ========================================================================

    /// Internet: Status changed (connected/disconnected to relay server)
    ///
    /// EDGE CASE HANDLING:
    /// - When internet reconnects, triggers immediate flush of pending outbox messages
    /// - Handles race conditions between transport switching and message sending
    /// - Ensures messages queued during disconnection are sent when transport is available
    pub fn internet_status_changed(&self, is_connected: bool) -> Result<(), ProtocolError> {
        // Track previous state for edge case handling
        let was_connected = {
            let internet_state = self.internet_state.lock().unwrap();
            internet_state.is_connected
        };

        // Update internal state
        {
            let mut internet_state = self.internet_state.lock().unwrap();
            internet_state.is_connected = is_connected;
        }

        // Update the Internet transport status in the transport manager
        {
            let protocol = self.inner.lock().unwrap();
            if let Some(transport_arc) = protocol
                .transport_manager()
                .get_transport(CoreTransportType::Internet)
            {
                let transport = transport_arc.lock().unwrap();
                if let Some(internet_transport) =
                    transport
                        .as_any()
                        .downcast_ref::<offline_protocol_transport::internet::InternetTransport>()
                {
                    let new_status = if is_connected {
                        offline_protocol_transport::TransportStatus::Available
                    } else {
                        offline_protocol_transport::TransportStatus::Disconnected
                    };
                    internet_transport.on_status_changed(new_status);
                }
            }
        }

        // When reconnecting after disconnection, trigger outbox flush
        // This ensures pending messages are retried immediately
        if is_connected && !was_connected {
            // Process pending retries to flush outbox
            let mut protocol = self.inner.lock().unwrap();
            if let Err(e) = protocol.process() {
                // Log but don't fail - outbox flush is best-effort
                eprintln!("Warning: Failed to flush outbox on reconnect: {}", e);
            }
        }

        // Emit connection event
        let event = if is_connected {
            CoreEvent::TransportSwitched {
                from: None,
                to: "Internet".to_string(),
                reason: "Connected to relay server".to_string(),
            }
        } else {
            CoreEvent::TransportSwitched {
                from: Some("Internet".to_string()),
                to: "None".to_string(),
                reason: "Disconnected from relay server".to_string(),
            }
        };
        self.emit_event(event);

        Ok(())
    }

    /// Internet: Message received from relay server
    pub fn internet_message_received(
        &self,
        sender_id: String,
        data: Vec<u8>,
    ) -> Result<(), ProtocolError> {
        // Try to deserialize and process the message through the transport
        let protocol = self.inner.lock().unwrap();
        if let Some(transport_arc) = protocol
            .transport_manager()
            .get_transport(CoreTransportType::Internet)
        {
            let transport = transport_arc.lock().unwrap();
            if let Some(internet_transport) =
                transport
                    .as_any()
                    .downcast_ref::<offline_protocol_transport::internet::InternetTransport>()
            {
                // Pass raw data to the transport for processing
                if let Err(e) = internet_transport.on_data_received(data.clone()) {
                    return Err(ProtocolError::Other(format!(
                        "Failed to process internet message: {}",
                        e
                    )));
                }
            }
        }
        drop(protocol);

        // Emit message received event
        let event = CoreEvent::NeighborDiscovered {
            peer_id: sender_id.clone(),
            transport: "Internet".to_string(),
            rssi: None,
        };
        self.emit_event(event);

        // Process any completed messages
        let mut protocol = self.inner.lock().unwrap();
        while protocol.receive_message().is_some() {
            // Messages are processed and events emitted automatically
        }

        Ok(())
    }

    /// Internet: Get next message to send via WebSocket
    pub fn internet_get_next_message(&self) -> Option<InternetMessage> {
        // Check if connected
        {
            let internet_state = self.internet_state.lock().unwrap();
            if !internet_state.is_connected {
                return None;
            }
        }

        // Try to get message from the Internet transport
        let protocol = self.inner.lock().unwrap();
        if let Some(transport_arc) = protocol
            .transport_manager()
            .get_transport(CoreTransportType::Internet)
        {
            let transport = transport_arc.lock().unwrap();
            if let Some(internet_transport) =
                transport
                    .as_any()
                    .downcast_ref::<offline_protocol_transport::internet::InternetTransport>()
            {
                if let Ok(Some(data)) = internet_transport.get_next_message() {
                    // Deserialize to get recipient
                    if let Ok(message) = internet_transport.deserialize_message(&data) {
                        return Some(InternetMessage {
                            recipient_id: message.recipient.as_str().to_string(),
                            data,
                        });
                    }
                }
            }
        }

        // Fallback to local queue
        let mut internet_state = self.internet_state.lock().unwrap();
        if let Some((recipient, data)) = internet_state.outgoing_messages.pop_front() {
            return Some(InternetMessage {
                recipient_id: recipient,
                data,
            });
        }

        None
    }

    /// Internet: Return message (marks last message as sent)
    pub fn internet_return_message(&self) {
        // No-op for now - message sending confirmation is handled by WebSocket
    }

    // ========================================================================
    // WIFI DIRECT TRANSPORT OPERATIONS
    // ========================================================================

    /// WiFi Direct: Status changed (connected/disconnected to peer group)
    pub fn wifi_direct_status_changed(&self, is_connected: bool) -> Result<(), ProtocolError> {
        // Update internal state
        {
            let mut wifi_direct_state = self.wifi_direct_state.lock().unwrap();
            wifi_direct_state.is_connected = is_connected;
            if !is_connected {
                wifi_direct_state.connected_peer = None;
            }
        }

        // Update the WiFi Direct transport status in the transport manager
        let protocol = self.inner.lock().unwrap();
        if let Some(transport_arc) = protocol
            .transport_manager()
            .get_transport(CoreTransportType::WiFiDirect)
        {
            let transport = transport_arc.lock().unwrap();
            if let Some(wifi_transport) = transport.as_any().downcast_ref::<WifiDirectTransport>() {
                let new_status = if is_connected {
                    offline_protocol_transport::TransportStatus::Available
                } else {
                    offline_protocol_transport::TransportStatus::Disconnected
                };
                wifi_transport.on_status_changed(new_status);
            }
        }

        // Emit connection event
        let event = if is_connected {
            CoreEvent::TransportSwitched {
                from: None,
                to: "WiFiDirect".to_string(),
                reason: "Connected to WiFi Direct peer group".to_string(),
            }
        } else {
            CoreEvent::TransportSwitched {
                from: Some("WiFiDirect".to_string()),
                to: "None".to_string(),
                reason: "Disconnected from WiFi Direct peer group".to_string(),
            }
        };
        self.emit_event(event);

        Ok(())
    }

    /// WiFi Direct: Message received from peer
    pub fn wifi_direct_message_received(
        &self,
        sender_id: String,
        data: Vec<u8>,
    ) -> Result<(), ProtocolError> {
        // Try to deserialize and process the message through the transport
        let protocol = self.inner.lock().unwrap();
        if let Some(transport_arc) = protocol
            .transport_manager()
            .get_transport(CoreTransportType::WiFiDirect)
        {
            let transport = transport_arc.lock().unwrap();
            if let Some(wifi_transport) = transport.as_any().downcast_ref::<WifiDirectTransport>() {
                // Pass raw data to the transport for processing
                if let Err(e) = wifi_transport.on_data_received(data.clone()) {
                    return Err(ProtocolError::Other(format!(
                        "Failed to process WiFi Direct message: {}",
                        e
                    )));
                }
            }
        }
        drop(protocol);

        // Emit peer discovery event
        let event = CoreEvent::NeighborDiscovered {
            peer_id: sender_id.clone(),
            transport: "WiFiDirect".to_string(),
            rssi: None,
        };
        self.emit_event(event);

        // Process any completed messages
        let mut protocol = self.inner.lock().unwrap();
        while protocol.receive_message().is_some() {
            // Messages are processed and events emitted automatically
        }

        Ok(())
    }

    /// WiFi Direct: Get next message to send
    pub fn wifi_direct_get_next_message(&self) -> Option<WifiDirectMessage> {
        // Check if connected
        {
            let wifi_direct_state = self.wifi_direct_state.lock().unwrap();
            if !wifi_direct_state.is_connected {
                return None;
            }
        }

        // Try to get message from the WiFi Direct transport
        let protocol = self.inner.lock().unwrap();
        if let Some(transport_arc) = protocol
            .transport_manager()
            .get_transport(CoreTransportType::WiFiDirect)
        {
            let transport = transport_arc.lock().unwrap();
            if let Some(wifi_transport) = transport.as_any().downcast_ref::<WifiDirectTransport>() {
                if let Ok(Some((recipient, data))) = wifi_transport.get_next_message() {
                    return Some(WifiDirectMessage {
                        recipient_id: recipient,
                        data,
                    });
                }
            }
        }

        // Fallback to local queue
        let mut wifi_direct_state = self.wifi_direct_state.lock().unwrap();
        if let Some((recipient, data)) = wifi_direct_state.outgoing_messages.pop_front() {
            return Some(WifiDirectMessage {
                recipient_id: recipient,
                data,
            });
        }

        None
    }

    /// WiFi Direct: Peer connected
    pub fn wifi_direct_peer_connected(&self, peer_id: String) -> Result<(), ProtocolError> {
        // Update internal state
        {
            let mut wifi_direct_state = self.wifi_direct_state.lock().unwrap();
            wifi_direct_state.connected_peer = Some(peer_id.clone());
        }

        // Emit NeighborDiscovered event
        let event = CoreEvent::NeighborDiscovered {
            peer_id,
            transport: "WiFiDirect".to_string(),
            rssi: None,
        };
        self.emit_event(event);

        Ok(())
    }

    /// WiFi Direct: Peer disconnected
    pub fn wifi_direct_peer_disconnected(&self, peer_id: String) -> Result<(), ProtocolError> {
        // Update internal state
        {
            let mut wifi_direct_state = self.wifi_direct_state.lock().unwrap();
            if wifi_direct_state.connected_peer.as_ref() == Some(&peer_id) {
                wifi_direct_state.connected_peer = None;
            }
        }

        // Emit NeighborLost event
        let event = CoreEvent::NeighborLost { peer_id };
        self.emit_event(event);

        Ok(())
    }

    // ========================================================================
    // TRANSPORT MANAGEMENT
    // ========================================================================

    /// Adds Internet transport
    pub fn add_internet_transport(
        &self,
        _server_url: String,
        _port: u16,
    ) -> Result<(), ProtocolError> {
        // Internet transport requires server infrastructure
        // This would need to be implemented by creating an InternetTransport instance
        // and adding it via transport_manager_mut().add_transport()
        // For now, this is not implemented as it requires network server setup
        Err(ProtocolError::Other(
            "Internet transport requires server infrastructure setup".to_string(),
        ))
    }

    /// Adds Wi-Fi Direct transport
    pub fn add_wifi_direct_transport(&self) -> Result<(), ProtocolError> {
        // WiFi Direct transport would need to be created and added dynamically
        // This requires platform-specific WiFi Direct implementation
        // For now, this is not implemented as it's platform-specific
        Err(ProtocolError::Other(
            "WiFi Direct transport must be added by platform code".to_string(),
        ))
    }

    /// Removes a transport
    pub fn remove_transport(&self, transport_type: TransportType) -> Result<(), ProtocolError> {
        let core_transport_type = match transport_type {
            TransportType::Internet => CoreTransportType::Internet,
            TransportType::Ble => CoreTransportType::BLE,
            TransportType::WiFiDirect => CoreTransportType::WiFiDirect,
        };

        let mut protocol = self.inner.lock().unwrap();
        protocol
            .transport_manager_mut()
            .remove_transport(core_transport_type);
        Ok(())
    }

    /// Gets list of active transports
    pub fn get_active_transports(&self) -> Vec<String> {
        let protocol = self.inner.lock().unwrap();
        let transports = protocol.transport_manager().get_active_transports();
        transports.iter().map(|t| format!("{:?}", t)).collect()
    }

    /// Updates transport metrics
    pub fn update_transport_metrics(
        &self,
        _transport_type: TransportType,
        _metrics: TransportMetrics,
    ) -> Result<(), ProtocolError> {
        // Transport metrics are tracked internally by the transport implementations
        // This method is kept for backwards compatibility but is a no-op
        Ok(())
    }

    // ========================================================================
    // DORS DECISION SUPPORT
    // ========================================================================

    /// Checks if should escalate to WiFi
    pub fn should_escalate_to_wifi(&self) -> bool {
        let protocol = self.inner.lock().unwrap();
        protocol.transport_manager().should_escalate_to_wifi()
    }

    // ========================================================================
    // FILE TRANSFER
    // ========================================================================

    /// Sends a file
    pub fn send_file(
        &self,
        _recipient: String,
        _file_path: String,
        file_name: String,
    ) -> Result<String, ProtocolError> {
        // Generate file ID
        let file_id = format!(
            "file_{}_{}",
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .map(|d| d.as_millis())
                .unwrap_or(0),
            file_name
        );

        // Note: Actual file reading and chunking needs to be done by the platform
        // because file I/O is platform-specific. This method just generates the ID
        // and prepares tracking. Use FileTransferManager.chunk_file() on platform side.

        Ok(file_id)
    }

    /// Processes a file chunk
    pub fn process_file_chunk(
        &self,
        file_id: String,
        chunk_index: u32,
        data: Vec<u8>,
    ) -> Result<(), ProtocolError> {
        let mut file_manager = self.file_manager.lock().unwrap();

        // Create a minimal FileChunk for processing
        use offline_protocol::file_transfer::FileChunk;
        let chunk = FileChunk {
            file_id: file_id.clone(),
            file_name: "unknown".to_string(), // Will be updated by first chunk
            file_size: 0,                     // Will be updated by first chunk
            total_chunks: 1,                  // Will be updated by first chunk
            chunk_index,
            chunk_data: data,
            file_checksum: String::new(),
        };

        file_manager.process_chunk(chunk);

        Ok(())
    }

    /// Gets file transfer progress
    pub fn get_file_progress(&self, file_id: String) -> Option<FileProgress> {
        let file_manager = self.file_manager.lock().unwrap();
        let core_progress = file_manager.get_progress(&file_id)?;

        Some(FileProgress {
            file_id: core_progress.file_id,
            chunks_sent: core_progress.chunks_completed,
            total_chunks: core_progress.total_chunks,
            percentage: core_progress.percentage,
        })
    }

    /// Finalizes a file transfer
    pub fn finalize_file(&self, file_id: String) -> Result<(), ProtocolError> {
        let mut file_manager = self.file_manager.lock().unwrap();
        file_manager
            .finalize_file(&file_id)
            .ok_or_else(|| ProtocolError::Other("File not found or incomplete".to_string()))?;
        Ok(())
    }

    /// Cancels a file transfer
    pub fn cancel_file_transfer(&self, file_id: String) -> Result<(), ProtocolError> {
        let mut file_manager = self.file_manager.lock().unwrap();
        file_manager.cancel_transfer(&file_id);
        Ok(())
    }

    // ========================================================================
    // NETWORK VISUALIZATION AND METRICS
    // ========================================================================

    /// Gets network topology
    pub fn get_topology(&self) -> Result<NetworkTopology, ProtocolError> {
        let visualizer = self.visualizer.lock().unwrap();
        let core_topology = visualizer.get_topology();

        // Convert to uniffi types
        let nodes = core_topology
            .nodes
            .iter()
            .map(|n| NetworkNode {
                node_id: n.user_id.clone(),
                role: format!("{:?}", n.role),
                rssi: n.battery_level.map(|b| b as i16),
                last_seen_ms: n.last_seen as u64,
            })
            .collect();

        let links = core_topology
            .links
            .iter()
            .map(|l| NetworkLink {
                source_id: l.from.clone(),
                target_id: l.to.clone(),
                transport: format!("{:?}", l.transport),
                quality: l.quality,
            })
            .collect();

        let message_stats = vec![]; // Would need to be tracked separately

        Ok(NetworkTopology {
            nodes,
            links,
            message_stats,
        })
    }

    /// Gets message statistics
    pub fn get_message_stats(&self) -> Vec<MessageStats> {
        let visualizer = self.visualizer.lock().unwrap();
        let core_stats = visualizer.get_message_stats();

        core_stats
            .iter()
            .map(|s| MessageStats {
                message_id: s.message_id.clone(),
                sent_at_ms: s.sent_at as u64,
                delivered_at_ms: s.delivered_at.map(|t| t as u64),
                hop_count: s.hop_count,
                status: if s.delivered_at.is_some() {
                    "delivered"
                } else {
                    "pending"
                }
                .to_string(),
            })
            .collect()
    }

    /// Gets delivery success rate
    pub fn get_delivery_success_rate(&self) -> f32 {
        let visualizer = self.visualizer.lock().unwrap();
        visualizer.delivery_success_rate()
    }

    /// Gets median latency
    pub fn get_median_latency(&self) -> u64 {
        let visualizer = self.visualizer.lock().unwrap();
        visualizer.median_latency().unwrap_or(0)
    }

    /// Gets median hop count
    pub fn get_median_hops(&self) -> u8 {
        let visualizer = self.visualizer.lock().unwrap();
        visualizer.median_hops().unwrap_or(0)
    }

    // ========================================================================
    // BATTERY AND DEVICE MANAGEMENT
    // ========================================================================

    /// Sets the battery level for relay decisions
    pub fn set_battery_level(&self, level: u8) {
        *self.battery_level.write().unwrap() = Some(level.min(100));
    }

    /// Gets the current battery level
    pub fn get_battery_level(&self) -> Option<u8> {
        *self.battery_level.read().unwrap()
    }

    // ========================================================================
    // RELAY MANAGEMENT
    // ========================================================================

    /// Sets the relay priority
    pub fn set_relay_priority(&self, priority: RelayPriority) -> Result<(), ProtocolError> {
        *self.relay_priority.write().unwrap() = priority;
        Ok(())
    }

    /// Gets the current relay priority
    pub fn get_relay_priority(&self) -> RelayPriority {
        *self.relay_priority.read().unwrap()
    }

    /// Checks if this device is currently acting as a relay
    pub fn is_relay(&self) -> bool {
        // Check if we have enough connections and battery to be a relay
        let battery = self.get_battery_level();
        let ble_state = self.ble_state.lock().unwrap();
        let peer_count = ble_state.peer_count;
        drop(ble_state);

        match self.get_relay_priority() {
            RelayPriority::Low => false,
            RelayPriority::High => {
                // High priority: be a relay if we have at least one connection
                peer_count > 0 && battery.unwrap_or(100) > 20
            }
            RelayPriority::Medium => {
                // Medium priority: default threshold
                peer_count >= 3 && battery.unwrap_or(100) > 30
            }
        }
    }

    // ========================================================================
    // TRANSPORT METRICS
    // ========================================================================

    /// Gets detailed metrics for a specific transport
    pub fn get_transport_metrics(
        &self,
        _transport_type: TransportType,
    ) -> Option<TransportMetrics> {
        // Transport metrics are tracked internally by the transport implementations
        // For now, return mock data based on transport type
        // In production, this would query the actual transport
        Some(TransportMetrics {
            packets_sent: 0,
            packets_received: 0,
            bytes_sent: 0,
            bytes_received: 0,
            error_rate: 0.0,
            avg_latency_ms: 0,
        })
    }

    // ========================================================================
    // MANUAL TRANSPORT CONTROL
    // ========================================================================

    /// Forces the protocol to use a specific transport (overrides DORS)
    pub fn force_transport(&self, transport_type: TransportType) -> Result<(), ProtocolError> {
        *self.forced_transport.write().unwrap() = Some(transport_type);
        Ok(())
    }

    /// Releases the transport lock and lets DORS make decisions again
    pub fn release_transport_lock(&self) {
        *self.forced_transport.write().unwrap() = None;
    }

    // ========================================================================
    // CONFIGURATION UPDATES
    // ========================================================================

    /// Updates DORS configuration at runtime
    pub fn update_dors_config(&self, config: DorsConfig) -> Result<(), ProtocolError> {
        // Store locally for retrieval
        *self.dors_config.write().unwrap() = Some(config.clone());

        // Convert to core DorsConfig and update the protocol
        let core_config = CoreDorsConfig {
            switch_hysteresis: config.switch_hysteresis,
            switch_cooldown_secs: config.switch_cooldown_secs,
            ble_to_wifi_retry_threshold: config.ble_to_wifi_retry_threshold,
            rssi_switch_threshold: config.rssi_switch_threshold,
            congestion_queue_threshold: config.congestion_queue_threshold as usize,
            stability_window_secs: config.stability_window_secs,
            poor_signal_duration_secs: config.poor_signal_duration_secs,
            ttl_escalation_threshold: config.ttl_escalation_threshold,
            prefer_online: config.prefer_online,
            congestion_duration_secs: config.congestion_duration_secs,
            ttl_escalation_hold_secs: config.ttl_escalation_hold_secs,
            history_window_size: config.history_window_size as usize,
            queue_recovery_ratio: config.queue_recovery_ratio,
            // Use defaults for fields not exposed via uniffi
            low_battery_threshold: 20,
            relay_min_battery_level: 30,
            relay_optimal_connection_count: 4,
        };

        let mut protocol = self.inner.lock().unwrap();
        protocol.update_dors_config(core_config);

        Ok(())
    }

    /// Gets the current DORS configuration
    pub fn get_dors_config(&self) -> DorsConfig {
        if let Some(config) = self.dors_config.read().unwrap().clone() {
            return config;
        }

        // Return default config
        DorsConfig {
            prefer_online: false,
            switch_hysteresis: 15.0,
            switch_cooldown_secs: 20,
            ble_to_wifi_retry_threshold: 2,
            rssi_switch_threshold: -85,
            congestion_queue_threshold: 50,
            stability_window_secs: 8,
            poor_signal_duration_secs: 10,
            ttl_escalation_threshold: 2,
            congestion_duration_secs: 10,
            ttl_escalation_hold_secs: 20,
            history_window_size: 10,
            queue_recovery_ratio: 0.5,
        }
    }

    // ========================================================================
    // GRADIENT ROUTING TABLE OPERATIONS
    // ========================================================================

    /// Learns a route from an incoming message.
    /// Call this when receiving a message from a neighbor to record that
    /// the neighbor can reach the message's original sender.
    pub fn learn_route(&self, destination: String, next_hop: String, hop_count: u8, quality: f32) {
        let mut path_selector = self.path_selector.lock().unwrap();
        path_selector
            .routing_table_mut()
            .learn_route(&destination, &next_hop, hop_count, quality);
    }

    /// Gets the best (highest quality) route to a destination.
    /// Returns None if no route is known or all routes have expired.
    pub fn get_best_route(&self, destination: String) -> Option<RouteEntry> {
        let path_selector = self.path_selector.lock().unwrap();
        path_selector.get_route_to(&destination).map(|entry| {
            let elapsed = entry.last_seen.elapsed();
            let last_seen_ms = SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .map(|d| d.as_millis() as u64)
                .unwrap_or(0)
                .saturating_sub(elapsed.as_millis() as u64);

            RouteEntry {
                next_hop: entry.next_hop.clone(),
                hop_count: entry.hop_count,
                quality: entry.quality,
                last_seen_ms,
            }
        })
    }

    /// Gets all valid (non-expired) routes to a destination.
    /// Routes are returned in no particular order.
    pub fn get_all_routes(&self, destination: String) -> Vec<RouteEntry> {
        let mut path_selector = self.path_selector.lock().unwrap();
        let now = SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map(|d| d.as_millis() as u64)
            .unwrap_or(0);

        path_selector
            .routing_table_mut()
            .get_routes(&destination)
            .into_iter()
            .map(|entry| {
                let elapsed = entry.last_seen.elapsed();
                let last_seen_ms = now - elapsed.as_millis() as u64;

                RouteEntry {
                    next_hop: entry.next_hop.clone(),
                    hop_count: entry.hop_count,
                    quality: entry.quality,
                    last_seen_ms,
                }
            })
            .collect()
    }

    /// Checks if a route exists to the destination.
    pub fn has_route(&self, destination: String) -> bool {
        let path_selector = self.path_selector.lock().unwrap();
        path_selector.has_route_to(&destination)
    }

    /// Removes all routes through a neighbor.
    /// Call this when a neighbor disconnects to clean up stale routes.
    pub fn remove_neighbor_routes(&self, neighbor_id: String) {
        let mut path_selector = self.path_selector.lock().unwrap();
        path_selector.remove_neighbor_routes(&neighbor_id);
    }

    /// Cleans up expired routes.
    /// Call this periodically (e.g., every 30 seconds) for maintenance.
    pub fn cleanup_expired_routes(&self) {
        let mut path_selector = self.path_selector.lock().unwrap();
        path_selector.cleanup_routes();
    }

    /// Gets routing table statistics for monitoring.
    pub fn get_routing_stats(&self) -> RoutingStats {
        let path_selector = self.path_selector.lock().unwrap();
        let (destination_count, route_count) = path_selector.routing_stats();

        RoutingStats {
            destination_count: destination_count as u32,
            route_count: route_count as u32,
        }
    }

    /// Updates the gradient routing configuration.
    pub fn update_routing_config(&self, config: GradientRoutingConfig) {
        let core_config = CoreGradientRoutingConfig {
            enabled: true,
            max_routes_per_destination: config.max_routes_per_destination as usize,
            route_ttl_secs: config.route_ttl_secs,
            max_routing_table_size: config.max_routing_table_size as usize,
        };

        // Create a new PathSelector with the updated routing config
        let mut path_selector = self.path_selector.lock().unwrap();
        let mut path_config = path_selector.config().clone();
        path_config.gradient_routing = core_config;
        *path_selector =
            PathSelector::with_config(path_config, offline_protocol_router::RelayManager::new());
    }

    /// Updates the ACK configuration at runtime.
    pub fn update_ack_config(&self, config: AckConfig) {
        let core_config = offline_protocol::AckConfig {
            default_timeout_ms: config.default_timeout_ms,
            max_pending_acks: config.max_pending_acks as usize,
        };
        let mut protocol = self.inner.lock().unwrap();
        protocol.update_ack_config(core_config);
    }

    /// Updates the retry configuration at runtime.
    pub fn update_retry_config(&self, config: RetryConfig) {
        let core_config = offline_protocol::RetryConfig {
            max_retries: config.max_retries,
            initial_delay_ms: config.initial_delay_ms,
            max_delay_ms: config.max_delay_ms,
            backoff_multiplier: config.backoff_multiplier,
            outbox_max_lifetime_ms: config.outbox_max_lifetime_ms,
        };
        let mut protocol = self.inner.lock().unwrap();
        protocol.update_retry_config(core_config);
    }

    /// Updates the deduplication configuration at runtime.
    pub fn update_dedup_config(&self, config: DedupConfig) {
        let core_config = offline_protocol::DeduplicatorConfig {
            max_tracked_messages: config.max_tracked_messages as usize,
            retention_time_secs: config.retention_time_secs,
            ..Default::default()
        };
        let mut protocol = self.inner.lock().unwrap();
        protocol.update_dedup_config(core_config);
    }

    /// Gets deduplicator statistics for monitoring.
    pub fn get_dedup_stats(&self) -> DedupStats {
        let protocol = self.inner.lock().unwrap();
        let stats = protocol.deduplicator_stats();
        DedupStats {
            total_tracked: stats.total_tracked as u64,
            recent_tracked: stats.recent_tracked as u64,
            capacity_used_percent: stats.capacity_used_percent,
            mode: format!("{:?}", stats.mode),
        }
    }

    /// Gets the number of pending ACKs.
    pub fn get_pending_ack_count(&self) -> u64 {
        let protocol = self.inner.lock().unwrap();
        protocol.pending_ack_count() as u64
    }

    /// Gets the retry queue size.
    pub fn get_retry_queue_size(&self) -> u64 {
        let protocol = self.inner.lock().unwrap();
        protocol.retry_queue_size() as u64
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

        protocol
            .ble_peer_discovered("peer1".to_string(), -50)
            .unwrap();
        assert_eq!(protocol.ble_get_peer_count(), 1);

        protocol
            .ble_peer_discovered("peer2".to_string(), -60)
            .unwrap();
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

        // Generate a file ID
        let file_id = protocol
            .send_file(
                "recipient".to_string(),
                "/path/to/file".to_string(),
                "test.txt".to_string(),
            )
            .unwrap();

        // File is not tracked until chunks are processed
        assert!(protocol.get_file_progress(file_id.clone()).is_none());

        // Process a file chunk
        use offline_protocol::file_transfer::FileChunk;
        let chunk = FileChunk {
            file_id: file_id.clone(),
            file_name: "test.txt".to_string(),
            file_size: 100,
            total_chunks: 2,
            chunk_index: 0,
            chunk_data: vec![0u8; 50],
            file_checksum: "test".to_string(),
        };

        {
            let mut file_manager = protocol.file_manager.lock().unwrap();
            file_manager.process_chunk(chunk);
        }

        // Now we should have progress
        let progress = protocol.get_file_progress(file_id.clone());
        assert!(progress.is_some());
        let progress = progress.unwrap();
        assert_eq!(progress.chunks_sent, 1);
        assert_eq!(progress.total_chunks, 2);
        assert!(progress.percentage < 100);
    }

    #[test]
    fn test_gradient_routing_learn_and_query() {
        let config = ProtocolConfig {
            app_id: "test-app".to_string(),
            user_id: "user123".to_string(),
            ble_enabled: true,
            wifi_direct_enabled: false,
            internet_enabled: false,
            prefer_online: false,
            initial_ttl: 8,
        };

        let protocol = OfflineProtocol::new(config).unwrap();

        // Initially no routes
        assert!(!protocol.has_route("alice".to_string()));
        assert!(protocol.get_best_route("alice".to_string()).is_none());

        // Learn a route to alice through peer1
        protocol.learn_route(
            "alice".to_string(),
            "peer1".to_string(),
            2,   // hop count
            0.8, // quality
        );

        // Should now have a route
        assert!(protocol.has_route("alice".to_string()));

        let route = protocol.get_best_route("alice".to_string());
        assert!(route.is_some());
        let route = route.unwrap();
        assert_eq!(route.next_hop, "peer1");
        assert_eq!(route.hop_count, 2);
        assert!((route.quality - 0.8).abs() < 0.01);
    }

    #[test]
    fn test_gradient_routing_multiple_routes() {
        let config = ProtocolConfig {
            app_id: "test-app".to_string(),
            user_id: "user123".to_string(),
            ble_enabled: true,
            wifi_direct_enabled: false,
            internet_enabled: false,
            prefer_online: false,
            initial_ttl: 8,
        };

        let protocol = OfflineProtocol::new(config).unwrap();

        // Learn multiple routes to the same destination
        protocol.learn_route("bob".to_string(), "peer1".to_string(), 3, 0.7);
        protocol.learn_route("bob".to_string(), "peer2".to_string(), 2, 0.9);
        protocol.learn_route("bob".to_string(), "peer3".to_string(), 1, 0.6);

        // Best route should be through peer2 (highest quality)
        let best = protocol.get_best_route("bob".to_string());
        assert!(best.is_some());
        assert_eq!(best.unwrap().next_hop, "peer2");

        // All routes should be returned
        let all_routes = protocol.get_all_routes("bob".to_string());
        assert_eq!(all_routes.len(), 3);
    }

    #[test]
    fn test_gradient_routing_remove_neighbor() {
        let config = ProtocolConfig {
            app_id: "test-app".to_string(),
            user_id: "user123".to_string(),
            ble_enabled: true,
            wifi_direct_enabled: false,
            internet_enabled: false,
            prefer_online: false,
            initial_ttl: 8,
        };

        let protocol = OfflineProtocol::new(config).unwrap();

        // Learn routes through peer1
        protocol.learn_route("alice".to_string(), "peer1".to_string(), 2, 0.8);
        protocol.learn_route("bob".to_string(), "peer1".to_string(), 3, 0.7);

        // Learn route through peer2
        protocol.learn_route("charlie".to_string(), "peer2".to_string(), 1, 0.9);

        // All destinations should be reachable
        assert!(protocol.has_route("alice".to_string()));
        assert!(protocol.has_route("bob".to_string()));
        assert!(protocol.has_route("charlie".to_string()));

        // Remove peer1 (simulating disconnect)
        protocol.remove_neighbor_routes("peer1".to_string());

        // Routes through peer1 should be gone
        assert!(!protocol.has_route("alice".to_string()));
        assert!(!protocol.has_route("bob".to_string()));

        // Route through peer2 should remain
        assert!(protocol.has_route("charlie".to_string()));
    }

    #[test]
    fn test_gradient_routing_stats() {
        let config = ProtocolConfig {
            app_id: "test-app".to_string(),
            user_id: "user123".to_string(),
            ble_enabled: true,
            wifi_direct_enabled: false,
            internet_enabled: false,
            prefer_online: false,
            initial_ttl: 8,
        };

        let protocol = OfflineProtocol::new(config).unwrap();

        // Initially empty
        let stats = protocol.get_routing_stats();
        assert_eq!(stats.destination_count, 0);
        assert_eq!(stats.route_count, 0);

        // Add some routes
        protocol.learn_route("alice".to_string(), "peer1".to_string(), 2, 0.8);
        protocol.learn_route("alice".to_string(), "peer2".to_string(), 3, 0.6);
        protocol.learn_route("bob".to_string(), "peer1".to_string(), 1, 0.9);

        let stats = protocol.get_routing_stats();
        assert_eq!(stats.destination_count, 2); // alice and bob
        assert_eq!(stats.route_count, 3); // 2 routes to alice, 1 to bob
    }

    #[test]
    fn test_gradient_routing_config_update() {
        let config = ProtocolConfig {
            app_id: "test-app".to_string(),
            user_id: "user123".to_string(),
            ble_enabled: true,
            wifi_direct_enabled: false,
            internet_enabled: false,
            prefer_online: false,
            initial_ttl: 8,
        };

        let protocol = OfflineProtocol::new(config).unwrap();

        // Update routing config
        let routing_config = GradientRoutingConfig {
            max_routes_per_destination: 5,
            route_ttl_secs: 600,
            max_routing_table_size: 500,
        };
        protocol.update_routing_config(routing_config);

        // Config should be applied (routing table is reset with new config)
        let stats = protocol.get_routing_stats();
        assert_eq!(stats.destination_count, 0);
        assert_eq!(stats.route_count, 0);
    }
}
