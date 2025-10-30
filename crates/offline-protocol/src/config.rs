//! Configuration structures for the SDK

use serde::{Deserialize, Serialize};

/// Complete configuration for the Offline Protocol SDK
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct OfflineProtocolConfig {
    /// Application identifier
    pub app_id: String,
    
    /// Username for this device
    pub username: String,
    
    /// Transport configuration
    #[serde(default)]
    pub transports: TransportsConfig,
    
    /// Network parameters
    #[serde(default)]
    pub network: NetworkConfig,
    
    /// DORS configuration
    #[serde(default)]
    pub dors: DorsConfig,
    
    /// Relay configuration
    #[serde(default)]
    pub relay: RelayConfig,
    
    /// Reliability settings
    #[serde(default)]
    pub reliability: ReliabilityConfig,
}

/// Transport configuration
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct TransportsConfig {
    /// BLE transport configuration
    #[serde(default)]
    pub ble: BleConfig,
    
    /// Wi-Fi Direct transport configuration
    #[serde(default)]
    pub wifi_direct: WiFiDirectConfig,
}

impl Default for TransportsConfig {
    fn default() -> Self {
        Self {
            ble: BleConfig::default(),
            wifi_direct: WiFiDirectConfig::default(),
        }
    }
}

/// BLE transport configuration
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct BleConfig {
    /// Enable BLE transport
    #[serde(default = "default_true")]
    pub enabled: bool,
    
    /// Scan interval in milliseconds
    #[serde(default = "default_scan_interval")]
    pub scan_interval_ms: u64,
    
    /// Advertising/beacon interval in milliseconds
    #[serde(default = "default_beacon_interval")]
    pub advertising_interval_ms: u64,
}

impl Default for BleConfig {
    fn default() -> Self {
        Self {
            enabled: true,
            scan_interval_ms: 5000,
            advertising_interval_ms: 5000,
        }
    }
}

/// Wi-Fi Direct transport configuration
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct WiFiDirectConfig {
    /// Enable Wi-Fi Direct transport
    #[serde(default)]
    pub enabled: bool,
    
    /// Automatically switch to Wi-Fi Direct when needed
    #[serde(default = "default_true")]
    pub auto_switch: bool,
    
    /// Group owner intent (0-15, higher = more likely to be owner)
    #[serde(default = "default_group_owner_intent")]
    pub group_owner_intent: u8,
}

impl Default for WiFiDirectConfig {
    fn default() -> Self {
        Self {
            enabled: true,
            auto_switch: true,
            group_owner_intent: 6,
        }
    }
}

/// Network parameters
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct NetworkConfig {
    /// Minimum connections to become a relay
    #[serde(default = "default_relay_threshold")]
    pub relay_threshold: u8,
    
    /// Initial TTL for messages
    #[serde(default = "default_ttl")]
    pub initial_ttl: u8,
    
    /// Enable DORS
    #[serde(default = "default_true")]
    pub enable_dors: bool,
}

impl Default for NetworkConfig {
    fn default() -> Self {
        Self {
            relay_threshold: 3,
            initial_ttl: 8,
            enable_dors: true,
        }
    }
}

/// DORS configuration
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct DorsConfig {
    /// Enable automatic transport switching
    #[serde(default = "default_true")]
    pub auto_switch: bool,
    
    /// Hysteresis time in seconds
    #[serde(default = "default_hysteresis")]
    pub switch_hysteresis: u64,
    
    /// Cooldown period in seconds
    #[serde(default = "default_cooldown")]
    pub switch_cooldown: u64,
    
    /// BLE retry threshold before switching
    #[serde(default = "default_retry_threshold")]
    pub ble_to_wifi_retry_threshold: u32,
    
    /// RSSI threshold for switching (dBm)
    #[serde(default = "default_rssi_threshold")]
    pub rssi_switch_threshold: i16,
}

impl Default for DorsConfig {
    fn default() -> Self {
        Self {
            auto_switch: true,
            switch_hysteresis: 15,
            switch_cooldown: 20,
            ble_to_wifi_retry_threshold: 2,
            rssi_switch_threshold: -85,
        }
    }
}

/// Relay configuration
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct RelayConfig {
    /// Allow this device to act as a relay
    #[serde(default = "default_true")]
    pub allow_act_as_relay: bool,
    
    /// Relay priority: "auto", "always", or "never"
    #[serde(default = "default_relay_priority")]
    pub relay_priority: String,
    
    /// Minimum battery percentage to act as relay
    #[serde(default = "default_min_battery")]
    pub min_battery_for_relay: u8,
}

impl Default for RelayConfig {
    fn default() -> Self {
        Self {
            allow_act_as_relay: true,
            relay_priority: "auto".to_string(),
            min_battery_for_relay: 30,
        }
    }
}

/// Reliability settings
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ReliabilityConfig {
    /// Maximum retry attempts
    #[serde(default = "default_max_retries")]
    pub max_retries: u32,
    
    /// ACK timeout in milliseconds
    #[serde(default = "default_ack_timeout")]
    pub ack_timeout: u64,
    
    /// Maximum message lifetime in outbox (milliseconds)
    #[serde(default = "default_outbox_lifetime")]
    pub outbox_max_lifetime: u64,
}

impl Default for ReliabilityConfig {
    fn default() -> Self {
        Self {
            max_retries: 3,
            ack_timeout: 10000,
            outbox_max_lifetime: 3600000, // 1 hour
        }
    }
}

// Default value functions for serde
fn default_true() -> bool {
    true
}

fn default_scan_interval() -> u64 {
    5000
}

fn default_beacon_interval() -> u64 {
    5000
}

fn default_group_owner_intent() -> u8 {
    6
}

fn default_relay_threshold() -> u8 {
    3
}

fn default_ttl() -> u8 {
    8
}

fn default_hysteresis() -> u64 {
    15
}

fn default_cooldown() -> u64 {
    20
}

fn default_retry_threshold() -> u32 {
    2
}

fn default_rssi_threshold() -> i16 {
    -85
}

fn default_relay_priority() -> String {
    "auto".to_string()
}

fn default_min_battery() -> u8 {
    30
}

fn default_max_retries() -> u32 {
    3
}

fn default_ack_timeout() -> u64 {
    10000
}

fn default_outbox_lifetime() -> u64 {
    3600000
}

