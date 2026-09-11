//! The typed form of every event the pipe can put on the wire.
//!
//! A buffered event is a small enum, never a JSON value: the emit path
//! constructs it under the shared-state mutex on whatever thread called into
//! the engine, and JSON is built only on the uploader thread when a batch is
//! cut. The enum's variants are the wire event types, its fields are exactly
//! the fields each type carries, and every spelling on the wire has one
//! function here that produces it. Nothing an identifier could hide in (a
//! peer, a message id, a group) has a field to land in.
//!
//! JSON is built through the wire crate's typed `*Data` structs, so a payload
//! that serializes here is one the ingest binds; the two fields the ingest
//! keeps outside those structs (`bucket_start_ms` on a rollup, and the
//! diagnostic `scores` array on a routing decision) are added beside them.

use offline_protocol_router::RelayRole;
use offline_protocol_telemetry_wire::event::{
    DeviceCapabilityData, EscalationReasons, MessageDeliveredData, MessageRelayedData,
    MetricsRollupData, MlsDecryptionFailedData, MlsSessionMissingData, ReasonRetryData,
    RelayDemotedBatteryData, RelayDemotedData, RelayPromotedData, RoutingDecisionData,
    SessionSummaryData, TransportStateData, TransportTimeMs, TYPE_DEVICE_CAPABILITY,
    TYPE_MESSAGE_DEFERRED, TYPE_MESSAGE_DELIVERED, TYPE_MESSAGE_FAILED, TYPE_MESSAGE_RELAYED,
    TYPE_METRICS_ROLLUP, TYPE_MLS_DECRYPTION_FAILED, TYPE_MLS_SESSION_MISSING, TYPE_RELAY_DEMOTED,
    TYPE_RELAY_DEMOTED_BATTERY, TYPE_RELAY_PROMOTED, TYPE_ROUTING_DECISION, TYPE_SESSION_SUMMARY,
    TYPE_TRANSPORT_STATE,
};
use offline_protocol_telemetry_wire::RawEvent;
use offline_protocol_transport::{TransportStatus, TransportType};
use serde_json::{json, Map, Value};
use uuid::Uuid;

use crate::mls_observability::{DecryptionFailureKind, MlsOperationContext};
use crate::telemetry::routing::{RoutingPhase, RoutingReasonCode};

/// A transport in its wire spelling.
///
/// The engine spells a transport three ways (`label()`, `Debug`, and the
/// serde rename), and wire v1 wants one; this is the fold.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum TransportKey {
    Ble,
    WifiDirect,
    Internet,
    Reticulum,
    Nostr,
}

impl TransportKey {
    pub(crate) fn from_transport(transport: TransportType) -> Self {
        match transport {
            TransportType::BLE => Self::Ble,
            TransportType::WiFiDirect => Self::WifiDirect,
            TransportType::Internet => Self::Internet,
            TransportType::Reticulum => Self::Reticulum,
            TransportType::Nostr => Self::Nostr,
        }
    }

    /// Parses any spelling the engine's events use for a transport.
    pub(crate) fn parse(label: &str) -> Option<Self> {
        match label {
            "ble" | "BLE" => Some(Self::Ble),
            "wifiDirect" | "WiFiDirect" | "wifi_direct" => Some(Self::WifiDirect),
            "internet" | "Internet" => Some(Self::Internet),
            "reticulum" | "Reticulum" => Some(Self::Reticulum),
            "nostr" | "Nostr" => Some(Self::Nostr),
            _ => None,
        }
    }

    pub(crate) fn wire(self) -> &'static str {
        match self {
            Self::Ble => "ble",
            Self::WifiDirect => "wifi_direct",
            Self::Internet => "internet",
            Self::Reticulum => "reticulum",
            Self::Nostr => "nostr",
        }
    }

    /// The rollup dwell bucket this transport lands in. Wire v1 has exactly
    /// four; Reticulum and Nostr dwell is counted under `none`, a known
    /// under-count rather than a misattribution.
    pub(crate) fn dwell(self) -> DwellKey {
        match self {
            Self::Ble => DwellKey::Ble,
            Self::WifiDirect => DwellKey::WifiDirect,
            Self::Internet => DwellKey::Internet,
            Self::Reticulum | Self::Nostr => DwellKey::None,
        }
    }
}

/// The four `transport_time_ms` buckets of a rollup.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum DwellKey {
    Ble,
    WifiDirect,
    Internet,
    None,
}

impl DwellKey {
    pub(crate) fn index(self) -> usize {
        match self {
            Self::Ble => 0,
            Self::WifiDirect => 1,
            Self::Internet => 2,
            Self::None => 3,
        }
    }

    pub(crate) fn wire(self) -> &'static str {
        match self {
            Self::Ble => "ble",
            Self::WifiDirect => "wifi_direct",
            Self::Internet => "internet",
            Self::None => "none",
        }
    }
}

pub(crate) fn wire_status(status: TransportStatus) -> &'static str {
    match status {
        TransportStatus::Available => "available",
        TransportStatus::Unavailable => "unavailable",
        TransportStatus::Connecting => "connecting",
        TransportStatus::Disconnected => "disconnected",
        TransportStatus::Error => "error",
    }
}

pub(crate) fn wire_relay_role(role: RelayRole) -> &'static str {
    match role {
        RelayRole::Regular => "regular",
        RelayRole::Relay => "relay",
    }
}

pub(crate) fn wire_phase(phase: RoutingPhase) -> &'static str {
    match phase {
        RoutingPhase::ScoreUpdated => "score_updated",
        RoutingPhase::Selected => "selected",
        RoutingPhase::Switched => "switched",
        RoutingPhase::Escalated => "escalated",
    }
}

pub(crate) fn wire_reason_code(code: RoutingReasonCode) -> &'static str {
    match code {
        RoutingReasonCode::InitialSelection => "initial_selection",
        RoutingReasonCode::PrimarySelected => "primary_selected",
        RoutingReasonCode::PrimarySuccess => "primary_success",
        RoutingReasonCode::FallbackSuccess => "fallback_success",
        RoutingReasonCode::EscalationApplied => "escalation_applied",
        RoutingReasonCode::CurrentUnavailable => "current_unavailable",
        RoutingReasonCode::RetryThreshold => "retry_threshold",
        RoutingReasonCode::PoorSignal => "poor_signal",
        RoutingReasonCode::Congestion => "congestion",
        RoutingReasonCode::LowTtl => "low_ttl",
        RoutingReasonCode::LowSuccessRate => "low_success_rate",
    }
}

pub(crate) fn wire_mls_context(context: MlsOperationContext) -> &'static str {
    match context {
        MlsOperationContext::Send => "send",
        MlsOperationContext::Receive => "receive",
        MlsOperationContext::Initialize => "initialize",
        MlsOperationContext::SessionLookup => "session_lookup",
        MlsOperationContext::Welcome => "welcome",
    }
}

pub(crate) fn wire_failure_kind(kind: DecryptionFailureKind) -> &'static str {
    match kind {
        DecryptionFailureKind::SessionNotFound => "session_not_found",
        DecryptionFailureKind::NotInitialized => "not_initialized",
        DecryptionFailureKind::InvalidCiphertext => "invalid_ciphertext",
        DecryptionFailureKind::IdentityMismatch => "identity_mismatch",
        DecryptionFailureKind::CryptoFailure => "crypto_failure",
        DecryptionFailureKind::Unknown => "unknown",
    }
}

/// One entry of the diagnostic `scores` array on a routing decision.
#[derive(Debug, Clone, Copy, PartialEq)]
pub(crate) struct ScoreEntry {
    pub(crate) transport: TransportKey,
    pub(crate) signal: f32,
    pub(crate) proximity: f32,
    pub(crate) bandwidth: f32,
    pub(crate) congestion: f32,
    pub(crate) energy: f32,
    pub(crate) reliability: f32,
    pub(crate) load: f32,
    pub(crate) total: f32,
}

/// One closed rollup window, in the units the wire carries.
#[derive(Debug, Clone, PartialEq)]
pub(crate) struct RollupData {
    pub(crate) bucket_start_ms: i64,
    pub(crate) bucket_duration_s: i64,
    pub(crate) neighbor_count_avg: f64,
    pub(crate) neighbor_count_max: i64,
    pub(crate) neighbor_count_p50: i64,
    pub(crate) ack_pending_avg: f64,
    pub(crate) ack_pending_max: i64,
    pub(crate) retry_queue_total_avg: f64,
    pub(crate) retry_queue_total_max: i64,
    pub(crate) retry_queue_critical_count_max: i64,
    /// Indexed by [`DwellKey::index`].
    pub(crate) transport_time_ms: [i64; 4],
    pub(crate) is_local_relay_duration_s: i64,
    pub(crate) current_transport_at_bucket_end: DwellKey,
    pub(crate) sends_attempted_sum: i64,
    pub(crate) sends_succeeded_sum: i64,
    pub(crate) sends_failed_sum: i64,
}

/// One session summary, in the units the wire carries.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct SessionSummary {
    pub(crate) session_duration_s: i64,
    pub(crate) routing_switches: u32,
    pub(crate) routing_escalations: u32,
    /// In wire order: `poor_signal`, `low_success_rate`, `retry_threshold`,
    /// `congestion`, `low_ttl`, `current_unavailable`.
    pub(crate) escalation_reasons: [u32; 6],
    /// Absent, never zero, when nothing paired: a percentile ignores an
    /// absent value and counts a zero.
    pub(crate) mls_session_ready_latency_p50_ms: Option<i64>,
    pub(crate) mls_encryption_used_count: u32,
}

/// The payload of one wire event.
#[derive(Debug, Clone, PartialEq)]
pub(crate) enum WireEventData {
    MessageDelivered {
        latency_ms: i32,
        hop_count: i16,
        transport: Option<TransportKey>,
    },
    MessageFailed {
        reason: Box<str>,
        retry_count: i16,
    },
    MessageDeferred {
        reason: Box<str>,
        retry_count: i16,
    },
    MessageRelayed {
        hop_count: i16,
        remaining_ttl: i16,
    },
    RelayPromoted {
        connection_count: i16,
        battery_level: Option<i16>,
    },
    RelayDemoted {
        reason: Box<str>,
    },
    RelayDemotedBattery {
        battery_level: i16,
        min_required: i16,
    },
    RoutingDecision {
        phase: RoutingPhase,
        from: Option<TransportKey>,
        to: Option<TransportKey>,
        winning_score: Option<f32>,
        reason_code: Option<RoutingReasonCode>,
        /// Present only under `routing_diagnostic`.
        scores: Option<Box<[ScoreEntry]>>,
    },
    TransportState {
        transport: TransportKey,
        previous: TransportStatus,
        current: TransportStatus,
    },
    DeviceCapability {
        battery_level: Option<u8>,
        is_charging: bool,
        relay_role: RelayRole,
        changed_fields: u8,
    },
    MlsDecryptionFailed {
        failure_kind: DecryptionFailureKind,
        context: MlsOperationContext,
    },
    MlsSessionMissing {
        context: MlsOperationContext,
    },
    MetricsRollup(Box<RollupData>),
    SessionSummary(SessionSummary),
}

impl WireEventData {
    /// The wire `type` of this event.
    pub(crate) fn wire_type(&self) -> &'static str {
        match self {
            Self::MessageDelivered { .. } => TYPE_MESSAGE_DELIVERED,
            Self::MessageFailed { .. } => TYPE_MESSAGE_FAILED,
            Self::MessageDeferred { .. } => TYPE_MESSAGE_DEFERRED,
            Self::MessageRelayed { .. } => TYPE_MESSAGE_RELAYED,
            Self::RelayPromoted { .. } => TYPE_RELAY_PROMOTED,
            Self::RelayDemoted { .. } => TYPE_RELAY_DEMOTED,
            Self::RelayDemotedBattery { .. } => TYPE_RELAY_DEMOTED_BATTERY,
            Self::RoutingDecision { .. } => TYPE_ROUTING_DECISION,
            Self::TransportState { .. } => TYPE_TRANSPORT_STATE,
            Self::DeviceCapability { .. } => TYPE_DEVICE_CAPABILITY,
            Self::MlsDecryptionFailed { .. } => TYPE_MLS_DECRYPTION_FAILED,
            Self::MlsSessionMissing { .. } => TYPE_MLS_SESSION_MISSING,
            Self::MetricsRollup(_) => TYPE_METRICS_ROLLUP,
            Self::SessionSummary(_) => TYPE_SESSION_SUMMARY,
        }
    }

    /// A cheap estimate of the serialized size, for the size trigger on the
    /// emit path. Exact sizes are measured when a batch is cut.
    pub(crate) fn approx_bytes(&self) -> usize {
        const ENVELOPE: usize = 48;
        ENVELOPE
            + match self {
                Self::MessageDelivered { .. } => 60,
                Self::MessageFailed { reason, .. } | Self::MessageDeferred { reason, .. } => {
                    40 + reason.len()
                }
                Self::MessageRelayed { .. } => 40,
                Self::RelayPromoted { .. } => 48,
                Self::RelayDemoted { reason } => 16 + reason.len(),
                Self::RelayDemotedBattery { .. } => 40,
                Self::RoutingDecision { scores, .. } => {
                    100 + scores.as_ref().map_or(0, |s| s.len() * 160)
                }
                Self::TransportState { .. } => 80,
                Self::DeviceCapability { .. } => 90,
                Self::MlsDecryptionFailed { .. } => 60,
                Self::MlsSessionMissing { .. } => 30,
                Self::MetricsRollup(_) => 520,
                Self::SessionSummary(_) => 260,
            }
    }

    /// The `data` object of this event, as the ingest binds it.
    pub(crate) fn to_json(&self) -> Value {
        let mut value = match self {
            Self::MessageDelivered {
                latency_ms,
                hop_count,
                transport,
            } => typed(MessageDeliveredData {
                latency_ms: Some(*latency_ms),
                hop_count: Some(*hop_count),
                transport: transport.map(|t| t.wire().to_string()),
            }),
            Self::MessageFailed {
                reason,
                retry_count,
            }
            | Self::MessageDeferred {
                reason,
                retry_count,
            } => typed(ReasonRetryData {
                reason_class: None,
                retry_count: Some(*retry_count),
                reason: Some(reason.to_string()),
            }),
            Self::MessageRelayed {
                hop_count,
                remaining_ttl,
            } => typed(MessageRelayedData {
                hop_count: Some(*hop_count),
                remaining_ttl: Some(*remaining_ttl),
            }),
            Self::RelayPromoted {
                connection_count,
                battery_level,
            } => typed(RelayPromotedData {
                connection_count: Some(*connection_count),
                battery_level: *battery_level,
            }),
            Self::RelayDemoted { reason } => typed(RelayDemotedData {
                reason_class: None,
                reason: Some(reason.to_string()),
            }),
            Self::RelayDemotedBattery {
                battery_level,
                min_required,
            } => typed(RelayDemotedBatteryData {
                battery_level: Some(*battery_level),
                min_required: Some(*min_required),
            }),
            Self::RoutingDecision {
                phase,
                from,
                to,
                winning_score,
                reason_code,
                scores,
            } => {
                let mut value = typed(RoutingDecisionData {
                    phase: Some(wire_phase(*phase).to_string()),
                    from_transport: from.map(|t| t.wire().to_string()),
                    to_transport: to.map(|t| t.wire().to_string()),
                    winning_score: *winning_score,
                    reason_code: reason_code.map(|c| wire_reason_code(c).to_string()),
                });
                if let (Some(scores), Value::Object(map)) = (scores, &mut value) {
                    map.insert(
                        "scores".to_string(),
                        Value::Array(scores.iter().map(score_json).collect()),
                    );
                }
                value
            }
            Self::TransportState {
                transport,
                previous,
                current,
            } => typed(TransportStateData {
                transport: Some(transport.wire().to_string()),
                previous_status: Some(wire_status(*previous).to_string()),
                current_status: Some(wire_status(*current).to_string()),
            }),
            Self::DeviceCapability {
                battery_level,
                is_charging,
                relay_role,
                changed_fields,
            } => typed(DeviceCapabilityData {
                battery_level: battery_level.map(i16::from),
                is_charging: Some(*is_charging),
                relay_role: Some(wire_relay_role(*relay_role).to_string()),
                changed_fields: Some(i16::from(*changed_fields)),
            }),
            Self::MlsDecryptionFailed {
                failure_kind,
                context,
            } => typed(MlsDecryptionFailedData {
                failure_kind: Some(wire_failure_kind(*failure_kind).to_string()),
                mls_context: Some(wire_mls_context(*context).to_string()),
            }),
            Self::MlsSessionMissing { context } => typed(MlsSessionMissingData {
                mls_context: Some(wire_mls_context(*context).to_string()),
            }),
            Self::MetricsRollup(rollup) => {
                let r = rollup.as_ref();
                let mut value = typed(MetricsRollupData {
                    bucket_duration_s: Some(clamp_i16(r.bucket_duration_s)),
                    neighbor_count_avg: Some(r.neighbor_count_avg as f32),
                    neighbor_count_max: Some(clamp_i32(r.neighbor_count_max)),
                    neighbor_count_p50: Some(clamp_i32(r.neighbor_count_p50)),
                    ack_pending_avg: Some(r.ack_pending_avg as f32),
                    ack_pending_max: Some(clamp_i32(r.ack_pending_max)),
                    retry_queue_total_avg: Some(r.retry_queue_total_avg as f32),
                    retry_queue_total_max: Some(clamp_i32(r.retry_queue_total_max)),
                    retry_queue_critical_max: Some(clamp_i32(r.retry_queue_critical_count_max)),
                    transport_time_ms: Some(TransportTimeMs {
                        ble: Some(clamp_i32(r.transport_time_ms[DwellKey::Ble.index()])),
                        wifi_direct: Some(clamp_i32(
                            r.transport_time_ms[DwellKey::WifiDirect.index()],
                        )),
                        internet: Some(clamp_i32(r.transport_time_ms[DwellKey::Internet.index()])),
                        none: Some(clamp_i32(r.transport_time_ms[DwellKey::None.index()])),
                    }),
                    is_local_relay_duration_s: Some(clamp_i32(r.is_local_relay_duration_s)),
                    current_transport_at_bucket_end: Some(
                        r.current_transport_at_bucket_end.wire().to_string(),
                    ),
                    sends_attempted: Some(clamp_i32(r.sends_attempted_sum)),
                    sends_succeeded: Some(clamp_i32(r.sends_succeeded_sum)),
                    sends_failed: Some(clamp_i32(r.sends_failed_sum)),
                    // Deliberately zero, not unimplemented: the field is a
                    // per-window sum nothing downstream reads yet, and
                    // present-but-zero keeps the wire schema stable.
                    bytes_sent: Some(0),
                    bytes_received: Some(0),
                });
                if let Value::Object(map) = &mut value {
                    // On the wire but outside the typed struct: the ingest
                    // takes the row's timestamp from the envelope `ts_ms`,
                    // which the pipe stamps from this same value.
                    map.insert("bucket_start_ms".to_string(), json!(r.bucket_start_ms));
                }
                value
            }
            Self::SessionSummary(summary) => typed(SessionSummaryData {
                session_duration_s: Some(clamp_i32(summary.session_duration_s)),
                routing_switches: Some(clamp_u32(summary.routing_switches)),
                routing_escalations: Some(clamp_u32(summary.routing_escalations)),
                escalation_reasons: Some(EscalationReasons {
                    poor_signal: Some(clamp_u32(summary.escalation_reasons[0])),
                    low_success_rate: Some(clamp_u32(summary.escalation_reasons[1])),
                    retry_threshold: Some(clamp_u32(summary.escalation_reasons[2])),
                    congestion: Some(clamp_u32(summary.escalation_reasons[3])),
                    low_ttl: Some(clamp_u32(summary.escalation_reasons[4])),
                    current_unavailable: Some(clamp_u32(summary.escalation_reasons[5])),
                }),
                mls_session_ready_latency_p50_ms: summary
                    .mls_session_ready_latency_p50_ms
                    .map(clamp_i32),
                mls_encryption_used_count: Some(clamp_u32(summary.mls_encryption_used_count)),
                ..SessionSummaryData::default()
            }),
        };
        strip_nulls(&mut value);
        value
    }
}

fn score_json(score: &ScoreEntry) -> Value {
    json!({
        "transport": score.transport.wire(),
        "signal": score.signal,
        "proximity": score.proximity,
        "bandwidth": score.bandwidth,
        "congestion": score.congestion,
        "energy": score.energy,
        "reliability": score.reliability,
        "load": score.load,
        "total": score.total,
    })
}

/// Serializes a typed payload. The typed structs are plain data with no
/// custom serializers, so this cannot fail in practice; the fallback keeps
/// the uploader path free of any panic.
fn typed<T: serde::Serialize>(data: T) -> Value {
    serde_json::to_value(data).unwrap_or_else(|_| Value::Object(Map::new()))
}

/// Removes every `null` from an object, recursively.
///
/// The typed structs carry every column as an `Option` and serialize an
/// absent one as `null`; the wire never carries `null` (absent and zero are
/// opposites to a percentile), and the earlier client never sent one.
fn strip_nulls(value: &mut Value) {
    if let Value::Object(map) = value {
        map.retain(|_, v| !v.is_null());
        for v in map.values_mut() {
            strip_nulls(v);
        }
    }
}

pub(crate) fn clamp_i16(value: i64) -> i16 {
    value.clamp(i64::from(i16::MIN), i64::from(i16::MAX)) as i16
}

pub(crate) fn clamp_i32(value: i64) -> i32 {
    value.clamp(i64::from(i32::MIN), i64::from(i32::MAX)) as i32
}

pub(crate) fn clamp_u32(value: u32) -> i32 {
    value.min(i32::MAX as u32) as i32
}

pub(crate) fn clamp_u64_i32(value: u64) -> i32 {
    value.min(i32::MAX as u64) as i32
}

pub(crate) fn clamp_u64_i16(value: u64) -> i16 {
    value.min(i16::MAX as u64) as i16
}

/// One event, timestamped. The session it belongs to rides beside it in
/// [`BufferedEvent`] rather than inside: `session_id` is an envelope field.
#[derive(Debug, Clone, PartialEq)]
pub(crate) struct WireEvent {
    pub(crate) ts_ms: i64,
    pub(crate) data: WireEventData,
}

impl WireEvent {
    pub(crate) fn to_raw(&self) -> RawEvent {
        RawEvent {
            event_type: self.data.wire_type().to_string(),
            ts_ms: self.ts_ms,
            data: self.data.to_json(),
        }
    }
}

/// One buffered event plus the session it was observed under.
///
/// Stamped at push time, so a session-boundary event carries the session it
/// reports rather than whichever one is current when the uploader next runs.
#[derive(Debug, Clone, PartialEq)]
pub(crate) struct BufferedEvent {
    pub(crate) session: Uuid,
    pub(crate) event: WireEvent,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_buffered_event_stays_small() {
        // The ring holds up to 4096 of these; the budget is under a kilobyte
        // per event and this pins it an order of magnitude below.
        assert!(
            std::mem::size_of::<BufferedEvent>() <= 256,
            "BufferedEvent is {} bytes",
            std::mem::size_of::<BufferedEvent>()
        );
    }

    #[test]
    fn nulls_never_reach_the_wire() {
        let data = WireEventData::RelayPromoted {
            connection_count: 3,
            battery_level: None,
        };
        let json = data.to_json();
        assert_eq!(json, json!({ "connection_count": 3 }));
    }

    #[test]
    fn a_reason_goes_up_raw_and_never_classified() {
        let data = WireEventData::MessageFailed {
            reason: "Max retries exceeded".into(),
            retry_count: 5,
        };
        assert_eq!(
            data.to_json(),
            json!({ "reason": "Max retries exceeded", "retry_count": 5 })
        );
    }

    #[test]
    fn every_transport_spelling_the_engine_uses_folds_to_one_wire_spelling() {
        for (label, key) in [
            ("ble", TransportKey::Ble),
            ("BLE", TransportKey::Ble),
            ("wifiDirect", TransportKey::WifiDirect),
            ("WiFiDirect", TransportKey::WifiDirect),
            ("internet", TransportKey::Internet),
            ("Internet", TransportKey::Internet),
            ("reticulum", TransportKey::Reticulum),
            ("nostr", TransportKey::Nostr),
        ] {
            assert_eq!(TransportKey::parse(label), Some(key), "{label}");
        }
        for transport in [
            TransportType::BLE,
            TransportType::WiFiDirect,
            TransportType::Internet,
            TransportType::Reticulum,
            TransportType::Nostr,
        ] {
            assert_eq!(
                TransportKey::parse(transport.label()),
                Some(TransportKey::from_transport(transport)),
                "label() of {transport:?}"
            );
            assert_eq!(
                TransportKey::parse(&format!("{transport:?}")),
                Some(TransportKey::from_transport(transport)),
                "Debug of {transport:?}"
            );
        }
        assert_eq!(TransportKey::parse("carrier-pigeon"), None);
    }

    #[test]
    fn the_rollup_json_carries_bucket_start_and_integer_p50() {
        let rollup = RollupData {
            bucket_start_ms: 1_747_000_060_000,
            bucket_duration_s: 60,
            neighbor_count_avg: 4.5,
            neighbor_count_max: 9,
            neighbor_count_p50: 4,
            ack_pending_avg: 1.25,
            ack_pending_max: 3,
            retry_queue_total_avg: 2.0,
            retry_queue_total_max: 5,
            retry_queue_critical_count_max: 1,
            transport_time_ms: [40_000, 15_000, 5_000, 0],
            is_local_relay_duration_s: 12,
            current_transport_at_bucket_end: DwellKey::Ble,
            sends_attempted_sum: 12,
            sends_succeeded_sum: 11,
            sends_failed_sum: 1,
        };
        let json = WireEventData::MetricsRollup(Box::new(rollup)).to_json();
        assert_eq!(json["bucket_start_ms"], json!(1_747_000_060_000_i64));
        assert!(json["neighbor_count_p50"].is_i64());
        assert_eq!(json["transport_time_ms"]["wifi_direct"], json!(15_000));
        assert_eq!(json["bytes_sent_sum"], json!(0));
    }

    #[test]
    fn the_summary_omits_the_p50_when_nothing_paired() {
        let summary = SessionSummary {
            session_duration_s: 10,
            routing_switches: 0,
            routing_escalations: 0,
            escalation_reasons: [0; 6],
            mls_session_ready_latency_p50_ms: None,
            mls_encryption_used_count: 0,
        };
        let json = WireEventData::SessionSummary(summary).to_json();
        assert!(json.get("mls_session_ready_latency_p50_ms").is_none());
        assert_eq!(json["escalation_reasons"]["current_unavailable"], json!(0));
        // The fields the client leaves to other sources stay absent.
        assert!(json.get("messages_sent").is_none());
    }
}
