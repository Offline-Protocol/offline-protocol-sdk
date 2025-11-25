//! Transport layer constants.

/// ATT overhead bytes for BLE MTU calculation.
pub const ATT_OVERHEAD_BYTES: usize = 3;

/// Heuristic send queue capacity for congestion calculation.
pub const HEURISTIC_SEND_CAPACITY: f32 = 50.0;

/// Weight for new latency values in EMA calculation.
pub const EMA_WEIGHT_NEW_LATENCY: f32 = 0.3;

/// Weight for existing latency values in EMA calculation.
pub const EMA_WEIGHT_EXISTING_LATENCY: f32 = 0.7;

/// Default device name for Wi-Fi Direct.
pub const DEFAULT_DEVICE_NAME: &str = "OfflineProtocolDevice";

/// Default group owner intent for Wi-Fi Direct.
pub const DEFAULT_GROUP_OWNER_INTENT: u8 = 7;

