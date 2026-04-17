//! Push-based snapshots of local device capability changes.
//!
//! Emitted when any of (battery level, charging state, relay role) changes
//! between consecutive `OfflineProtocol::process()` ticks. Apps that need
//! "always-on" polling consume this instead of reconstructing the state from
//! the cumulative `TransportMetrics` stream.
//!
//! Advertised services are **not** represented — the SDK does not currently
//! track an advertised-services list. A later step introduces that surface
//! with its own dedicated record.

use offline_protocol_router::RelayRole;

/// Bitmask bit indicating `battery_level` changed between ticks.
pub const CHANGED_BATTERY: u8 = 0b0000_0001;
/// Bitmask bit indicating `is_charging` changed between ticks.
pub const CHANGED_CHARGING: u8 = 0b0000_0010;
/// Bitmask bit indicating `relay_role` changed between ticks.
pub const CHANGED_RELAY_ROLE: u8 = 0b0000_0100;

/// Snapshot of local device capability at the moment of emission.
///
/// The `changed_fields` bitmask identifies which fields triggered emission —
/// sinks can filter on a specific bit to ignore unrelated churn.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct DeviceCapabilitySnapshot {
    /// Emission timestamp (milliseconds since Unix epoch).
    pub timestamp_ms: i64,
    /// Current battery level (0-100), or `None` if unknown.
    pub battery_level: Option<u8>,
    /// Whether the device is charging.
    pub is_charging: bool,
    /// Current relay role (regular or relay).
    pub relay_role: RelayRole,
    /// Bitmask of the fields that triggered this emission.
    pub changed_fields: u8,
}

impl DeviceCapabilitySnapshot {
    /// Stable OTel-friendly name for this record.
    pub const NAME: &'static str = "protocol.device.capability_changed";

    /// Returns the stable telemetry name.
    pub fn telemetry_name(&self) -> &'static str {
        Self::NAME
    }

    /// Returns `true` when the given change bit is set.
    pub fn has_changed(&self, bit: u8) -> bool {
        self.changed_fields & bit != 0
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn name_is_stable() {
        let snapshot = DeviceCapabilitySnapshot {
            timestamp_ms: 0,
            battery_level: Some(50),
            is_charging: false,
            relay_role: RelayRole::Regular,
            changed_fields: CHANGED_BATTERY,
        };
        assert_eq!(
            DeviceCapabilitySnapshot::NAME,
            "protocol.device.capability_changed"
        );
        assert_eq!(
            snapshot.telemetry_name(),
            "protocol.device.capability_changed"
        );
    }

    #[test]
    fn has_changed_respects_bits() {
        let snapshot = DeviceCapabilitySnapshot {
            timestamp_ms: 0,
            battery_level: None,
            is_charging: true,
            relay_role: RelayRole::Regular,
            changed_fields: CHANGED_CHARGING | CHANGED_RELAY_ROLE,
        };
        assert!(snapshot.has_changed(CHANGED_CHARGING));
        assert!(snapshot.has_changed(CHANGED_RELAY_ROLE));
        assert!(!snapshot.has_changed(CHANGED_BATTERY));
    }
}
