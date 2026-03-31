//! Constants used throughout the offline-protocol-transport crate.

// BLE Transport Constants
/// BLE service UUID for the Offline Protocol.
pub const BLE_SERVICE_UUID: &str = "6E400001-B5A3-F393-E0A9-E50E24DCCA9E";

/// BLE characteristic UUID for message data.
pub const BLE_MESSAGE_CHAR_UUID: &str = "6E400002-B5A3-F393-E0A9-E50E24DCCA9E";

/// BLE characteristic UUID for device ID.
pub const BLE_DEVICE_ID_CHAR_UUID: &str = "6E400003-B5A3-F393-E0A9-E50E24DCCA9E";

/// Maximum fragment size for BLE transmission (bytes).
/// This accounts for BLE MTU limitations and overhead.
pub const BLE_MAX_FRAGMENT_SIZE: usize = 185;

/// Timeout for fragment reassembly in seconds.
/// Fragments older than this are discarded.
pub const BLE_FRAGMENT_TIMEOUT_SECS: u64 = 30;

/// Maximum number of concurrent fragment assemblies.
/// Prevents unbounded memory growth from incomplete fragments.
pub const BLE_MAX_FRAGMENT_ASSEMBLIES: usize = 64;

/// Maximum number of fragments per message.
/// Prevents fragmentation attacks and memory exhaustion.
pub const BLE_MAX_FRAGMENT_COUNT: usize = 512;

/// Magic bytes identifying Offline Protocol fragments.
pub const FRAGMENT_MAGIC: [u8; 2] = *b"OP"; // Offline Protocol

/// Fragment protocol version.
pub const FRAGMENT_VERSION: u8 = 1;

/// Fixed size of fragment header in bytes.
/// Format: magic (2) + version (1) + id_len (1) + index (2) + total (2) + data_len (2)
pub const FRAGMENT_HEADER_FIXED: usize = 2 /*magic*/ + 1 /*version*/ + 1 /*id_len*/ + 2 /*index*/ + 2 /*total*/ + 2 /*data_len*/;

/// ATT overhead bytes for BLE MTU calculation.
pub const ATT_OVERHEAD_BYTES: usize = 3;

/// Heuristic send queue capacity for congestion calculation.
pub const HEURISTIC_SEND_CAPACITY: f32 = 50.0;

/// Weight for new latency values in EMA calculation.
pub const EMA_WEIGHT_NEW_LATENCY: f32 = 0.3;

/// Weight for existing latency values in EMA calculation.
pub const EMA_WEIGHT_EXISTING_LATENCY: f32 = 0.7;

// WiFi Direct Transport Constants
/// Maximum payload size for WiFi Direct transmission (bytes).
pub const WIFI_DIRECT_MAX_PAYLOAD_SIZE: usize = 65535;

/// Connection timeout for WiFi Direct in seconds.
pub const WIFI_DIRECT_CONNECTION_TIMEOUT_SECS: u64 = 30;

/// Default device name for Wi-Fi Direct.
pub const DEFAULT_DEVICE_NAME: &str = "OfflineProtocolDevice";

/// Default group owner intent for Wi-Fi Direct.
pub const DEFAULT_GROUP_OWNER_INTENT: u8 = 7;

// Internet Transport Constants
/// Default WebSocket server address for Internet transport.
pub const INTERNET_DEFAULT_SERVER_ADDRESS: &str = "ws://localhost:8080";

/// Connection timeout for Internet transport in seconds.
pub const INTERNET_CONNECTION_TIMEOUT_SECS: u64 = 30;

/// Heartbeat interval for Internet transport in seconds.
/// Used to keep the connection alive and detect disconnections.
pub const INTERNET_HEARTBEAT_INTERVAL_SECS: u64 = 30;

/// Timeout for pending Internet send confirmations in seconds.
/// Messages awaiting platform confirmation beyond this duration are treated as failed.
pub const INTERNET_PENDING_CONFIRMATION_TIMEOUT_SECS: u64 = 15;

// Reticulum Transport Constants
/// Connection timeout for reaching the Reticulum daemon (seconds).
pub const RETICULUM_CONNECTION_TIMEOUT_SECS: u64 = 60;

/// Timeout for pending Reticulum send confirmations (seconds).
/// Higher than Internet because Reticulum paths can be high-latency
/// (especially LoRa multi-hop).
pub const RETICULUM_PENDING_CONFIRMATION_TIMEOUT_SECS: u64 = 120;

/// Default maximum payload size for Reticulum (bytes).
/// Reticulum's MDU is 383 bytes per single packet, but larger payloads
/// are handled transparently by Reticulum's Resource mechanism.
pub const RETICULUM_MAX_PAYLOAD_SIZE: usize = 65536;

// Transport-wide Constants
/// Default maximum message size in bytes (1 MB).
/// Applied at the transport layer before JSON deserialization to prevent
/// memory exhaustion from oversized payloads.
pub const DEFAULT_MAX_MESSAGE_SIZE: usize = 1_048_576;
