//! Record classification and field projection.
//!
//! This is the synchronous decision the pipe makes for every record on the
//! emit path: drop it, forward it with only its documented fields, fold it
//! into the rollup, or feed it to the session tracker. Free-form text and
//! identifiers never make it into a [`WireEvent`]: a forwarded event is a
//! typed projection, and a `reason` is the engine's own fixed token or
//! literal, sent raw and classified server-side (the classification taxonomy
//! is the ingest's, not the client's, so that it can evolve without a
//! release and so that a fork of the client carries no rules worth copying).
//!
//! Protocol events are classified by variant. Ten are collected; the
//! wildcard that drops the rest can only ever lose an event, never leak one,
//! and `every_protocol_event_is_classified_by_its_stable_name` pins the ten
//! by name so a variant added later is seen to be dropped rather than
//! silently forwarded.

use crate::events::Event;
use crate::mls_observability::MlsLifecycleEvent;
use crate::telemetry::pipe::wire::{
    clamp_u64_i16, clamp_u64_i32, ScoreEntry, TransportKey, WireEvent, WireEventData,
};
use crate::telemetry::TelemetryRecord;

/// The protocol events the pipe collects, by stable telemetry name. The
/// remaining sixty-odd are dropped without their payload being read.
pub(crate) const PROTOCOL_ALLOWLIST: [&str; 10] = [
    "protocol.message.sent",
    "protocol.message.delivered",
    "protocol.message.failed",
    "protocol.message.deferred",
    "protocol.message.relayed",
    "protocol.relay.promoted",
    "protocol.relay.demoted",
    "protocol.relay.demoted_battery",
    "protocol.neighbor.discovered",
    "protocol.neighbor.lost",
];

/// What the rollup counts per window.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum MessageKind {
    Sent,
    Delivered,
    Failed,
}

/// One metrics frame, reduced to what the rollup reads.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) struct FrameSample {
    pub(crate) ts_ms: i64,
    pub(crate) neighbor_count: u64,
    pub(crate) ack_pending: u64,
    pub(crate) retry_total: u64,
    pub(crate) retry_critical: u64,
    pub(crate) current_transport: Option<TransportKey>,
    pub(crate) is_local_relay: bool,
}

/// One record for the session tracker.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum SessionInput {
    MlsInitialized {
        session_id: String,
        ts_ms: i64,
    },
    MlsSessionReady {
        session_id: String,
        ts_ms: i64,
    },
    EncryptionUsed,
    /// Neighbour churn has no summary field on either side of the contract
    /// yet, so it is observed and counted nowhere.
    NeighborChurn,
}

/// What the pipe does with a record.
#[derive(Debug, Clone, PartialEq)]
pub(crate) enum Disposition {
    Drop,
    /// `protocol.message.sent`: counted into the open rollup window and
    /// never forwarded, because its payload carries the message content.
    CountSent,
    Rollup(FrameSample),
    Session(SessionInput),
    Forward(WireEvent),
}

/// Ambient facts the classifier needs but does not own.
#[derive(Debug, Clone, Copy)]
pub(crate) struct ClassifyCtx {
    /// Whether routing decisions carry the diagnostic score breakdown.
    pub(crate) routing_diagnostic: bool,
    /// The clock, for records the engine does not stamp.
    pub(crate) now_ms: i64,
}

/// Classifies one record.
pub(crate) fn classify(record: &TelemetryRecord, ctx: &ClassifyCtx) -> Disposition {
    match record {
        TelemetryRecord::Protocol(event) => classify_protocol(event, ctx.now_ms),
        TelemetryRecord::Mls(event) => classify_mls(event),
        TelemetryRecord::MetricsSnapshot(frame) => Disposition::Rollup(FrameSample {
            ts_ms: frame.timestamp_ms,
            neighbor_count: frame.neighbor_count as u64,
            ack_pending: frame.ack_pending as u64,
            retry_total: frame.retry_queue.total_count as u64,
            retry_critical: frame.retry_queue.critical_priority_count as u64,
            current_transport: frame.current_transport.map(TransportKey::from_transport),
            is_local_relay: frame.is_local_relay,
        }),
        TelemetryRecord::TransportState(event) => Disposition::Forward(WireEvent {
            ts_ms: event.timestamp_ms,
            data: WireEventData::TransportState {
                transport: TransportKey::from_transport(event.transport),
                previous: event.previous,
                current: event.current,
            },
        }),
        TelemetryRecord::Routing(decision) => Disposition::Forward(WireEvent {
            ts_ms: decision.timestamp_ms,
            data: WireEventData::RoutingDecision {
                phase: decision.phase,
                from: decision.from.map(TransportKey::from_transport),
                to: decision.to.map(TransportKey::from_transport),
                winning_score: decision.winning_score,
                reason_code: decision.reason_code,
                scores: if ctx.routing_diagnostic && !decision.scores.is_empty() {
                    Some(
                        decision
                            .scores
                            .iter()
                            .map(|(transport, score)| ScoreEntry {
                                transport: TransportKey::from_transport(*transport),
                                signal: score.signal,
                                proximity: score.proximity,
                                bandwidth: score.bandwidth,
                                congestion: score.congestion,
                                energy: score.energy,
                                reliability: score.reliability,
                                load: score.load,
                                total: score.total,
                            })
                            .collect(),
                    )
                } else {
                    None
                },
            },
        }),
        TelemetryRecord::Device(snapshot) => Disposition::Forward(WireEvent {
            ts_ms: snapshot.timestamp_ms,
            data: WireEventData::DeviceCapability {
                battery_level: snapshot.battery_level,
                is_charging: snapshot.is_charging,
                relay_role: snapshot.relay_role,
                changed_fields: snapshot.changed_fields,
            },
        }),
    }
}

/// Classifies a protocol event by variant.
///
/// Protocol events carry no timestamp of their own, so a forwarded one is
/// stamped with `now_ms`. Allocates only for the three variants that carry a
/// `reason`; a dropped event costs one match and nothing else.
pub(crate) fn classify_protocol(event: &Event, now_ms: i64) -> Disposition {
    let data = match event {
        Event::MessageSent { .. } => return Disposition::CountSent,
        Event::MessageDelivered {
            latency_ms,
            hop_count,
            transport,
            ..
        } => WireEventData::MessageDelivered {
            latency_ms: clamp_u64_i32(*latency_ms),
            hop_count: i16::from(*hop_count),
            transport: TransportKey::parse(transport),
        },
        Event::MessageFailed {
            reason,
            retry_count,
            ..
        } => WireEventData::MessageFailed {
            reason: reason.as_str().into(),
            retry_count: clamp_u64_i16(u64::from(*retry_count)),
        },
        Event::MessageDeferred {
            reason,
            retry_count,
            ..
        } => WireEventData::MessageDeferred {
            reason: reason.as_str().into(),
            retry_count: clamp_u64_i16(u64::from(*retry_count)),
        },
        Event::MessageRelayed {
            hop_count,
            remaining_ttl,
            ..
        } => WireEventData::MessageRelayed {
            hop_count: i16::from(*hop_count),
            remaining_ttl: i16::from(*remaining_ttl),
        },
        Event::RelayPromoted {
            connection_count,
            battery_level,
        } => WireEventData::RelayPromoted {
            connection_count: clamp_u64_i16(*connection_count as u64),
            battery_level: battery_level.map(i16::from),
        },
        Event::RelayDemoted { reason } => WireEventData::RelayDemoted {
            reason: reason.as_str().into(),
        },
        Event::RelayDemotedBattery {
            battery_level,
            min_required,
        } => WireEventData::RelayDemotedBattery {
            battery_level: i16::from(*battery_level),
            min_required: i16::from(*min_required),
        },
        Event::NeighborDiscovered { .. } | Event::NeighborLost { .. } => {
            return Disposition::Session(SessionInput::NeighborChurn);
        }
        // A drop, not a projection: nothing from the event reaches the wire
        // through this arm, which is why a wildcard is safe here where it
        // would not be in a privacy classifier. The name-pinned test keeps
        // the set of collected variants explicit.
        _ => return Disposition::Drop,
    };
    Disposition::Forward(WireEvent {
        ts_ms: now_ms,
        data,
    })
}

fn classify_mls(event: &MlsLifecycleEvent) -> Disposition {
    match event {
        MlsLifecycleEvent::Initialized {
            timestamp_ms,
            session_id,
            ..
        } => Disposition::Session(SessionInput::MlsInitialized {
            session_id: session_id.clone(),
            ts_ms: *timestamp_ms,
        }),
        MlsLifecycleEvent::SessionReady {
            timestamp_ms,
            session_id,
            ..
        } => Disposition::Session(SessionInput::MlsSessionReady {
            session_id: session_id.clone(),
            ts_ms: *timestamp_ms,
        }),
        MlsLifecycleEvent::EncryptionUsed { .. } => {
            Disposition::Session(SessionInput::EncryptionUsed)
        }
        MlsLifecycleEvent::DecryptionFailed {
            timestamp_ms,
            context,
            failure_kind,
            ..
        } => Disposition::Forward(WireEvent {
            ts_ms: *timestamp_ms,
            data: WireEventData::MlsDecryptionFailed {
                failure_kind: *failure_kind,
                context: *context,
            },
        }),
        MlsLifecycleEvent::SessionMissing {
            timestamp_ms,
            context,
            ..
        } => Disposition::Forward(WireEvent {
            ts_ms: *timestamp_ms,
            data: WireEventData::MlsSessionMissing { context: *context },
        }),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::telemetry::record::tests::event_exemplars;

    #[test]
    fn every_protocol_event_is_classified_by_its_stable_name() {
        // The ten allowlisted names are collected; everything else is dropped.
        // A variant added to `Event` lands in the wildcard, which this test
        // reports as "dropped" rather than letting it slip through unseen.
        for event in event_exemplars() {
            let name = event.telemetry_name();
            let collected = !matches!(classify_protocol(&event, 0), Disposition::Drop);
            assert_eq!(
                collected,
                PROTOCOL_ALLOWLIST.contains(&name),
                "{name}: collected={collected}, allowlisted={}",
                PROTOCOL_ALLOWLIST.contains(&name)
            );
        }
    }

    #[test]
    fn message_sent_is_counted_and_its_content_never_projected() {
        let event = Event::MessageSent {
            message_id: "m".into(),
            sender: "a".into(),
            recipient: "b".into(),
            content: "secret".into(),
            priority: "medium".into(),
            requires_ack: false,
            timestamp: 0,
            lamport_clock: 0,
            forward_info: None,
        };
        assert_eq!(classify_protocol(&event, 0), Disposition::CountSent);
    }

    #[test]
    fn a_forwarded_protocol_event_carries_only_its_documented_fields() {
        let event = Event::MessageDelivered {
            message_id: "m".into(),
            latency_ms: 240,
            hop_count: 1,
            transport: "ble".into(),
        };
        let Disposition::Forward(wire) = classify_protocol(&event, 7) else {
            panic!("delivered is forwarded");
        };
        assert_eq!(wire.ts_ms, 7);
        assert_eq!(
            wire.data.to_json(),
            serde_json::json!({ "latency_ms": 240, "hop_count": 1, "transport": "ble" })
        );
    }
}
