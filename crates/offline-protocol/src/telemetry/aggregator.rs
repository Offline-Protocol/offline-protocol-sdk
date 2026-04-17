//! Pure helpers that build telemetry records from protocol state.
//!
//! These are allocation-aware but I/O free and lock free — callers hold the
//! required references. The separation keeps the emit-site code on the
//! `process()` tick tiny and the build logic unit-testable without spinning
//! up a full protocol instance.

use std::collections::HashMap;

use offline_protocol_reliability::{AckManager, Deduplicator, RetryQueue};
use offline_protocol_router::RelayRole;
use offline_protocol_transport::{TransportStatus, TransportType};

use crate::telemetry::device::{
    DeviceCapabilitySnapshot, CHANGED_BATTERY, CHANGED_CHARGING, CHANGED_RELAY_ROLE,
};
use crate::telemetry::metrics_snapshot::MetricsFrame;
use crate::telemetry::transport_state::TransportStateEvent;
use crate::TransportManager;

/// Local snapshot of device capability fields. Used by the process-tick
/// diff to decide whether a `DeviceCapabilitySnapshot` record needs to fire.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) struct DeviceSnap {
    pub(crate) battery_level: Option<u8>,
    pub(crate) is_charging: bool,
    pub(crate) relay_role: RelayRole,
}

impl DeviceSnap {
    pub(crate) fn from_parts(
        battery_level: Option<u8>,
        is_charging: bool,
        relay_role: RelayRole,
    ) -> Self {
        Self {
            battery_level,
            is_charging,
            relay_role,
        }
    }
}

/// Builds a single [`MetricsFrame`] from the supplied protocol components.
pub(crate) fn build_metrics_frame(
    now_ms: i64,
    transport_manager: &TransportManager,
    retry_queue: &RetryQueue,
    deduplicator: &Deduplicator,
    ack_manager: &AckManager,
    neighbor_count: usize,
    relay_count: usize,
) -> MetricsFrame {
    let available = transport_manager.get_available_transports();
    let transports: Vec<(TransportType, _)> = available.into_iter().collect();
    MetricsFrame {
        timestamp_ms: now_ms,
        transports,
        retry_queue: retry_queue.stats(),
        dedup: deduplicator.stats(),
        ack_pending: ack_manager.pending_count(),
        neighbor_count,
        relay_count,
        current_transport: transport_manager.current_transport(),
    }
}

/// Returns one [`TransportStateEvent`] per transport whose status has
/// changed since the last snapshot.
pub(crate) fn diff_transport_state(
    now_ms: i64,
    previous: &HashMap<TransportType, TransportStatus>,
    current: &HashMap<TransportType, TransportStatus>,
) -> Vec<TransportStateEvent> {
    let mut out = Vec::new();
    for (transport, curr_status) in current {
        let prev_status = previous
            .get(transport)
            .copied()
            .unwrap_or(TransportStatus::Unavailable);
        if prev_status != *curr_status {
            out.push(TransportStateEvent {
                timestamp_ms: now_ms,
                transport: *transport,
                previous: prev_status,
                current: *curr_status,
            });
        }
    }
    out
}

/// Returns a [`DeviceCapabilitySnapshot`] when any device-capability field
/// has changed since the last snapshot.
pub(crate) fn diff_device_capability(
    now_ms: i64,
    previous: Option<DeviceSnap>,
    current: DeviceSnap,
) -> Option<DeviceCapabilitySnapshot> {
    let changed_fields = match previous {
        None => CHANGED_BATTERY | CHANGED_CHARGING | CHANGED_RELAY_ROLE,
        Some(prev) => {
            let mut bits = 0u8;
            if prev.battery_level != current.battery_level {
                bits |= CHANGED_BATTERY;
            }
            if prev.is_charging != current.is_charging {
                bits |= CHANGED_CHARGING;
            }
            if prev.relay_role != current.relay_role {
                bits |= CHANGED_RELAY_ROLE;
            }
            bits
        }
    };
    if changed_fields == 0 {
        return None;
    }
    Some(DeviceCapabilitySnapshot {
        timestamp_ms: now_ms,
        battery_level: current.battery_level,
        is_charging: current.is_charging,
        relay_role: current.relay_role,
        changed_fields,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn diff_reports_only_changed_transports() {
        let mut previous = HashMap::new();
        previous.insert(TransportType::BLE, TransportStatus::Available);
        previous.insert(TransportType::Internet, TransportStatus::Disconnected);
        let mut current = HashMap::new();
        current.insert(TransportType::BLE, TransportStatus::Available); // unchanged
        current.insert(TransportType::Internet, TransportStatus::Available); // changed

        let events = diff_transport_state(42, &previous, &current);
        assert_eq!(events.len(), 1);
        assert_eq!(events[0].transport, TransportType::Internet);
        assert_eq!(events[0].previous, TransportStatus::Disconnected);
        assert_eq!(events[0].current, TransportStatus::Available);
        assert_eq!(events[0].timestamp_ms, 42);
    }

    #[test]
    fn diff_fills_unavailable_when_previous_missing() {
        let previous = HashMap::new();
        let mut current = HashMap::new();
        current.insert(TransportType::BLE, TransportStatus::Available);
        let events = diff_transport_state(0, &previous, &current);
        assert_eq!(events.len(), 1);
        assert_eq!(events[0].previous, TransportStatus::Unavailable);
        assert_eq!(events[0].current, TransportStatus::Available);
    }

    #[test]
    fn device_diff_flags_each_changed_field() {
        let previous = DeviceSnap::from_parts(Some(80), false, RelayRole::Regular);
        let current = DeviceSnap::from_parts(Some(79), true, RelayRole::Relay);
        let snapshot = diff_device_capability(7, Some(previous), current).expect("changed");
        assert_eq!(snapshot.timestamp_ms, 7);
        assert!(snapshot.has_changed(CHANGED_BATTERY));
        assert!(snapshot.has_changed(CHANGED_CHARGING));
        assert!(snapshot.has_changed(CHANGED_RELAY_ROLE));
    }

    #[test]
    fn device_diff_returns_none_when_unchanged() {
        let snap = DeviceSnap::from_parts(Some(80), false, RelayRole::Regular);
        let out = diff_device_capability(0, Some(snap), snap);
        assert!(out.is_none());
    }

    #[test]
    fn device_diff_treats_first_snapshot_as_all_changed() {
        let current = DeviceSnap::from_parts(None, false, RelayRole::Regular);
        let snapshot = diff_device_capability(0, None, current).expect("always emits first");
        assert!(snapshot.has_changed(CHANGED_BATTERY));
        assert!(snapshot.has_changed(CHANGED_CHARGING));
        assert!(snapshot.has_changed(CHANGED_RELAY_ROLE));
    }
}
