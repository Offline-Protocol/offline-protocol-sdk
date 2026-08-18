//! Relay management for promotion and demotion logic.

use serde::{Deserialize, Serialize};

/// Battery level below which a device stops doing anything for other people,
/// however willing its configuration is.
///
/// This is the hard floor beneath the soft [`RelayConfig::min_battery_for_relay`]:
/// a charging device, or one configured [`RelayPriority::Always`], is excused
/// the soft minimum but never this. It is public because the protocol crate
/// applies the same floor to message forwarding — a device must not keep
/// carrying traffic at a level that would have stripped it of the relay role,
/// and two copies of the number would eventually disagree.
pub const CRITICAL_RELAY_BATTERY_LEVEL: u8 = 15;

/// Configuration for relay behavior.
#[derive(Debug, Clone)]
pub struct RelayConfig {
    /// Minimum battery level to act as relay (percentage).
    pub min_battery_for_relay: u8,

    /// Whether this device allows acting as a relay.
    pub allow_relay: bool,

    /// Priority for relay selection (higher = more preferred).
    pub relay_priority: RelayPriority,
}

impl Default for RelayConfig {
    fn default() -> Self {
        Self {
            min_battery_for_relay: 30,
            allow_relay: true,
            relay_priority: RelayPriority::Auto,
        }
    }
}

/// Relay priority levels.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum RelayPriority {
    /// Never act as relay.
    Never,
    /// Automatically decide based on conditions.
    Auto,
    /// Always try to act as relay (if conditions met).
    Always,
}

/// Relay role of a device.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum RelayRole {
    /// Regular device (not a relay).
    Regular,
    /// Acting as a relay.
    Relay,
}
