//! Push-based `TransportStatus` transition records.
//!
//! Complementary to the existing `Event::TransportSwitched`, which reports the
//! currently-selected transport at the protocol level. This record reports the
//! underlying connection status of a *single* transport (BLE connected,
//! internet disconnected, ...) and fires on every observed transition rather
//! than only on the selection switchover.

use offline_protocol_transport::{TransportStatus, TransportType};

/// A single `TransportStatus` transition observed by the protocol engine.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct TransportStateEvent {
    /// Emission timestamp (milliseconds since Unix epoch).
    pub timestamp_ms: i64,
    /// The transport whose status changed.
    pub transport: TransportType,
    /// Previous status as last observed by the protocol engine.
    pub previous: TransportStatus,
    /// Current status.
    pub current: TransportStatus,
}

impl TransportStateEvent {
    /// Stable OTel-friendly name for this record.
    pub const NAME: &'static str = "protocol.transport.state_changed";

    /// Returns the stable telemetry name.
    pub fn telemetry_name(&self) -> &'static str {
        Self::NAME
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn name_is_stable() {
        let event = TransportStateEvent {
            timestamp_ms: 0,
            transport: TransportType::BLE,
            previous: TransportStatus::Disconnected,
            current: TransportStatus::Available,
        };
        assert_eq!(
            TransportStateEvent::NAME,
            "protocol.transport.state_changed"
        );
        assert_eq!(event.telemetry_name(), "protocol.transport.state_changed");
    }

    #[test]
    fn transitions_are_comparable() {
        let a = TransportStateEvent {
            timestamp_ms: 1,
            transport: TransportType::BLE,
            previous: TransportStatus::Disconnected,
            current: TransportStatus::Available,
        };
        let b = a;
        assert_eq!(a, b);
    }
}
