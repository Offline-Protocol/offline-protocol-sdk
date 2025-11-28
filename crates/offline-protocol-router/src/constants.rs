//! Router layer constants.

/// Number of top relays to forward to for redundancy.
pub const DEFAULT_FORWARD_TO_TOP_K: usize = 3;

// Adaptive TTL constants

/// Base TTL for small networks (< 50 devices).
pub const ADAPTIVE_TTL_BASE: u8 = 8;

/// Additional TTL per 100 devices in the network.
pub const ADAPTIVE_TTL_PER_100_DEVICES: u8 = 2;

/// Maximum TTL to prevent infinite propagation.
pub const ADAPTIVE_TTL_MAX: u8 = 24;

/// Minimum TTL to ensure basic propagation.
pub const ADAPTIVE_TTL_MIN: u8 = 4;

/// TTL boost for messages that have been queued due to congestion.
pub const ADAPTIVE_TTL_CONGESTION_BOOST: u8 = 2;

/// Network size threshold for small networks (no adaptive TTL needed).
pub const ADAPTIVE_TTL_SMALL_NETWORK_THRESHOLD: usize = 50;

// Gossip forwarding constants

/// Target number of peers to forward messages to in large networks.
pub const DEFAULT_GOSSIP_TARGET_FANOUT: usize = 4;

/// Minimum forwarding probability to ensure message propagation.
pub const DEFAULT_GOSSIP_MIN_PROBABILITY: f32 = 0.15;

/// Peer count threshold below which we use full flooding instead of gossip.
pub const DEFAULT_GOSSIP_SMALL_NETWORK_THRESHOLD: usize = 10;

/// Maximum acceptable congestion level.
pub const DEFAULT_MAX_CONGESTION_LEVEL: f32 = 0.7;

/// Weight for signal strength in path scoring.
pub const SIGNAL_WEIGHT: f32 = 0.3;

/// Weight for proximity in path scoring.
pub const PROXIMITY_WEIGHT: f32 = 0.2;

/// Weight for capacity in path scoring.
pub const CAPACITY_WEIGHT: f32 = 0.3;

/// Weight for energy in path scoring.
pub const ENERGY_WEIGHT: f32 = 0.2;

/// RSSI threshold for excellent signal.
pub const EXCELLENT_RSSI_THRESHOLD: i16 = -50;

/// RSSI threshold for good signal.
pub const GOOD_RSSI_THRESHOLD: i16 = -70;

/// RSSI threshold for fair signal.
pub const FAIR_RSSI_THRESHOLD: i16 = -85;

/// Maximum RSSI for poor signal.
pub const POOR_RSSI_MAX: i16 = -100;

/// Signal score for excellent RSSI.
pub const EXCELLENT_SIGNAL_SCORE: f32 = 100.0;

/// Base score for good signal.
pub const GOOD_SIGNAL_BASE: f32 = 70.0;

/// Base score for fair signal.
pub const FAIR_SIGNAL_BASE: f32 = 40.0;

/// Assumed capacity for non-relay devices.
pub const NON_RELAY_BASIC_CAPACITY: f32 = 50.0;

/// Assumed battery level for non-relay devices.
pub const NON_RELAY_ASSUMED_BATTERY: f32 = 70.0;

/// Battery score for charging devices.
pub const CHARGING_BATTERY_SCORE: f32 = 100.0;
