//! Pure helpers that build telemetry records from protocol state.
//!
//! These are allocation-aware but I/O free and lock free — callers hold the
//! required references. The separation keeps the emit-site code on the
//! `process()` tick tiny and the build logic unit-testable without spinning
//! up a full protocol instance.

use std::collections::{HashMap, HashSet};

use offline_protocol_reliability::{AckManager, Deduplicator, RetryQueue};
use offline_protocol_router::RelayRole;
use offline_protocol_transport::{TransportMetrics, TransportStatus, TransportType};

use crate::telemetry::device::{
    DeviceCapabilitySnapshot, CHANGED_BATTERY, CHANGED_CHARGING, CHANGED_RELAY_ROLE,
};
use crate::telemetry::metrics_snapshot::MetricsFrame;
use crate::telemetry::transport_state::TransportStateEvent;

/// Deterministic walk order: honour the existing transport-priority
/// (Internet > WiFiDirect > BLE > everything else) so any "first with X"
/// selection is stable across ticks regardless of HashMap iteration order.
///
/// Also used as the stable ordering key for [`MetricsFrame::transports`] so
/// sinks that index positionally see the same layout on every emission.
const TRANSPORT_PRIORITY: &[TransportType] = &[
    TransportType::Internet,
    TransportType::WiFiDirect,
    TransportType::BLE,
    TransportType::Reticulum,
    TransportType::Nostr,
];

/// Returns the priority index for `t`. Transports not present in
/// [`TRANSPORT_PRIORITY`] sort after every listed transport (stable-sort
/// ties then fall back to HashMap iteration order — currently impossible
/// since every [`TransportType`] variant is listed, but future variants
/// land at the tail without the helper breaking).
fn priority_index(t: TransportType) -> usize {
    TRANSPORT_PRIORITY
        .iter()
        .position(|p| *p == t)
        .unwrap_or(TRANSPORT_PRIORITY.len())
}

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
///
/// `available_transports` is borrowed — callers snapshot the map once per
/// tick and reuse it across the telemetry helpers that need it, avoiding
/// redundant locking and per-tick HashMap allocations.
///
/// The emitted `transports` vec is sorted by [`TRANSPORT_PRIORITY`] so
/// sinks that index positionally see the same layout on every tick; HashMap
/// iteration order does not leak into the record.
#[allow(clippy::too_many_arguments)]
pub(crate) fn build_metrics_frame(
    now_ms: i64,
    current_transport: Option<TransportType>,
    available_transports: &HashMap<TransportType, TransportMetrics>,
    retry_queue: &RetryQueue,
    deduplicator: &Deduplicator,
    ack_manager: &AckManager,
    neighbor_count: usize,
    is_local_relay: bool,
) -> MetricsFrame {
    let mut transports: Vec<(TransportType, TransportMetrics)> = available_transports
        .iter()
        .map(|(t, m)| (*t, m.clone()))
        .collect();
    transports.sort_by_key(|(t, _)| priority_index(*t));
    MetricsFrame {
        timestamp_ms: now_ms,
        transports,
        retry_queue: retry_queue.stats(),
        dedup: deduplicator.stats(),
        ack_pending: ack_manager.pending_count(),
        neighbor_count,
        is_local_relay,
        current_transport,
    }
}

/// Returns one [`TransportStateEvent`] per transport whose status has
/// changed since the last snapshot.
///
/// Iterates the **union** of `previous` and `current` keys so that a
/// transport which disappeared between ticks (e.g. `remove_transport()`
/// called at runtime) emits a `prev_status → Unavailable` transition rather
/// than being silently forgotten.
pub(crate) fn diff_transport_state(
    now_ms: i64,
    previous: &HashMap<TransportType, TransportStatus>,
    current: &HashMap<TransportType, TransportStatus>,
) -> Vec<TransportStateEvent> {
    let mut keys: HashSet<TransportType> = HashSet::with_capacity(previous.len() + current.len());
    keys.extend(previous.keys().copied());
    keys.extend(current.keys().copied());

    let mut out = Vec::new();
    for transport in keys {
        let prev_status = previous
            .get(&transport)
            .copied()
            .unwrap_or(TransportStatus::Unavailable);
        let curr_status = current
            .get(&transport)
            .copied()
            .unwrap_or(TransportStatus::Unavailable);
        if prev_status != curr_status {
            out.push(TransportStateEvent {
                timestamp_ms: now_ms,
                transport,
                previous: prev_status,
                current: curr_status,
            });
        }
    }
    out
}

/// Derives `(battery_level, is_charging)` for the local device from the
/// per-transport metrics map.
///
/// Selection order:
///   1. Currently selected transport, if it reports `Some(battery_level)`.
///   2. First available transport (deterministic by [`TRANSPORT_PRIORITY`])
///      that reports `Some(battery_level)`.
///   3. `(None, is_charging)` where `is_charging` comes from the current
///      transport if present, else from the first transport in priority
///      order.
///
/// `is_charging` is **always** the current transport's reading when a
/// current transport is registered AND present in `available` —
/// regardless of which transport supplied `battery_level`. Mixing
/// `is_charging` from one transport with `battery_level` from another
/// produces non-deterministic flips across ticks (HashMap iteration order
/// is unspecified) and would spuriously trigger `CHANGED_CHARGING` on the
/// device-capability diff. When `current` is set but missing from
/// `available` (e.g. status flipped mid-tick), the function falls through
/// to the no-current branch.
pub(crate) fn device_battery_from_available(
    current: Option<TransportType>,
    available: &HashMap<TransportType, TransportMetrics>,
) -> (Option<u8>, bool) {
    let first_with_battery = |skip: Option<TransportType>| -> Option<u8> {
        for t in TRANSPORT_PRIORITY {
            if skip == Some(*t) {
                continue;
            }
            if let Some(m) = available.get(t) {
                if m.battery_level.is_some() {
                    return m.battery_level;
                }
            }
        }
        // Fallback for transport types not in the priority list.
        available
            .iter()
            .find(|(t, m)| Some(**t) != skip && m.battery_level.is_some())
            .and_then(|(_, m)| m.battery_level)
    };

    if let Some(current) = current {
        if let Some(metrics) = available.get(&current) {
            let battery = metrics
                .battery_level
                .or_else(|| first_with_battery(Some(current)));
            return (battery, metrics.is_charging);
        }
        // `current` is set but not present in `available` — fall through
        // to the no-current branch but without an `is_charging` anchor.
    }
    let battery = first_with_battery(None);
    let is_charging = TRANSPORT_PRIORITY
        .iter()
        .find_map(|t| available.get(t))
        .or_else(|| available.values().next())
        .map(|m| m.is_charging)
        .unwrap_or(false);
    (battery, is_charging)
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
    fn diff_emits_unavailable_when_transport_removed() {
        // A transport that was Available in `previous` but is missing from
        // `current` (e.g. `remove_transport()` between ticks) must emit a
        // transition to Unavailable rather than disappear silently.
        let mut previous = HashMap::new();
        previous.insert(TransportType::BLE, TransportStatus::Available);
        previous.insert(TransportType::Internet, TransportStatus::Available);
        let mut current = HashMap::new();
        current.insert(TransportType::BLE, TransportStatus::Available); // kept

        let events = diff_transport_state(9, &previous, &current);
        assert_eq!(events.len(), 1, "got {events:?}");
        assert_eq!(events[0].transport, TransportType::Internet);
        assert_eq!(events[0].previous, TransportStatus::Available);
        assert_eq!(events[0].current, TransportStatus::Unavailable);
        assert_eq!(events[0].timestamp_ms, 9);
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

    fn metrics(battery: Option<u8>, is_charging: bool) -> TransportMetrics {
        TransportMetrics {
            battery_level: battery,
            is_charging,
            ..Default::default()
        }
    }

    #[test]
    fn device_battery_uses_current_transport_when_present() {
        // current = BLE with no battery, fallback by priority must pick
        // Internet's battery. is_charging stays anchored to BLE because
        // BLE is the current transport (deterministic across ticks).
        let mut available = HashMap::new();
        available.insert(TransportType::BLE, metrics(None, false));
        available.insert(TransportType::Internet, metrics(Some(73), true));

        let (battery, is_charging) =
            device_battery_from_available(Some(TransportType::BLE), &available);
        assert_eq!(battery, Some(73));
        assert!(!is_charging);
    }

    #[test]
    fn device_battery_falls_through_when_current_not_in_available() {
        // current = BLE but BLE is not in `available` (e.g. status flipped
        // mid-tick). The current branch is skipped; the no-current branch
        // walks priority order and returns Internet's reading for both
        // fields. Regression guard for an untested fall-through path.
        let mut available = HashMap::new();
        available.insert(TransportType::Internet, metrics(Some(60), true));

        let (battery, is_charging) =
            device_battery_from_available(Some(TransportType::BLE), &available);
        assert_eq!(battery, Some(60));
        assert!(is_charging);
    }

    #[test]
    fn device_battery_priority_walk_is_deterministic_when_current_is_none() {
        // Priority walk (Internet > WiFiDirect > BLE) makes the choice
        // stable regardless of HashMap iteration order.
        let mut available = HashMap::new();
        available.insert(TransportType::BLE, metrics(Some(40), false));
        available.insert(TransportType::Internet, metrics(Some(90), true));

        let (battery, is_charging) = device_battery_from_available(None, &available);
        assert_eq!(battery, Some(90));
        assert!(is_charging);
    }

    #[test]
    fn device_battery_returns_none_when_no_transports_have_battery() {
        let mut available = HashMap::new();
        available.insert(TransportType::BLE, metrics(None, true));

        let (battery, is_charging) = device_battery_from_available(None, &available);
        assert_eq!(battery, None);
        // is_charging anchors on the first priority-ordered transport
        // that exists in `available`.
        assert!(is_charging);
    }

    #[test]
    fn build_metrics_frame_orders_transports_by_priority() {
        // Regression guard: `MetricsFrame::transports` must be ordered by
        // TRANSPORT_PRIORITY (Internet > WiFiDirect > BLE > ...) so sinks
        // indexing positionally see a stable layout across ticks. Prior
        // implementation let HashMap iteration order leak through.
        use offline_protocol_reliability::{AckManager, Deduplicator, RetryQueue};
        let mut available = HashMap::new();
        available.insert(TransportType::BLE, metrics(None, false));
        available.insert(TransportType::Internet, metrics(Some(50), true));
        available.insert(TransportType::WiFiDirect, metrics(None, false));

        let retry_queue = RetryQueue::new();
        let deduplicator = Deduplicator::new();
        let ack_manager = AckManager::new();
        let frame = build_metrics_frame(
            0,
            Some(TransportType::BLE),
            &available,
            &retry_queue,
            &deduplicator,
            &ack_manager,
            0,
            false,
        );
        let order: Vec<TransportType> = frame.transports.iter().map(|(t, _)| *t).collect();
        assert_eq!(
            order,
            vec![
                TransportType::Internet,
                TransportType::WiFiDirect,
                TransportType::BLE
            ],
            "metrics frame transports must be ordered by TRANSPORT_PRIORITY",
        );
        assert_eq!(frame.current_transport, Some(TransportType::BLE));
    }
}
