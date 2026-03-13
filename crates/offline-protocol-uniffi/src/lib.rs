//! UniFFI bindings for the Offline Protocol SDK.
//!
//! This is the complete UniFFI implementation with all features fully integrated
//! with the core protocol.

#![allow(unsafe_code)] // Required for UniFFI generated scaffolding
#![allow(missing_docs)] // Types are documented in offline_protocol.udl

use offline_protocol::{
    EstablishmentState as CoreEstablishmentState, Event as CoreEvent, NetworkVisualizer,
    OfflineProtocol as CoreProtocol, OverflowPolicy as CoreOverflowPolicy,
    PendingQueueConfig as CorePendingQueueConfig, ProtocolConfig as CoreConfig,
};
use offline_protocol_core::{
    ContentType as CoreContentType, MediaMetadata as CoreMediaMetadata,
    MessagePriority as CorePriority,
};
use offline_protocol_mls::{
    EncryptedMessage as CoreEncryptedMessage, GroupId as CoreGroupId, GroupInfo as CoreGroupInfo,
    KeyPackageBundle as CoreKeyPackageBundle, MlsManager as CoreMlsManager,
    MlsStorage as CoreMlsStorage, StorageError as CoreStorageError,
    WelcomeMessage as CoreWelcomeMessage,
};
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

/// Per-peer establishment state (for SessionNotReady and get_establishment_state).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum EstablishmentState {
    NoKeyPackage,
    HaveKeyPackage,
    SessionPending,
    SessionConfirmed,
}

impl From<CoreEstablishmentState> for EstablishmentState {
    fn from(s: CoreEstablishmentState) -> Self {
        match s {
            CoreEstablishmentState::NoKeyPackage => EstablishmentState::NoKeyPackage,
            CoreEstablishmentState::HaveKeyPackage => EstablishmentState::HaveKeyPackage,
            CoreEstablishmentState::SessionPending => EstablishmentState::SessionPending,
            CoreEstablishmentState::SessionConfirmed => EstablishmentState::SessionConfirmed,
        }
    }
}

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

    /// No key package available for recipient
    #[error("No key package available for recipient: {0}")]
    NoKeyPackage(String),

    /// Session not ready; establishment in progress (state included for UI/retry).
    #[error("Session not ready: {0:?}")]
    SessionNotReady(EstablishmentState),

    /// Outbound message encryption failed
    #[error("Failed to encrypt message: {0}")]
    EncryptFailed(String),

    /// Invalid state for operation
    #[error("Invalid state: {0}")]
    InvalidState(String),

    /// MLS not initialized
    #[error("MLS not initialized")]
    MlsNotInitialized,

    /// MLS operation failed
    #[error("MLS error: {0}")]
    MlsError(String),

    /// Other error
    #[error("{0}")]
    Other(String),
}

/// Error types for MLS storage operations
#[derive(Debug, thiserror::Error)]
pub enum MlsStorageError {
    /// Failed to store data
    #[error("Failed to store data")]
    StoreFailed,

    /// Failed to load data
    #[error("Failed to load data")]
    LoadFailed,

    /// Failed to delete data
    #[error("Failed to delete data")]
    DeleteFailed,

    /// Key not found
    #[error("Key not found")]
    KeyNotFound,

    /// Data is corrupted
    #[error("Corrupted data")]
    CorruptedData,
}

impl From<CoreStorageError> for MlsStorageError {
    fn from(err: CoreStorageError) -> Self {
        match err {
            CoreStorageError::StoreFailed(_) => MlsStorageError::StoreFailed,
            CoreStorageError::LoadFailed(_) => MlsStorageError::LoadFailed,
            CoreStorageError::DeleteFailed(_) => MlsStorageError::DeleteFailed,
            CoreStorageError::KeyNotFound(_) => MlsStorageError::KeyNotFound,
            CoreStorageError::CorruptedData(_) => MlsStorageError::CorruptedData,
            CoreStorageError::Unavailable(_) => MlsStorageError::LoadFailed,
        }
    }
}

/// MLS Storage callback interface - apps implement this for platform-native secure storage
pub trait MlsStorageProvider: Send + Sync {
    /// Store data with the given key type and ID
    fn store(&self, key_type: String, key_id: String, data: Vec<u8>)
        -> Result<(), MlsStorageError>;

    /// Load data for the given key type and ID
    fn load(&self, key_type: String, key_id: String) -> Result<Option<Vec<u8>>, MlsStorageError>;

    /// Delete data for the given key type and ID
    fn delete(&self, key_type: String, key_id: String) -> Result<(), MlsStorageError>;

    /// List all key IDs for a given key type
    fn list_keys(&self, key_type: String) -> Result<Vec<String>, MlsStorageError>;
}

/// Wrapper to adapt UniFFI callback to core MlsStorage trait
struct MlsStorageWrapper {
    provider: Arc<dyn MlsStorageProvider>,
}

impl CoreMlsStorage for MlsStorageWrapper {
    fn store(
        &self,
        key_type: &str,
        key_id: &str,
        data: &[u8],
    ) -> offline_protocol_mls::storage::StorageResult<()> {
        self.provider
            .store(key_type.to_string(), key_id.to_string(), data.to_vec())
            .map_err(|e| match e {
                MlsStorageError::StoreFailed => {
                    CoreStorageError::StoreFailed("Storage failed".to_string())
                }
                MlsStorageError::LoadFailed => {
                    CoreStorageError::StoreFailed("Load failed".to_string())
                }
                MlsStorageError::DeleteFailed => {
                    CoreStorageError::StoreFailed("Delete failed".to_string())
                }
                MlsStorageError::KeyNotFound => CoreStorageError::KeyNotFound(key_id.to_string()),
                MlsStorageError::CorruptedData => {
                    CoreStorageError::CorruptedData("Data corrupted".to_string())
                }
            })
    }

    fn load(
        &self,
        key_type: &str,
        key_id: &str,
    ) -> offline_protocol_mls::storage::StorageResult<Option<Vec<u8>>> {
        self.provider
            .load(key_type.to_string(), key_id.to_string())
            .map_err(|e| match e {
                MlsStorageError::StoreFailed => {
                    CoreStorageError::LoadFailed("Storage failed".to_string())
                }
                MlsStorageError::LoadFailed => {
                    CoreStorageError::LoadFailed("Load failed".to_string())
                }
                MlsStorageError::DeleteFailed => {
                    CoreStorageError::LoadFailed("Delete failed".to_string())
                }
                MlsStorageError::KeyNotFound => CoreStorageError::KeyNotFound(key_id.to_string()),
                MlsStorageError::CorruptedData => {
                    CoreStorageError::CorruptedData("Data corrupted".to_string())
                }
            })
    }

    fn delete(
        &self,
        key_type: &str,
        key_id: &str,
    ) -> offline_protocol_mls::storage::StorageResult<()> {
        self.provider
            .delete(key_type.to_string(), key_id.to_string())
            .map_err(|e| match e {
                MlsStorageError::StoreFailed => {
                    CoreStorageError::DeleteFailed("Storage failed".to_string())
                }
                MlsStorageError::LoadFailed => {
                    CoreStorageError::DeleteFailed("Load failed".to_string())
                }
                MlsStorageError::DeleteFailed => {
                    CoreStorageError::DeleteFailed("Delete failed".to_string())
                }
                MlsStorageError::KeyNotFound => CoreStorageError::KeyNotFound(key_id.to_string()),
                MlsStorageError::CorruptedData => {
                    CoreStorageError::CorruptedData("Data corrupted".to_string())
                }
            })
    }

    fn list_keys(
        &self,
        key_type: &str,
    ) -> offline_protocol_mls::storage::StorageResult<Vec<String>> {
        self.provider
            .list_keys(key_type.to_string())
            .map_err(|e| match e {
                MlsStorageError::StoreFailed => {
                    CoreStorageError::LoadFailed("Storage failed".to_string())
                }
                MlsStorageError::LoadFailed => {
                    CoreStorageError::LoadFailed("Load failed".to_string())
                }
                MlsStorageError::DeleteFailed => {
                    CoreStorageError::LoadFailed("Delete failed".to_string())
                }
                MlsStorageError::KeyNotFound => CoreStorageError::KeyNotFound("".to_string()),
                MlsStorageError::CorruptedData => {
                    CoreStorageError::CorruptedData("Data corrupted".to_string())
                }
            })
    }
}

impl From<offline_protocol::Error> for ProtocolError {
    fn from(err: offline_protocol::Error) -> Self {
        match err {
            offline_protocol::Error::NotStarted => ProtocolError::NotStarted,
            offline_protocol::Error::AlreadyStarted => ProtocolError::AlreadyStarted,
            offline_protocol::Error::InvalidConfiguration(msg) => {
                ProtocolError::InvalidConfiguration(msg)
            }
            offline_protocol::Error::NoKeyPackage(peer_id) => ProtocolError::NoKeyPackage(peer_id),
            offline_protocol::Error::SessionNotReady(state) => {
                ProtocolError::SessionNotReady(state.into())
            }
            offline_protocol::Error::EncryptFailed(message) => {
                ProtocolError::EncryptFailed(message)
            }
            offline_protocol::Error::MlsNotInitialized => ProtocolError::MlsNotInitialized,
            offline_protocol::Error::Mls(err) => ProtocolError::MlsError(err.to_string()),
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

/// Content type for messages.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ContentType {
    Text,
    Image,
    Video,
    Audio,
    VoiceNote,
    VideoNote,
    File,
    FileChunk,
}

impl From<ContentType> for CoreContentType {
    fn from(ct: ContentType) -> Self {
        match ct {
            ContentType::Text => CoreContentType::Text,
            ContentType::Image => CoreContentType::Image,
            ContentType::Video => CoreContentType::Video,
            ContentType::Audio => CoreContentType::Audio,
            ContentType::VoiceNote => CoreContentType::VoiceNote,
            ContentType::VideoNote => CoreContentType::VideoNote,
            ContentType::File => CoreContentType::File,
            ContentType::FileChunk => CoreContentType::FileChunk,
        }
    }
}

/// Media metadata for attachments.
#[derive(Debug, Clone)]
pub struct MediaMetadata {
    pub mime_type: String,
    pub file_name: String,
    pub file_size: u64,
    pub duration_ms: Option<u64>,
    pub width: Option<u32>,
    pub height: Option<u32>,
    pub thumbnail_base64: Option<String>,
}

impl From<MediaMetadata> for CoreMediaMetadata {
    fn from(m: MediaMetadata) -> Self {
        CoreMediaMetadata {
            mime_type: m.mime_type,
            file_name: m.file_name,
            file_size: m.file_size,
            duration_ms: m.duration_ms,
            width: m.width,
            height: m.height,
            thumbnail_base64: m.thumbnail_base64,
        }
    }
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

// ========================================================================
// MLS TYPES
// ========================================================================

/// Key package bundle for distribution
#[derive(Debug, Clone)]
pub struct MlsKeyPackageBundle {
    pub package_id: String,
    pub user_id: String,
    pub key_package_data: Vec<u8>,
    pub created_at_ms: u64,
    pub expires_at_ms: u64,
    pub synced: bool,
}

impl From<CoreKeyPackageBundle> for MlsKeyPackageBundle {
    fn from(bundle: CoreKeyPackageBundle) -> Self {
        Self {
            package_id: bundle.package_id,
            user_id: bundle.user_id,
            key_package_data: bundle.key_package_data,
            created_at_ms: bundle.created_at_ms,
            expires_at_ms: bundle.expires_at_ms,
            synced: bundle.synced,
        }
    }
}

/// Welcome message for inviting users to a group
#[derive(Debug, Clone)]
pub struct MlsWelcomeMessage {
    pub group_id: String,
    pub welcome_data: Vec<u8>,
    pub inviter_id: String,
    pub group_name: Option<String>,
    pub timestamp_ms: u64,
}

impl From<CoreWelcomeMessage> for MlsWelcomeMessage {
    fn from(msg: CoreWelcomeMessage) -> Self {
        Self {
            group_id: msg.group_id.as_str().to_string(),
            welcome_data: msg.welcome_data,
            inviter_id: msg.inviter_id,
            group_name: msg.group_name,
            timestamp_ms: msg.timestamp_ms,
        }
    }
}

impl From<MlsWelcomeMessage> for CoreWelcomeMessage {
    fn from(msg: MlsWelcomeMessage) -> Self {
        Self {
            group_id: CoreGroupId::new(msg.group_id),
            welcome_data: msg.welcome_data,
            inviter_id: msg.inviter_id,
            group_name: msg.group_name,
            timestamp_ms: msg.timestamp_ms,
        }
    }
}

/// Encrypted message for transport
#[derive(Debug, Clone)]
pub struct MlsEncryptedMessage {
    pub group_id: String,
    pub message_type: String,
    pub epoch: u64,
    pub ciphertext: Vec<u8>,
    pub sender_id: String,
    pub timestamp_ms: u64,
}

impl From<CoreEncryptedMessage> for MlsEncryptedMessage {
    fn from(msg: CoreEncryptedMessage) -> Self {
        Self {
            group_id: msg.group_id.as_str().to_string(),
            message_type: msg.message_type.as_str().to_string(),
            epoch: msg.epoch,
            ciphertext: msg.ciphertext,
            sender_id: msg.sender_id,
            timestamp_ms: msg.timestamp_ms,
        }
    }
}

impl From<MlsEncryptedMessage> for CoreEncryptedMessage {
    fn from(msg: MlsEncryptedMessage) -> Self {
        use offline_protocol_mls::MlsMessageType;
        let message_type =
            MlsMessageType::from_str_opt(&msg.message_type).unwrap_or(MlsMessageType::Application);
        Self {
            group_id: CoreGroupId::new(msg.group_id),
            message_type,
            epoch: msg.epoch,
            ciphertext: msg.ciphertext,
            sender_id: msg.sender_id,
            timestamp_ms: msg.timestamp_ms,
        }
    }
}

/// Result of adding a member to an MLS group.
///
/// Contains both the Welcome message (to be sent to the invitee) and the
/// Commit message (to be distributed to all existing group members so they
/// can advance their MLS epoch).
#[derive(Debug, Clone)]
pub struct MlsAddMemberResult {
    pub welcome: MlsWelcomeMessage,
    pub commit: MlsEncryptedMessage,
}

/// Group information
#[derive(Debug, Clone)]
pub struct MlsGroupInfo {
    pub group_id: String,
    pub name: Option<String>,
    pub members: Vec<String>,
    pub epoch: u64,
    pub is_session: bool,
    pub created_at_ms: u64,
    pub last_activity_ms: u64,
}

impl From<CoreGroupInfo> for MlsGroupInfo {
    fn from(info: CoreGroupInfo) -> Self {
        Self {
            group_id: info.group_id.as_str().to_string(),
            name: info.name,
            members: info.members,
            epoch: info.epoch,
            is_session: info.is_session,
            created_at_ms: info.created_at_ms,
            last_activity_ms: info.last_activity_ms,
        }
    }
}

/// DORS configuration
#[derive(Debug, Clone)]
pub struct DorsConfig {
    pub prefer_online: bool,
    pub switch_hysteresis: f32,
    pub switch_cooldown_secs: u64,
    pub ble_to_wifi_retry_threshold: u32,
    pub min_success_rate_before_escalation: f32,
    pub min_ble_samples_before_success_rate_escalation: u64,
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

/// Encryption configuration for automatic MLS handling
#[derive(Debug, Clone)]
pub struct EncryptionConfig {
    /// Whether automatic encryption is enabled (default: true)
    pub enabled: bool,
    /// Auto-exchange key packages on peer discovery (default: true)
    pub auto_key_exchange: bool,
    /// Store pending messages when no session exists (default: true)
    pub store_pending: bool,
    /// Require encryption for outbound sends (default: false)
    pub require_encryption: bool,
    /// Pending queue configuration for encrypted pre-session messages.
    pub pending_queue: PendingQueueConfig,
}

impl Default for EncryptionConfig {
    fn default() -> Self {
        Self {
            enabled: true,
            auto_key_exchange: true,
            store_pending: true,
            require_encryption: false,
            pending_queue: PendingQueueConfig::default(),
        }
    }
}

#[derive(Debug, Clone)]
pub enum OverflowPolicy {
    DropOldest,
    DropNewest,
}

impl Default for OverflowPolicy {
    fn default() -> Self {
        Self::DropOldest
    }
}

#[derive(Debug, Clone)]
pub struct PendingQueueConfig {
    pub max_pending_per_peer: u64,
    pub max_pending_global: u64,
    pub pending_ttl_ms: u64,
    pub overflow_policy: OverflowPolicy,
}

impl Default for PendingQueueConfig {
    fn default() -> Self {
        Self {
            max_pending_per_peer: 64,
            max_pending_global: 4096,
            pending_ttl_ms: 120_000,
            overflow_policy: OverflowPolicy::DropOldest,
        }
    }
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
    pub encryption_enabled: bool,
    pub auto_key_exchange: bool,
    pub store_pending: bool,
    pub require_encryption: bool,
    pub max_pending_per_peer: u64,
    pub max_pending_global: u64,
    pub pending_ttl_ms: u64,
    pub overflow_policy: OverflowPolicy,
    pub max_group_members: u32,
    pub group_relay_enabled: bool,
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
        core_config.encryption.enabled = config.encryption_enabled;
        core_config.encryption.auto_key_exchange = config.auto_key_exchange;
        core_config.encryption.store_pending = config.store_pending;
        core_config.encryption.require_encryption = config.require_encryption;
        core_config.encryption.pending_queue = CorePendingQueueConfig {
            max_pending_per_peer: config.max_pending_per_peer as usize,
            max_pending_global: config.max_pending_global as usize,
            pending_ttl_ms: config.pending_ttl_ms,
            overflow_policy: match config.overflow_policy {
                OverflowPolicy::DropOldest => CoreOverflowPolicy::DropOldest,
                OverflowPolicy::DropNewest => CoreOverflowPolicy::DropNewest,
            },
        };
        core_config.group.max_group_members = config.max_group_members as usize;
        core_config.group.relay_enabled = config.group_relay_enabled;
        core_config
    }
}

/// Event callback trait
pub trait EventCallback: Send + Sync {
    fn on_event(&self, event_json: String);
}

/// BLE transport callback trait — notifies platform when outgoing fragments are available.
/// Replaces timer-based polling with event-driven sending.
pub trait BleTransportCallback: Send + Sync {
    fn on_fragments_available(&self);
}

/// WiFi Direct transport callback trait — notifies platform when outgoing messages are available.
pub trait WifiDirectTransportCallback: Send + Sync {
    fn on_messages_available(&self);
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
    /// Unique message identifier. Use this with `internet_confirm_sent()` or
    /// `internet_send_failed()`/`internet_send_failed_with_reason()` to report
    /// the send outcome.
    pub message_id: String,
    pub recipient_id: String,
    pub data: Vec<u8>,
    pub reply_to_msg: Option<String>,
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
    // TRANSPORT CALLBACKS (EVENT-DRIVEN SENDING)
    // ========================================================================

    /// Registers a BLE transport callback that fires when outgoing fragments
    /// become available. This replaces timer-based polling — the platform
    /// should call `ble_get_next_fragment()` inside the callback.
    pub fn set_ble_transport_callback(&self, callback: Box<dyn BleTransportCallback>) {
        let callback: Arc<dyn BleTransportCallback> = Arc::from(callback);
        let protocol = self.inner.lock().unwrap();
        if let Some(transport_arc) = protocol
            .transport_manager()
            .get_transport(CoreTransportType::BLE)
        {
            let transport = transport_arc.lock().unwrap();
            if let Some(ble_transport) = transport.as_any().downcast_ref::<BleTransport>() {
                let cb = callback.clone();
                ble_transport.set_on_fragments_available(Arc::new(move || {
                    cb.on_fragments_available();
                }));
            }
        }
    }

    /// Registers a WiFi Direct transport callback that fires when outgoing
    /// messages become available. This replaces timer-based polling.
    pub fn set_wifi_direct_transport_callback(
        &self,
        callback: Box<dyn WifiDirectTransportCallback>,
    ) {
        let callback: Arc<dyn WifiDirectTransportCallback> = Arc::from(callback);
        let protocol = self.inner.lock().unwrap();
        if let Some(transport_arc) = protocol
            .transport_manager()
            .get_transport(CoreTransportType::WiFiDirect)
        {
            let transport = transport_arc.lock().unwrap();
            if let Some(wifi_transport) = transport.as_any().downcast_ref::<WifiDirectTransport>() {
                let cb = callback.clone();
                wifi_transport.set_on_messages_available(Arc::new(move || {
                    cb.on_messages_available();
                }));
            }
        }
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
        reply_to_msg: Option<String>,
    ) -> Result<String, ProtocolError> {
        let mut protocol = self.inner.lock().unwrap();

        // Check if a transport is forced (bypasses DORS)
        let forced = *self.forced_transport.read().unwrap();

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
                    reply_to_msg,
                )
                .map_err(ProtocolError::from)?
        } else {
            protocol
                .send_message(&recipient, &content, Some(priority.into()), reply_to_msg)
                .map_err(ProtocolError::from)?
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
                "lamport_clock": msg.lamport_clock.value(),
                "hop_count": msg.hop_count.value(),
                "priority": format!("{:?}", msg.priority),
            }))
            .ok()
        })
    }

    // ========================================================================
    // CONNECTION REQUESTS (TRANSPORT-AGNOSTIC)
    // ========================================================================

    /// Sends a connection request to another user via any available transport (DORS-routed).
    pub fn send_connection_request(
        &self,
        recipient: String,
        sender_name: String,
        key_package: Option<Vec<u8>>,
    ) -> Result<String, ProtocolError> {
        let mut protocol = self.inner.lock().unwrap();
        let message_id = protocol
            .send_connection_request(&recipient, &sender_name, key_package)
            .map_err(ProtocolError::from)?;
        Ok(message_id.as_str())
    }

    /// Accepts a connection request from another user via any available transport (DORS-routed).
    pub fn accept_connection_request(
        &self,
        recipient: String,
        accepter_name: String,
        key_package: Option<Vec<u8>>,
    ) -> Result<String, ProtocolError> {
        let mut protocol = self.inner.lock().unwrap();
        let message_id = protocol
            .accept_connection_request(&recipient, &accepter_name, key_package)
            .map_err(ProtocolError::from)?;
        Ok(message_id.as_str())
    }

    /// Rejects a connection request from another user via any available transport (DORS-routed).
    pub fn reject_connection_request(&self, recipient: String) -> Result<String, ProtocolError> {
        let mut protocol = self.inner.lock().unwrap();
        let message_id = protocol
            .reject_connection_request(&recipient)
            .map_err(ProtocolError::from)?;
        Ok(message_id.as_str())
    }

    // ========================================================================
    // SERVICE DISCOVERY (delegated via MeshServices wrapper)
    // ========================================================================

    /// Registers a local service for discovery.
    pub(crate) fn svc_register_service(
        &self,
        service_id: String,
        version: String,
        capabilities: HashMap<String, String>,
    ) -> Result<(), ProtocolError> {
        use offline_protocol_core::{ServiceDescriptor, ServiceId};
        let sid = ServiceId::new(&service_id)
            .map_err(|e| ProtocolError::InvalidConfiguration(e.to_string()))?;
        let descriptor = ServiceDescriptor {
            service_id: sid,
            version,
            capabilities,
        };
        let mut protocol = self.inner.lock().unwrap();
        protocol
            .register_service(descriptor)
            .map_err(ProtocolError::from)
    }

    /// Unregisters a local service. Returns true if found and removed.
    pub(crate) fn svc_unregister_service(&self, service_id: String) -> Result<bool, ProtocolError> {
        let mut protocol = self.inner.lock().unwrap();
        protocol
            .unregister_service(&service_id)
            .map_err(ProtocolError::from)
    }

    /// Broadcasts a service discovery query. Returns a query_id.
    pub(crate) fn svc_discover_services(
        &self,
        service_id: Option<String>,
    ) -> Result<String, ProtocolError> {
        let mut protocol = self.inner.lock().unwrap();
        protocol
            .discover_services(service_id.as_deref())
            .map_err(ProtocolError::from)
    }

    /// Sends a service request to a specific provider peer. Returns a request_id.
    pub(crate) fn svc_send_service_request(
        &self,
        provider: String,
        service_id: String,
        method: String,
        body: String,
    ) -> Result<String, ProtocolError> {
        let mut protocol = self.inner.lock().unwrap();
        protocol
            .send_service_request(&provider, &service_id, &method, &body)
            .map_err(ProtocolError::from)
    }

    /// Responds to a service request from another peer.
    pub(crate) fn svc_respond_to_service_request(
        &self,
        request_id: String,
        requester: String,
        service_id: String,
        status: String,
        body: String,
    ) -> Result<String, ProtocolError> {
        let mut protocol = self.inner.lock().unwrap();
        let message_id = protocol
            .respond_to_service_request(&request_id, &requester, &service_id, &status, &body)
            .map_err(ProtocolError::from)?;
        Ok(message_id.as_str())
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

        // Register peer with the BLE transport so send() can route to them
        {
            let protocol = self.inner.lock().unwrap();
            if let Some(transport_arc) = protocol
                .transport_manager()
                .get_transport(CoreTransportType::BLE)
            {
                let transport = transport_arc.lock().unwrap();
                if let Some(ble_transport) = transport.as_any().downcast_ref::<BleTransport>() {
                    ble_transport.on_peer_discovered(offline_protocol_transport::ble::PeerDevice {
                        device_id: peer_id.clone(),
                        address: String::new(),
                        rssi,
                        last_seen: SystemTime::now(),
                        connected: true,
                    });
                }
            }
        }

        // Notify the core protocol of neighbor discovery for auto key exchange
        {
            let mut protocol = self.inner.lock().unwrap();
            protocol.on_neighbor_discovered(&peer_id);
        }

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

        // Unregister peer from the BLE transport
        {
            let protocol = self.inner.lock().unwrap();
            if let Some(transport_arc) = protocol
                .transport_manager()
                .get_transport(CoreTransportType::BLE)
            {
                let transport = transport_arc.lock().unwrap();
                if let Some(ble_transport) = transport.as_any().downcast_ref::<BleTransport>() {
                    ble_transport.on_peer_lost(&peer_id);
                }
            }
        }

        // Notify the core protocol of neighbor loss
        {
            let mut protocol = self.inner.lock().unwrap();
            protocol.on_neighbor_lost(&peer_id);
        }

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
        let mut protocol = self.inner.lock().unwrap();
        if let Some(transport_arc) = protocol
            .transport_manager()
            .get_transport(CoreTransportType::BLE)
        {
            let transport = transport_arc.lock().unwrap();
            if let Some(ble_transport) = transport.as_any().downcast_ref::<BleTransport>() {
                ble_transport.on_fragment_received(fragment).map_err(|e| {
                    ProtocolError::Other(format!("Fragment processing failed: {}", e))
                })?;
            } else {
                return Err(ProtocolError::Other(
                    "BLE transport not available or wrong type".to_string(),
                ));
            }
        }

        while protocol.receive_message().is_some() {}

        Ok(())
    }

    /// BLE: Get next fragment to send
    pub fn ble_get_next_fragment(&self) -> Option<BleFragment> {
        //  Ensure BLE transport is available for fragment polling
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
        let mut protocol = self.inner.lock().unwrap();
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
                if let Err(e) = internet_transport.on_data_received(data) {
                    return Err(ProtocolError::Other(format!(
                        "Failed to process internet message: {}",
                        e
                    )));
                }
            }
        }

        while protocol.receive_message().is_some() {}
        drop(protocol);

        let event = CoreEvent::NeighborDiscovered {
            peer_id: sender_id.clone(),
            transport: "Internet".to_string(),
            rssi: None,
        };
        self.emit_event(event);

        Ok(())
    }

    /// Internet: Get next message to send via WebSocket.
    ///
    /// Returns the next queued message with its `message_id`. After sending
    /// over the wire, the platform **must** call either `internet_confirm_sent(message_id)`
    /// or `internet_send_failed(message_id)`/`internet_send_failed_with_reason(message_id, reason)`
    /// to close the feedback loop.
    pub fn internet_get_next_message(&self) -> Option<InternetMessage> {
        {
            let internet_state = self.internet_state.lock().unwrap();
            if !internet_state.is_connected {
                return None;
            }
        }

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
                if let Ok(Some((message_id, data))) = internet_transport.get_next_message() {
                    if let Ok(message) = internet_transport.deserialize_message(&data) {
                        return Some(InternetMessage {
                            message_id,
                            recipient_id: message.recipient.as_str().to_string(),
                            data,
                            reply_to_msg: message
                                .reply_to_msg
                                .as_ref()
                                .map(|id| id.as_str().to_string()),
                        });
                    }
                }
            }
        }

        // Fallback to local queue.
        // Loop so that un-deserializable entries are skipped rather than
        // blocking the rest of the queue.
        let mut internet_state = self.internet_state.lock().unwrap();
        while let Some((recipient, data)) = internet_state.outgoing_messages.pop_front() {
            let parsed = if let Some(transport_arc) = protocol
                .transport_manager()
                .get_transport(CoreTransportType::Internet)
            {
                let transport = transport_arc.lock().unwrap();
                transport
                    .as_any()
                    .downcast_ref::<offline_protocol_transport::internet::InternetTransport>()
                    .and_then(|it| it.deserialize_message(&data).ok())
            } else {
                None
            };

            let msg_id = parsed
                .as_ref()
                .map(|msg| msg.id.as_str().to_string())
                .unwrap_or_default();

            // An empty message_id would break the confirm/fail feedback loop — skip it
            // and try the next entry.  These messages are permanently lost (no outbox
            // entry, no retry).  If this fires systematically it indicates a
            // serialization schema mismatch that must be investigated.
            if msg_id.is_empty() {
                tracing::warn!(
                    recipient = %recipient,
                    data_len = data.len(),
                    "Dropping fallback internet message: could not recover message_id from deserialization — message is permanently lost"
                );
                continue;
            }

            let reply_to_msg = parsed
                .as_ref()
                .and_then(|msg| msg.reply_to_msg.as_ref().map(|id| id.as_str().to_string()));

            return Some(InternetMessage {
                message_id: msg_id,
                recipient_id: recipient,
                data,
                reply_to_msg,
            });
        }

        None
    }

    /// Internet: Confirm that a message was successfully sent over the wire.
    ///
    /// The platform must call this after the WebSocket send completes successfully.
    /// This feeds real delivery data into transport metrics so DORS can make
    /// accurate routing decisions.
    pub fn internet_confirm_sent(&self, message_id: String) {
        let mut protocol = self.inner.lock().unwrap();
        if let Err(err) = protocol.on_transport_send_confirmed(&message_id) {
            tracing::warn!(
                message_id = %message_id,
                error = %err,
                "Failed to apply welcome lifecycle transport confirmation"
            );
        }
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
                internet_transport.confirm_sent(&message_id);
            }
        }
    }

    /// Internet: Report that a message failed to send over the wire.
    ///
    /// The platform must call this when the WebSocket send fails.
    pub fn internet_send_failed(&self, message_id: String) {
        self.internet_send_failed_with_reason(
            message_id,
            Some("Internet transport send failed".to_string()),
        );
    }

    /// Internet: Report that a message failed to send over the wire.
    ///
    /// `reason` should carry platform-specific error context so reliability
    /// telemetry can classify root causes more accurately.
    pub fn internet_send_failed_with_reason(&self, message_id: String, reason: Option<String>) {
        let mut protocol = self.inner.lock().unwrap();
        if let Err(err) = protocol.on_transport_send_failed(&message_id, reason) {
            tracing::warn!(
                message_id = %message_id,
                error = %err,
                "Failed to apply welcome lifecycle transport failure"
            );
        }
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
                internet_transport.report_send_failure(&message_id);
            }
        }
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
        let mut protocol = self.inner.lock().unwrap();
        if let Some(transport_arc) = protocol
            .transport_manager()
            .get_transport(CoreTransportType::WiFiDirect)
        {
            let transport = transport_arc.lock().unwrap();
            if let Some(wifi_transport) = transport.as_any().downcast_ref::<WifiDirectTransport>() {
                if let Err(e) = wifi_transport.on_data_received(data) {
                    return Err(ProtocolError::Other(format!(
                        "Failed to process WiFi Direct message: {}",
                        e
                    )));
                }
            }
        }

        while protocol.receive_message().is_some() {}
        drop(protocol);

        let event = CoreEvent::NeighborDiscovered {
            peer_id: sender_id.clone(),
            transport: "WiFiDirect".to_string(),
            rssi: None,
        };
        self.emit_event(event);

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
    // MEDIA AND FILE TRANSFER
    // ========================================================================

    /// Sends a media attachment through the protocol.
    ///
    /// The platform reads the file and passes the raw bytes. The SDK chunks
    /// the data, sends each chunk as a message (internet-preferred), and
    /// emits progress events.
    pub fn send_media(
        &self,
        recipient: String,
        file_data: Vec<u8>,
        file_name: String,
        content_type: ContentType,
        media_metadata: Option<MediaMetadata>,
    ) -> Result<String, ProtocolError> {
        let mut protocol = self.inner.lock().unwrap();
        let core_meta = media_metadata.map(CoreMediaMetadata::from);
        protocol
            .send_media(
                recipient,
                file_data,
                file_name,
                content_type.into(),
                core_meta,
            )
            .map_err(|e| e.into())
    }

    /// Convenience: sends a generic file (delegates to send_media with ContentType::File).
    pub fn send_file(
        &self,
        recipient: String,
        file_data: Vec<u8>,
        file_name: String,
    ) -> Result<String, ProtocolError> {
        self.send_media(recipient, file_data, file_name, ContentType::File, None)
    }

    /// Processes a received file chunk (manual path, for platforms handling
    /// their own chunk routing outside the protocol receive loop).
    #[allow(clippy::too_many_arguments)]
    pub fn process_file_chunk(
        &self,
        file_id: String,
        chunk_index: u32,
        total_chunks: u32,
        file_size: u64,
        file_name: String,
        file_checksum: String,
        data: Vec<u8>,
    ) -> Result<(), ProtocolError> {
        let mut protocol = self.inner.lock().unwrap();

        use offline_protocol::file_transfer::FileChunk;
        let chunk = FileChunk {
            file_id,
            file_name,
            file_size,
            total_chunks,
            chunk_index,
            chunk_data: data,
            file_checksum,
        };

        protocol.file_transfer_manager_mut().process_chunk(chunk);
        Ok(())
    }

    /// Gets file transfer progress.
    pub fn get_file_progress(&self, file_id: String) -> Option<FileProgress> {
        let protocol = self.inner.lock().unwrap();
        let core_progress = protocol.file_transfer_manager().get_progress(&file_id)?;

        Some(FileProgress {
            file_id: core_progress.file_id,
            chunks_sent: core_progress.chunks_completed,
            total_chunks: core_progress.total_chunks,
            percentage: core_progress.percentage,
        })
    }

    /// Finalizes a file transfer, returning the reassembled bytes.
    pub fn finalize_file(&self, file_id: String) -> Result<(), ProtocolError> {
        let mut protocol = self.inner.lock().unwrap();
        protocol
            .file_transfer_manager_mut()
            .finalize_file(&file_id)
            .ok_or_else(|| ProtocolError::Other("File not found or incomplete".to_string()))?;
        Ok(())
    }

    /// Cancels an active file transfer.
    pub fn cancel_file_transfer(&self, file_id: String) -> Result<(), ProtocolError> {
        let mut protocol = self.inner.lock().unwrap();
        if protocol
            .file_transfer_manager_mut()
            .cancel_transfer(&file_id)
        {
            Ok(())
        } else {
            Err(ProtocolError::Other("File transfer not found".to_string()))
        }
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
                status: (if s.delivered_at.is_some() {
                    "delivered"
                } else {
                    "pending"
                })
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
            min_success_rate_before_escalation: config.min_success_rate_before_escalation,
            min_ble_samples_before_success_rate_escalation: config
                .min_ble_samples_before_success_rate_escalation
                as usize,
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

        let core = CoreDorsConfig::default();
        DorsConfig {
            prefer_online: core.prefer_online,
            switch_hysteresis: core.switch_hysteresis,
            switch_cooldown_secs: core.switch_cooldown_secs,
            ble_to_wifi_retry_threshold: core.ble_to_wifi_retry_threshold,
            min_success_rate_before_escalation: core.min_success_rate_before_escalation,
            min_ble_samples_before_success_rate_escalation: core
                .min_ble_samples_before_success_rate_escalation
                as u64,
            rssi_switch_threshold: core.rssi_switch_threshold,
            congestion_queue_threshold: core.congestion_queue_threshold as u64,
            stability_window_secs: core.stability_window_secs,
            poor_signal_duration_secs: core.poor_signal_duration_secs,
            ttl_escalation_threshold: core.ttl_escalation_threshold,
            congestion_duration_secs: core.congestion_duration_secs,
            ttl_escalation_hold_secs: core.ttl_escalation_hold_secs,
            history_window_size: core.history_window_size as u64,
            queue_recovery_ratio: core.queue_recovery_ratio,
        }
    }

    // ========================================================================
    // GRADIENT ROUTING TABLE OPERATIONS
    // ========================================================================

    /// Learns a route from an incoming message.
    /// Call this when receiving a message from a neighbor to record that
    /// the neighbor can reach the message's original sender.
    pub fn learn_route(
        &self,
        destination: String,
        next_hop: String,
        hop_count: u8,
        quality: f32,
        sequence_number: u32,
    ) {
        let mut path_selector = self.path_selector.lock().unwrap();
        path_selector.routing_table_mut().learn_route(
            &destination,
            &next_hop,
            hop_count,
            quality,
            sequence_number,
        );
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
                let last_seen_ms = now - (elapsed.as_millis() as u64);

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

    // ========================================================================
    // MLS (END-TO-END ENCRYPTION) OPERATIONS
    // ========================================================================

    /// Initialize MLS with a storage provider
    pub fn initialize_mls(
        &self,
        storage: Box<dyn MlsStorageProvider>,
    ) -> Result<(), ProtocolError> {
        let wrapper = Arc::new(MlsStorageWrapper {
            provider: Arc::from(storage),
        });

        // Single-authority lifecycle:
        // - CoreProtocol owns the only MlsManager instance for this runtime.
        // - UniFFI manual MLS APIs must route through that owner.
        // - Repeated calls are idempotent and never replace the existing manager.
        let mut protocol = self
            .inner
            .lock()
            .map_err(|_| ProtocolError::Other("Protocol lock poisoned".to_string()))?;
        if protocol.is_mls_initialized() {
            return Ok(());
        }
        protocol
            .initialize_mls(wrapper)
            .map_err(|e| ProtocolError::MlsError(e.to_string()))?;
        Ok(())
    }

    /// Check if MLS is initialized
    pub fn is_mls_initialized(&self) -> bool {
        let protocol = self.inner.lock().unwrap();
        protocol.is_mls_initialized()
    }

    /// Returns the core-owned MLS manager handle.
    ///
    /// This is the only MLS state owner for the runtime. UniFFI must never
    /// create or cache an independent manager because that would diverge
    /// key-package/session/group state from auto-encryption flows.
    fn get_mls_manager(&self) -> Result<Arc<RwLock<CoreMlsManager>>, ProtocolError> {
        let protocol = self
            .inner
            .lock()
            .map_err(|_| ProtocolError::Other("Protocol lock poisoned".to_string()))?;
        protocol
            .mls_manager()
            .cloned()
            .ok_or(ProtocolError::MlsNotInitialized)
    }

    /// Generate a key package for distribution
    pub fn mls_generate_key_package(&self) -> Result<MlsKeyPackageBundle, ProtocolError> {
        let manager = self.get_mls_manager()?;
        let guard = manager
            .read()
            .map_err(|_| ProtocolError::Other("MLS manager lock poisoned".to_string()))?;
        guard
            .generate_key_package()
            .map(MlsKeyPackageBundle::from)
            .map_err(|e| ProtocolError::MlsError(e.to_string()))
    }

    /// Get an existing key package or generate a new one
    pub fn mls_get_or_create_key_package(&self) -> Result<MlsKeyPackageBundle, ProtocolError> {
        let manager = self.get_mls_manager()?;
        let guard = manager
            .read()
            .map_err(|_| ProtocolError::Other("MLS manager lock poisoned".to_string()))?;
        guard
            .get_or_create_key_package()
            .map(MlsKeyPackageBundle::from)
            .map_err(|e| ProtocolError::MlsError(e.to_string()))
    }

    /// Import a contact's key package
    pub fn mls_import_key_package(
        &self,
        user_id: String,
        key_package_data: Vec<u8>,
    ) -> Result<(), ProtocolError> {
        let manager = self.get_mls_manager()?;
        let guard = manager
            .read()
            .map_err(|_| ProtocolError::Other("MLS manager lock poisoned".to_string()))?;
        guard
            .import_key_package(&user_id, &key_package_data)
            .map_err(|e| ProtocolError::MlsError(e.to_string()))
    }

    /// Get pending key packages
    pub fn mls_get_pending_key_packages(&self) -> Vec<MlsKeyPackageBundle> {
        let manager = match self.get_mls_manager() {
            Ok(m) => m,
            Err(_) => return Vec::new(),
        };
        let guard = match manager.read() {
            Ok(g) => g,
            Err(_) => return Vec::new(),
        };
        guard
            .get_pending_key_packages()
            .unwrap_or_default()
            .into_iter()
            .map(MlsKeyPackageBundle::from)
            .collect()
    }

    /// Mark a key package as synced
    pub fn mls_mark_key_package_synced(&self, package_id: String) -> Result<(), ProtocolError> {
        let manager = self.get_mls_manager()?;
        let guard = manager
            .read()
            .map_err(|_| ProtocolError::Other("MLS manager lock poisoned".to_string()))?;
        guard
            .mark_key_package_synced(&package_id)
            .map_err(|e| ProtocolError::MlsError(e.to_string()))
    }

    /// Check if a 1:1 session exists
    pub fn mls_has_session(&self, other_user_id: String) -> bool {
        let manager = match self.get_mls_manager() {
            Ok(m) => m,
            Err(_) => return false,
        };
        let guard = match manager.read() {
            Ok(g) => g,
            Err(_) => return false,
        };
        guard.has_session(&other_user_id).unwrap_or(false)
    }

    /// Check if a pending key package is available for a peer
    pub fn has_pending_key_package(&self, peer_id: String) -> bool {
        let guard = match self.inner.lock() {
            Ok(g) => g,
            Err(_) => {
                return false;
            }
        };
        guard.has_pending_key_package(&peer_id)
    }

    /// Returns the current establishment state for a peer.
    pub fn get_establishment_state(
        &self,
        peer_id: String,
    ) -> Result<EstablishmentState, ProtocolError> {
        let guard = self
            .inner
            .lock()
            .map_err(|_| ProtocolError::Other("Protocol lock poisoned".to_string()))?;
        guard
            .get_establishment_state(&peer_id)
            .map(Into::into)
            .map_err(ProtocolError::from)
    }

    /// Establish a secure session with a peer (high-level API)
    ///
    /// This method handles the complete session establishment flow:
    /// - If session already exists, returns None
    /// - If a pending key package is available, imports it, creates session, sends Welcome
    /// - If no key package is available, returns SessionNotReady(state) so caller can retry
    pub fn establish_secure_session(
        &self,
        peer_id: String,
    ) -> Result<Option<MlsWelcomeMessage>, ProtocolError> {
        let mut guard = self
            .inner
            .lock()
            .map_err(|_| ProtocolError::Other("Protocol lock poisoned".to_string()))?;

        guard
            .establish_secure_session(&peer_id)
            .map(|opt| opt.map(MlsWelcomeMessage::from))
            .map_err(ProtocolError::from)
    }

    /// Create a 1:1 session
    pub fn mls_create_session(
        &self,
        other_user_id: String,
    ) -> Result<MlsWelcomeMessage, ProtocolError> {
        let mut guard = self
            .inner
            .lock()
            .map_err(|_| ProtocolError::Other("Protocol lock poisoned".to_string()))?;
        guard
            .manual_mls_create_session(&other_user_id)
            .map(MlsWelcomeMessage::from)
            .map_err(|e| ProtocolError::MlsError(e.to_string()))
    }

    /// Join a session using a Welcome message
    pub fn mls_join_session(
        &self,
        welcome: MlsWelcomeMessage,
    ) -> Result<MlsGroupInfo, ProtocolError> {
        let core_welcome: CoreWelcomeMessage = welcome.into();
        let mut guard = self
            .inner
            .lock()
            .map_err(|_| ProtocolError::Other("Protocol lock poisoned".to_string()))?;
        guard
            .manual_mls_join_session(&core_welcome)
            .map(MlsGroupInfo::from)
            .map_err(|e| ProtocolError::MlsError(e.to_string()))
    }

    /// Encrypt a message for a 1:1 session
    pub fn mls_encrypt_for_user(
        &self,
        other_user_id: String,
        plaintext: Vec<u8>,
    ) -> Result<MlsEncryptedMessage, ProtocolError> {
        let manager = self.get_mls_manager()?;
        let guard = manager
            .read()
            .map_err(|_| ProtocolError::Other("MLS manager lock poisoned".to_string()))?;
        guard
            .encrypt_for_user(&other_user_id, &plaintext)
            .map(MlsEncryptedMessage::from)
            .map_err(|e| ProtocolError::MlsError(e.to_string()))
    }

    /// Decrypt a message from a 1:1 session
    pub fn mls_decrypt_from_user(
        &self,
        encrypted: MlsEncryptedMessage,
    ) -> Result<Option<Vec<u8>>, ProtocolError> {
        let core_encrypted: CoreEncryptedMessage = encrypted.into();
        let mut guard = self
            .inner
            .lock()
            .map_err(|_| ProtocolError::Other("Protocol lock poisoned".to_string()))?;
        guard
            .manual_mls_decrypt_from_user(&core_encrypted)
            .map_err(|e| ProtocolError::MlsError(e.to_string()))
    }

    /// List all active 1:1 sessions
    pub fn mls_list_sessions(&self) -> Vec<String> {
        let manager = match self.get_mls_manager() {
            Ok(m) => m,
            Err(_) => return Vec::new(),
        };
        let guard = match manager.read() {
            Ok(g) => g,
            Err(_) => return Vec::new(),
        };
        guard.list_sessions().unwrap_or_default()
    }

    /// Delete a 1:1 session
    pub fn mls_delete_session(&self, other_user_id: String) -> Result<(), ProtocolError> {
        let mut guard = self
            .inner
            .lock()
            .map_err(|_| ProtocolError::Other("Protocol lock poisoned".to_string()))?;
        guard
            .manual_mls_delete_session(&other_user_id)
            .map_err(|e| ProtocolError::MlsError(e.to_string()))
    }

    /// Get a pending Welcome message
    pub fn mls_get_pending_welcome(&self, other_user_id: String) -> Option<MlsWelcomeMessage> {
        let manager = self.get_mls_manager().ok()?;
        let guard = manager.read().ok()?;
        guard
            .get_pending_welcome(&other_user_id)
            .ok()
            .flatten()
            .map(MlsWelcomeMessage::from)
    }

    /// Clear a pending Welcome message
    pub fn mls_clear_pending_welcome(&self, other_user_id: String) -> Result<(), ProtocolError> {
        let manager = self.get_mls_manager()?;
        let guard = manager
            .read()
            .map_err(|_| ProtocolError::Other("MLS manager lock poisoned".to_string()))?;
        guard
            .clear_pending_welcome(&other_user_id)
            .map_err(|e| ProtocolError::MlsError(e.to_string()))
    }

    /// Create a new group
    pub fn mls_create_group(&self, group_name: String) -> Result<MlsGroupInfo, ProtocolError> {
        let manager = self.get_mls_manager()?;
        let guard = manager
            .read()
            .map_err(|_| ProtocolError::Other("MLS manager lock poisoned".to_string()))?;
        guard
            .create_group(&group_name)
            .map(MlsGroupInfo::from)
            .map_err(|e| ProtocolError::MlsError(e.to_string()))
    }

    /// Add a member to a group.
    ///
    /// Returns both the Welcome (for the invitee) and the Commit (to distribute
    /// to existing members so they advance their MLS epoch).
    pub fn mls_add_group_member(
        &self,
        group_id: String,
        member_key_package: Vec<u8>,
    ) -> Result<MlsAddMemberResult, ProtocolError> {
        let manager = self.get_mls_manager()?;
        let guard = manager
            .read()
            .map_err(|_| ProtocolError::Other("MLS manager lock poisoned".to_string()))?;
        guard
            .add_group_member(&CoreGroupId::new(group_id), &member_key_package)
            .map(|(welcome, commit)| MlsAddMemberResult {
                welcome: MlsWelcomeMessage::from(welcome),
                commit: MlsEncryptedMessage::from(commit),
            })
            .map_err(|e| ProtocolError::MlsError(e.to_string()))
    }

    /// Remove a member from a group
    pub fn mls_remove_group_member(
        &self,
        group_id: String,
        member_id: String,
    ) -> Result<MlsEncryptedMessage, ProtocolError> {
        let manager = self.get_mls_manager()?;
        let guard = manager
            .read()
            .map_err(|_| ProtocolError::Other("MLS manager lock poisoned".to_string()))?;
        guard
            .remove_group_member(&CoreGroupId::new(group_id), &member_id)
            .map(MlsEncryptedMessage::from)
            .map_err(|e| ProtocolError::MlsError(e.to_string()))
    }

    /// Leave a group
    pub fn mls_leave_group(&self, group_id: String) -> Result<(), ProtocolError> {
        let manager = self.get_mls_manager()?;
        let guard = manager
            .read()
            .map_err(|_| ProtocolError::Other("MLS manager lock poisoned".to_string()))?;
        guard
            .leave_group(&CoreGroupId::new(group_id))
            .map_err(|e| ProtocolError::MlsError(e.to_string()))
    }

    /// Encrypt a message for a group
    pub fn mls_encrypt_for_group(
        &self,
        group_id: String,
        plaintext: Vec<u8>,
    ) -> Result<MlsEncryptedMessage, ProtocolError> {
        let manager = self.get_mls_manager()?;
        let guard = manager
            .read()
            .map_err(|_| ProtocolError::Other("MLS manager lock poisoned".to_string()))?;
        guard
            .encrypt_for_group(&CoreGroupId::new(group_id), &plaintext)
            .map(MlsEncryptedMessage::from)
            .map_err(|e| ProtocolError::MlsError(e.to_string()))
    }

    /// Decrypt a message from a group
    pub fn mls_decrypt_from_group(
        &self,
        encrypted: MlsEncryptedMessage,
    ) -> Result<Option<Vec<u8>>, ProtocolError> {
        let manager = self.get_mls_manager()?;
        let guard = manager
            .read()
            .map_err(|_| ProtocolError::Other("MLS manager lock poisoned".to_string()))?;
        guard
            .decrypt_from_group(&encrypted.into())
            .map_err(|e| ProtocolError::MlsError(e.to_string()))
    }

    /// Join a group using a Welcome message
    pub fn mls_join_group(
        &self,
        welcome: MlsWelcomeMessage,
    ) -> Result<MlsGroupInfo, ProtocolError> {
        let manager = self.get_mls_manager()?;
        let guard = manager
            .read()
            .map_err(|_| ProtocolError::Other("MLS manager lock poisoned".to_string()))?;
        guard
            .join_group(&welcome.into())
            .map(MlsGroupInfo::from)
            .map_err(|e| ProtocolError::MlsError(e.to_string()))
    }

    /// List all groups
    pub fn mls_list_groups(&self) -> Vec<String> {
        let manager = match self.get_mls_manager() {
            Ok(m) => m,
            Err(_) => return Vec::new(),
        };
        let guard = match manager.read() {
            Ok(g) => g,
            Err(_) => return Vec::new(),
        };
        guard
            .list_groups()
            .unwrap_or_default()
            .into_iter()
            .map(|g| g.as_str().to_string())
            .collect()
    }

    /// Get information about a group
    pub fn mls_get_group_info(&self, group_id: String) -> Option<MlsGroupInfo> {
        let manager = self.get_mls_manager().ok()?;
        let guard = manager.read().ok()?;
        guard
            .get_group_info(&CoreGroupId::new(group_id))
            .ok()
            .flatten()
            .map(MlsGroupInfo::from)
    }

    /// Decrypt any encrypted message
    pub fn mls_decrypt(
        &self,
        encrypted: MlsEncryptedMessage,
    ) -> Result<Option<Vec<u8>>, ProtocolError> {
        let core_encrypted: CoreEncryptedMessage = encrypted.into();
        let mut guard = self
            .inner
            .lock()
            .map_err(|_| ProtocolError::Other("Protocol lock poisoned".to_string()))?;
        guard
            .manual_mls_decrypt(&core_encrypted)
            .map_err(|e| ProtocolError::MlsError(e.to_string()))
    }

    /// Process a Welcome message
    pub fn mls_process_welcome(
        &self,
        welcome: MlsWelcomeMessage,
    ) -> Result<MlsGroupInfo, ProtocolError> {
        let core_welcome: CoreWelcomeMessage = welcome.into();
        let mut guard = self
            .inner
            .lock()
            .map_err(|_| ProtocolError::Other("Protocol lock poisoned".to_string()))?;
        guard
            .manual_mls_process_welcome(&core_welcome)
            .map(MlsGroupInfo::from)
            .map_err(|e| ProtocolError::MlsError(e.to_string()))
    }

    // ========================================================================
    // IDENTITY AND SIGNING OPERATIONS
    // ========================================================================

    /// Get the identity public key (Ed25519, 32 bytes).
    ///
    /// This is the public key used for MLS operations and can be shared with others
    /// to establish identity and verify signatures.
    pub fn get_identity_public_key(&self) -> Result<Vec<u8>, ProtocolError> {
        let manager = self.get_mls_manager()?;
        let guard = manager
            .read()
            .map_err(|_| ProtocolError::Other("MLS manager lock poisoned".to_string()))?;
        guard
            .get_identity_public_key()
            .map_err(|e| ProtocolError::MlsError(e.to_string()))
    }

    /// Derive a deterministic user ID from a public key.
    ///
    /// Returns a base58-encoded string derived from SHA-256(publicKey)[0:20].
    /// The same public key always produces the same user ID.
    pub fn derive_user_id_from_public_key(&self, public_key: Vec<u8>) -> String {
        CoreMlsManager::derive_user_id_from_public_key(&public_key)
    }

    /// Sign arbitrary data with the identity private key (Ed25519).
    ///
    /// Returns the signature as raw bytes (64 bytes).
    pub fn sign_data(&self, data: Vec<u8>) -> Result<Vec<u8>, ProtocolError> {
        let manager = self.get_mls_manager()?;
        let guard = manager
            .read()
            .map_err(|_| ProtocolError::Other("MLS manager lock poisoned".to_string()))?;
        guard
            .sign_data(&data)
            .map_err(|e| ProtocolError::MlsError(e.to_string()))
    }

    /// Verify a signature against a public key.
    ///
    /// Returns true if the signature is valid, false otherwise.
    pub fn verify_signature(
        &self,
        public_key: Vec<u8>,
        data: Vec<u8>,
        signature: Vec<u8>,
    ) -> Result<bool, ProtocolError> {
        CoreMlsManager::verify_signature(&public_key, &data, &signature)
            .map_err(|e| ProtocolError::MlsError(e.to_string()))
    }

    // ========================================================================
    // PRESENCE AND KEY MANAGEMENT (RELAY SERVER API)
    // ========================================================================

    /// Check if a user is online.
    /// Returns JSON string to send via WebSocket relay.
    pub fn check_presence(&self, username: String) -> Result<String, ProtocolError> {
        let payload = serde_json::json!({
            "type": "CheckPresence",
            "username": username
        });
        serde_json::to_string(&payload)
            .map_err(|e| ProtocolError::Other(format!("Failed to serialize CheckPresence: {}", e)))
    }

    /// Request prekey bundle for a user to establish encrypted communication.
    /// Returns JSON string to send via WebSocket relay.
    pub fn request_prekey_bundle(&self, username: String) -> Result<String, ProtocolError> {
        let payload = serde_json::json!({
            "type": "RequestPreKeyBundle",
            "username": username
        });
        serde_json::to_string(&payload).map_err(|e| {
            ProtocolError::Other(format!("Failed to serialize RequestPreKeyBundle: {}", e))
        })
    }

    /// Upload identity key and prekeys for Signal Protocol.
    /// Returns JSON string to send via WebSocket relay.
    /// Parameters are JSON strings that will be parsed and included in the payload.
    pub fn upload_keys(
        &self,
        identity_key: String,
        signed_prekey_json: String,
        one_time_prekeys_json: String,
    ) -> Result<String, ProtocolError> {
        // Parse the JSON strings into values
        let signed_prekey: serde_json::Value =
            serde_json::from_str(&signed_prekey_json).map_err(|e| {
                ProtocolError::Other(format!("Failed to parse signed_prekey JSON: {}", e))
            })?;

        let one_time_prekeys: Vec<serde_json::Value> = serde_json::from_str(&one_time_prekeys_json)
            .map_err(|e| {
                ProtocolError::Other(format!("Failed to parse one_time_prekeys JSON: {}", e))
            })?;

        let payload = serde_json::json!({
            "type": "UploadKeys",
            "identity_key": identity_key,
            "signed_prekey": signed_prekey,
            "one_time_prekeys": one_time_prekeys
        });
        serde_json::to_string(&payload)
            .map_err(|e| ProtocolError::Other(format!("Failed to serialize UploadKeys: {}", e)))
    }

    /// Set typing indicator in a conversation.
    /// For direct messages, conversation_id should be the recipient's username.
    /// For group chats, conversation_id should be the group_id.
    /// Returns JSON string to send via WebSocket relay.
    pub fn set_typing(&self, conversation_id: String) -> Result<String, ProtocolError> {
        let payload = serde_json::json!({
            "type": "SetTyping",
            "conversation_id": conversation_id
        });
        serde_json::to_string(&payload)
            .map_err(|e| ProtocolError::Other(format!("Failed to serialize SetTyping: {}", e)))
    }

    /// Clear typing indicator in a conversation.
    /// For direct messages, conversation_id should be the recipient's username.
    /// For group chats, conversation_id should be the group_id.
    /// Returns JSON string to send via WebSocket relay.
    pub fn clear_typing(&self, conversation_id: String) -> Result<String, ProtocolError> {
        let payload = serde_json::json!({
            "type": "ClearTyping",
            "conversation_id": conversation_id
        });
        serde_json::to_string(&payload)
            .map_err(|e| ProtocolError::Other(format!("Failed to serialize ClearTyping: {}", e)))
    }

    // ========================================================================
    // GROUP MESSAGING (MLS-encrypted, transport-agnostic)
    // ========================================================================

    /// Create a new MLS group.
    pub fn create_group(&self, group_name: String) -> Result<MlsGroupInfo, ProtocolError> {
        let mut guard = self
            .inner
            .lock()
            .map_err(|_| ProtocolError::Other("Protocol lock poisoned".to_string()))?;
        guard
            .create_group(&group_name)
            .map(MlsGroupInfo::from)
            .map_err(|e| ProtocolError::Other(e.to_string()))
    }

    /// Send an MLS-encrypted message to all group members.
    pub fn send_group_message(
        &self,
        group_id: String,
        content: String,
        priority: Option<MessagePriority>,
        reply_to_msg: Option<String>,
    ) -> Result<Vec<String>, ProtocolError> {
        let core_priority = priority.map(|p| match p {
            MessagePriority::Low => CorePriority::Low,
            MessagePriority::Medium => CorePriority::Medium,
            MessagePriority::High => CorePriority::High,
            MessagePriority::Critical => CorePriority::Critical,
        });
        let mut guard = self
            .inner
            .lock()
            .map_err(|_| ProtocolError::Other("Protocol lock poisoned".to_string()))?;
        guard
            .send_group_message(&group_id, &content, core_priority, reply_to_msg.as_deref())
            .map(|ids| ids.into_iter().map(|id| id.as_str().to_string()).collect())
            .map_err(|e| ProtocolError::SendFailed(e.to_string()))
    }

    /// Invite a user to an MLS group.
    pub fn invite_to_group(
        &self,
        group_id: String,
        invitee_user_id: String,
    ) -> Result<(), ProtocolError> {
        let mut guard = self
            .inner
            .lock()
            .map_err(|_| ProtocolError::Other("Protocol lock poisoned".to_string()))?;
        guard
            .invite_to_group(&group_id, &invitee_user_id)
            .map_err(|e| ProtocolError::Other(e.to_string()))
    }

    /// Remove a member from an MLS group.
    pub fn remove_from_group(
        &self,
        group_id: String,
        member_id: String,
    ) -> Result<(), ProtocolError> {
        let mut guard = self
            .inner
            .lock()
            .map_err(|_| ProtocolError::Other("Protocol lock poisoned".to_string()))?;
        guard
            .remove_from_group(&group_id, &member_id)
            .map_err(|e| ProtocolError::Other(e.to_string()))
    }

    /// Leave an MLS group.
    pub fn leave_group(&self, group_id: String) -> Result<(), ProtocolError> {
        let mut guard = self
            .inner
            .lock()
            .map_err(|_| ProtocolError::Other("Protocol lock poisoned".to_string()))?;
        guard
            .leave_group(&group_id)
            .map_err(|e| ProtocolError::Other(e.to_string()))
    }

    /// List all MLS groups (excluding 1:1 sessions).
    pub fn list_groups(&self) -> Result<Vec<String>, ProtocolError> {
        let guard = self
            .inner
            .lock()
            .map_err(|_| ProtocolError::Other("Protocol lock poisoned".to_string()))?;
        guard
            .list_groups()
            .map_err(|e| ProtocolError::Other(e.to_string()))
    }

    // ========================================================================
    // GROUP MANAGEMENT (RELAY SERVER API) — DEPRECATED
    // Use the canonical group APIs above instead.
    // ========================================================================

    /// Create a new group. Creator becomes admin.
    /// Returns JSON string to send via WebSocket relay.
    pub fn group_create(&self, name: String) -> Result<String, ProtocolError> {
        let payload = serde_json::json!({
            "type": "CreateGroup",
            "name": name
        });
        serde_json::to_string(&payload)
            .map_err(|e| ProtocolError::Other(format!("Failed to serialize CreateGroup: {}", e)))
    }

    /// Send encrypted message to a group. Content must be pre-encrypted by client.
    /// Returns JSON string to send via WebSocket relay.
    pub fn group_send_message(
        &self,
        group_id: String,
        content: String,
        reply_to_msg: Option<String>,
    ) -> Result<String, ProtocolError> {
        let mut payload = serde_json::json!({
            "type": "SendGroupMessage",
            "group_id": group_id,
            "content": content
        });

        if let Some(reply_to) = reply_to_msg {
            payload["reply_to_msg"] = serde_json::Value::String(reply_to);
        }

        serde_json::to_string(&payload).map_err(|e| {
            ProtocolError::Other(format!("Failed to serialize SendGroupMessage: {}", e))
        })
    }

    /// Add member to group. Admin only.
    /// Returns JSON string to send via WebSocket relay.
    pub fn group_add_member(
        &self,
        group_id: String,
        username: String,
    ) -> Result<String, ProtocolError> {
        let payload = serde_json::json!({
            "type": "AddGroupMember",
            "group_id": group_id,
            "username": username
        });
        serde_json::to_string(&payload)
            .map_err(|e| ProtocolError::Other(format!("Failed to serialize AddGroupMember: {}", e)))
    }

    /// Remove member from group. Admin only, or user can remove themselves.
    /// Returns JSON string to send via WebSocket relay.
    pub fn group_remove_member(
        &self,
        group_id: String,
        username: String,
    ) -> Result<String, ProtocolError> {
        let payload = serde_json::json!({
            "type": "RemoveGroupMember",
            "group_id": group_id,
            "username": username
        });
        serde_json::to_string(&payload).map_err(|e| {
            ProtocolError::Other(format!("Failed to serialize RemoveGroupMember: {}", e))
        })
    }

    /// Set member as admin. Admin only.
    /// Returns JSON string to send via WebSocket relay.
    pub fn group_set_admin(
        &self,
        group_id: String,
        username: String,
    ) -> Result<String, ProtocolError> {
        let payload = serde_json::json!({
            "type": "SetGroupAdmin",
            "group_id": group_id,
            "username": username
        });
        serde_json::to_string(&payload)
            .map_err(|e| ProtocolError::Other(format!("Failed to serialize SetGroupAdmin: {}", e)))
    }

    /// Remove admin role from member. Admin only.
    /// Returns JSON string to send via WebSocket relay.
    pub fn group_remove_admin(
        &self,
        group_id: String,
        username: String,
    ) -> Result<String, ProtocolError> {
        let payload = serde_json::json!({
            "type": "RemoveGroupAdmin",
            "group_id": group_id,
            "username": username
        });
        serde_json::to_string(&payload).map_err(|e| {
            ProtocolError::Other(format!("Failed to serialize RemoveGroupAdmin: {}", e))
        })
    }

    /// Leave a group.
    /// Returns JSON string to send via WebSocket relay.
    pub fn group_leave(&self, group_id: String) -> Result<String, ProtocolError> {
        let payload = serde_json::json!({
            "type": "LeaveGroup",
            "group_id": group_id
        });
        serde_json::to_string(&payload)
            .map_err(|e| ProtocolError::Other(format!("Failed to serialize LeaveGroup: {}", e)))
    }

    /// Delete a group. Admin only.
    /// Returns JSON string to send via WebSocket relay.
    pub fn group_delete(&self, group_id: String) -> Result<String, ProtocolError> {
        let payload = serde_json::json!({
            "type": "DeleteGroup",
            "group_id": group_id
        });
        serde_json::to_string(&payload)
            .map_err(|e| ProtocolError::Other(format!("Failed to serialize DeleteGroup: {}", e)))
    }

    /// Get group information.
    /// Returns JSON string to send via WebSocket relay.
    pub fn group_get_info(&self, group_id: String) -> Result<String, ProtocolError> {
        let payload = serde_json::json!({
            "type": "GetGroupInfo",
            "group_id": group_id
        });
        serde_json::to_string(&payload)
            .map_err(|e| ProtocolError::Other(format!("Failed to serialize GetGroupInfo: {}", e)))
    }

    /// Get all groups the user is a member of.
    /// Returns JSON string to send via WebSocket relay.
    pub fn group_get_user_groups(&self) -> Result<String, ProtocolError> {
        let payload = serde_json::json!({
            "type": "GetUserGroups"
        });
        serde_json::to_string(&payload)
            .map_err(|e| ProtocolError::Other(format!("Failed to serialize GetUserGroups: {}", e)))
    }
}

/// Standalone mesh services interface for UniFFI.
///
/// Holds an `Arc<OfflineProtocol>` and delegates through public wrapper methods,
/// avoiding direct exposure of internal synchronization primitives.
pub struct MeshServices {
    protocol: Arc<OfflineProtocol>,
}

impl MeshServices {
    /// Creates a MeshServices instance sharing the given protocol's state.
    pub fn new(protocol: Arc<OfflineProtocol>) -> Result<Self, ProtocolError> {
        Ok(Self { protocol })
    }

    /// Registers a local service that this node offers for discovery.
    pub fn register_service(
        &self,
        service_id: String,
        version: String,
        capabilities: HashMap<String, String>,
    ) -> Result<(), ProtocolError> {
        self.protocol
            .svc_register_service(service_id, version, capabilities)
    }

    /// Unregisters a local service. Returns true if found and removed.
    pub fn unregister_service(&self, service_id: String) -> Result<bool, ProtocolError> {
        self.protocol.svc_unregister_service(service_id)
    }

    /// Broadcasts a service discovery query. Returns a query_id.
    pub fn discover_services(&self, service_id: Option<String>) -> Result<String, ProtocolError> {
        self.protocol.svc_discover_services(service_id)
    }

    /// Sends a service request to a specific provider peer. Returns a request_id.
    pub fn send_service_request(
        &self,
        provider: String,
        service_id: String,
        method: String,
        body: String,
    ) -> Result<String, ProtocolError> {
        self.protocol
            .svc_send_service_request(provider, service_id, method, body)
    }

    /// Responds to a service request from another peer.
    pub fn respond_to_service_request(
        &self,
        request_id: String,
        requester: String,
        service_id: String,
        status: String,
        body: String,
    ) -> Result<String, ProtocolError> {
        self.protocol
            .svc_respond_to_service_request(request_id, requester, service_id, status, body)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::HashMap;
    use std::sync::Arc;
    use std::thread;

    #[derive(Default)]
    struct TestMlsStorageProvider {
        data: Mutex<HashMap<(String, String), Vec<u8>>>,
    }

    impl MlsStorageProvider for TestMlsStorageProvider {
        fn store(
            &self,
            key_type: String,
            key_id: String,
            data: Vec<u8>,
        ) -> Result<(), MlsStorageError> {
            let mut guard = self.data.lock().map_err(|_| MlsStorageError::StoreFailed)?;
            guard.insert((key_type, key_id), data);
            Ok(())
        }

        fn load(
            &self,
            key_type: String,
            key_id: String,
        ) -> Result<Option<Vec<u8>>, MlsStorageError> {
            let guard = self.data.lock().map_err(|_| MlsStorageError::LoadFailed)?;
            Ok(guard.get(&(key_type, key_id)).cloned())
        }

        fn delete(&self, key_type: String, key_id: String) -> Result<(), MlsStorageError> {
            let mut guard = self
                .data
                .lock()
                .map_err(|_| MlsStorageError::DeleteFailed)?;
            guard.remove(&(key_type, key_id));
            Ok(())
        }

        fn list_keys(&self, key_type: String) -> Result<Vec<String>, MlsStorageError> {
            let guard = self.data.lock().map_err(|_| MlsStorageError::LoadFailed)?;
            Ok(guard
                .keys()
                .filter_map(|(stored_type, key_id)| {
                    if stored_type == &key_type {
                        Some(key_id.clone())
                    } else {
                        None
                    }
                })
                .collect())
        }
    }

    fn create_test_config() -> ProtocolConfig {
        ProtocolConfig {
            app_id: "test-app".to_string(),
            user_id: "user123".to_string(),
            ble_enabled: true,
            wifi_direct_enabled: true,
            internet_enabled: true,
            prefer_online: false,
            initial_ttl: 8,
            encryption_enabled: true,
            auto_key_exchange: true,
            store_pending: true,
            require_encryption: false,
            max_pending_per_peer: 64,
            max_pending_global: 4096,
            pending_ttl_ms: 120_000,
            overflow_policy: OverflowPolicy::DropOldest,
            max_group_members: 256,
            group_relay_enabled: true,
        }
    }

    fn create_ble_only_config() -> ProtocolConfig {
        ProtocolConfig {
            app_id: "test-app".to_string(),
            user_id: "user123".to_string(),
            ble_enabled: true,
            wifi_direct_enabled: false,
            internet_enabled: false,
            prefer_online: false,
            initial_ttl: 8,
            encryption_enabled: true,
            auto_key_exchange: true,
            store_pending: true,
            require_encryption: false,
            max_pending_per_peer: 64,
            max_pending_global: 4096,
            pending_ttl_ms: 120_000,
            overflow_policy: OverflowPolicy::DropOldest,
            max_group_members: 256,
            group_relay_enabled: true,
        }
    }

    #[test]
    fn test_mls_initialize_is_idempotent_for_legacy_entrypoint() {
        let config = create_test_config();
        let protocol = OfflineProtocol::new(config).unwrap();

        protocol
            .initialize_mls(Box::new(TestMlsStorageProvider::default()))
            .unwrap();
        let first_handle = {
            let guard = protocol.inner.lock().unwrap();
            guard.mls_manager().cloned().unwrap()
        };

        protocol
            .initialize_mls(Box::new(TestMlsStorageProvider::default()))
            .unwrap();
        let second_handle = {
            let guard = protocol.inner.lock().unwrap();
            guard.mls_manager().cloned().unwrap()
        };

        assert!(protocol.is_mls_initialized());
        assert!(Arc::ptr_eq(&first_handle, &second_handle));
        assert!(protocol.mls_generate_key_package().is_ok());
    }

    #[test]
    fn test_mls_initialize_is_race_safe_single_instance() {
        let protocol = Arc::new(OfflineProtocol::new(create_test_config()).unwrap());
        let mut join_handles = Vec::new();

        for _ in 0..8 {
            let protocol_clone = Arc::clone(&protocol);
            join_handles.push(thread::spawn(move || {
                protocol_clone
                    .initialize_mls(Box::new(TestMlsStorageProvider::default()))
                    .unwrap();
                let core_guard = protocol_clone.inner.lock().unwrap();
                let mls_handle = core_guard.mls_manager().cloned().unwrap();
                Arc::as_ptr(&mls_handle) as usize
            }));
        }

        let first_ptr = join_handles.remove(0).join().unwrap();
        for handle in join_handles {
            let ptr = handle.join().unwrap();
            assert_eq!(ptr, first_ptr);
        }
    }

    #[test]
    fn test_mls_manual_and_core_paths_share_single_state_under_concurrency() {
        let protocol = Arc::new(OfflineProtocol::new(create_test_config()).unwrap());
        protocol
            .initialize_mls(Box::new(TestMlsStorageProvider::default()))
            .unwrap();

        let core_mls_handle = {
            let core_guard = protocol.inner.lock().unwrap();
            core_guard.mls_manager().cloned().unwrap()
        };

        let manual_protocol = Arc::clone(&protocol);
        let manual_thread = thread::spawn(move || {
            for i in 0..20 {
                manual_protocol
                    .mls_create_group(format!("manual-group-{}", i))
                    .unwrap();
            }
        });

        let core_thread = thread::spawn(move || {
            for i in 0..20 {
                let manager = core_mls_handle.read().unwrap();
                manager.create_group(&format!("core-group-{}", i)).unwrap();
            }
        });

        manual_thread.join().unwrap();
        core_thread.join().unwrap();

        let from_manual_api = protocol.mls_list_groups();
        let from_core_api = {
            let core_guard = protocol.inner.lock().unwrap();
            let manager = core_guard.mls_manager().cloned().unwrap();
            let groups = manager
                .read()
                .unwrap()
                .list_groups()
                .unwrap()
                .into_iter()
                .map(|group_id| group_id.as_str().to_string())
                .collect::<Vec<_>>();
            groups
        };

        assert_eq!(from_manual_api.len(), from_core_api.len());
        for group_id in from_core_api {
            assert!(from_manual_api.contains(&group_id));
        }
    }

    #[test]
    fn test_protocol_creation() {
        let config = create_test_config();

        let protocol = OfflineProtocol::new(config);
        assert!(protocol.is_ok());
    }

    #[test]
    fn test_protocol_config_maps_pending_queue_settings_to_core() {
        let mut config = create_test_config();
        config.max_pending_per_peer = 11;
        config.max_pending_global = 99;
        config.pending_ttl_ms = 55_000;
        config.overflow_policy = OverflowPolicy::DropNewest;

        let core: CoreConfig = config.into();
        assert_eq!(core.encryption.pending_queue.max_pending_per_peer, 11);
        assert_eq!(core.encryption.pending_queue.max_pending_global, 99);
        assert_eq!(core.encryption.pending_queue.pending_ttl_ms, 55_000);
        assert_eq!(
            core.encryption.pending_queue.overflow_policy,
            CoreOverflowPolicy::DropNewest
        );
    }

    #[test]
    fn test_protocol_lifecycle() {
        let config = create_test_config();

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
        let config = create_test_config();

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
        let config = create_test_config();
        let protocol = OfflineProtocol::new(config).unwrap();

        let file_id = "file_test_001".to_string();

        assert!(protocol.get_file_progress(file_id.clone()).is_none());

        protocol
            .process_file_chunk(
                file_id.clone(),
                0,
                2,
                100,
                "test.txt".to_string(),
                "abc123".to_string(),
                vec![0u8; 50],
            )
            .unwrap();

        let progress = protocol.get_file_progress(file_id.clone());
        assert!(progress.is_some());
        let progress = progress.unwrap();
        assert_eq!(progress.chunks_sent, 1);
        assert_eq!(progress.total_chunks, 2);
        assert!(progress.percentage < 100);
    }

    #[test]
    fn test_gradient_routing_learn_and_query() {
        let config = create_ble_only_config();

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
            0,   // sequence_number (none from message)
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
        let config = create_ble_only_config();

        let protocol = OfflineProtocol::new(config).unwrap();

        // Learn multiple routes to the same destination
        protocol.learn_route("bob".to_string(), "peer1".to_string(), 3, 0.7, 0);
        protocol.learn_route("bob".to_string(), "peer2".to_string(), 2, 0.9, 0);
        protocol.learn_route("bob".to_string(), "peer3".to_string(), 1, 0.6, 0);

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
        let config = create_ble_only_config();

        let protocol = OfflineProtocol::new(config).unwrap();

        // Learn routes through peer1
        protocol.learn_route("alice".to_string(), "peer1".to_string(), 2, 0.8, 0);
        protocol.learn_route("bob".to_string(), "peer1".to_string(), 3, 0.7, 0);

        // Learn route through peer2
        protocol.learn_route("charlie".to_string(), "peer2".to_string(), 1, 0.9, 0);

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
        let config = create_ble_only_config();

        let protocol = OfflineProtocol::new(config).unwrap();

        // Initially empty
        let stats = protocol.get_routing_stats();
        assert_eq!(stats.destination_count, 0);
        assert_eq!(stats.route_count, 0);

        // Add some routes
        protocol.learn_route("alice".to_string(), "peer1".to_string(), 2, 0.8, 0);
        protocol.learn_route("alice".to_string(), "peer2".to_string(), 3, 0.6, 0);
        protocol.learn_route("bob".to_string(), "peer1".to_string(), 1, 0.9, 0);

        let stats = protocol.get_routing_stats();
        assert_eq!(stats.destination_count, 2); // alice and bob
        assert_eq!(stats.route_count, 3); // 2 routes to alice, 1 to bob
    }

    #[test]
    fn test_gradient_routing_config_update() {
        let config = create_ble_only_config();

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
