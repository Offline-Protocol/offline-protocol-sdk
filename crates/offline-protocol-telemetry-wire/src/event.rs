//! Event surface.
//!
//! Two layers live here:
//!
//! * [`RawEvent`] — the M1 envelope: an opaque, type-tagged record. The
//!   `type` field is the stable wire identity (see SDK `ALL_TELEMETRY_NAMES`
//!   at `record.rs:108-184`, or one of our own derived names like
//!   `mesh_metrics_rollup` and `mesh_session_summary`); `ts_ms` is the emit
//!   time; `data` is the per-type payload. The batch always deserializes into
//!   `RawEvent`s — they are cheap and never reject.
//!
//! * The M3 typed layer ([`Event`] + the per-type `*Data` structs +
//!   [`classify`]). After the envelope parses, the ingest server classifies
//!   each `(type, &data)` into a typed variant and routes it to its hypertable
//!   (plan §7.5). Unknown types fall through to [`Event::Extension`] and land
//!   in the `raw_extension_events` catchall (plan §2.6).
//!
//! ## Why the field types mirror the storage schema, not the wire spec
//!
//! Each `*Data` struct types its fields to the **column** width of the matching
//! `raw_*` hypertable (migration `0005_typed_raw_events.sql`): `SMALLINT`→`i16`,
//! `INTEGER`→`i32`, `BIGINT`→`i64`, `REAL`→`f32`. That is deliberate: a JSON
//! number outside the column's range then fails `serde_json::from_value`, so
//! [`classify`] rejects the *event* (plan §7.5: "out-of-range drops the event")
//! instead of letting the value reach the bulk INSERT, where a single bad row
//! would fail the whole batch. The plan's §2.2/§2.3 `u16`/`u32` hints are the
//! wire's logical ranges; the migration already widened them where a `u16`
//! could exceed `i16` (see the §3.1 type-choice notes), and these structs
//! follow the migration, not the hint.
//!
//! ### Negative values are accepted within the column range — by design
//!
//! The `raw_*` columns are **signed** (`SMALLINT`/`INTEGER`/`BIGINT`) with no
//! `CHECK (col >= 0)` constraint, even though most of these fields are
//! logically unsigned (counts, durations, latencies, battery levels). So the
//! boundary `classify` enforces is exactly the column's storable range —
//! `[i16::MIN, i16::MAX]` for `SMALLINT`, etc. — **not** the logical `[0, …]`
//! range. A `hop_count: -1` deserializes and is stored verbatim.
//!
//! This is deliberate, not an oversight:
//!   * The raw tables are an *unaggregated landing zone*; the privacy guarantee
//!     is the 30-day retention drop, not field-level sanitisation. The M4
//!     aggregation pass is the right place to clamp/discard nonsense (a
//!     `WHERE col >= 0`), where the rule lives in one query instead of being
//!     duplicated across ~40 nullable wire fields here.
//!   * Per-field non-negativity would need a custom `deserialize_with` on every
//!     numeric field — large surface area — and would *reject the whole event*
//!     for a single negative, losing its other (valid) fields. Storing a
//!     negative that an aggregation query can trivially filter is the better
//!     failure mode.
//!   * It keeps "passed `classify`" meaning precisely "fits the column", which
//!     is the invariant the fan-out INSERT relies on: a classified event can
//!     never fail its bind. (Overflow past the column range *is* still rejected.)
//!
//! The `negative_value_within_column_range_is_accepted` test pins this so the
//! choice can't silently flip.
//!
//! All event-specific fields are `Option<_>`: the typed columns are nullable
//! (migration 0005), the SDK listener emits a field only when present (its
//! `pick` projection), and serde maps a missing `Option` field to `None`. The
//! structs do **not** set `deny_unknown_fields` — an SDK that adds a field to a
//! known event must not start failing validation (plan §2.6 forward-compat
//! ethos); unknown keys within a known type are ignored.

use serde::de::DeserializeOwned;
use serde::{Deserialize, Serialize};
use serde_json::Value;

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct RawEvent {
    #[serde(rename = "type")]
    pub event_type: String,
    pub ts_ms: i64,
    pub data: Value,
}

// --- Wire `type` discriminators ----------------------------------------------
//
// The stable per-event identity strings (plan §2.3 / §7.2). Pulled out as
// constants so [`classify`] and any future caller can't drift apart on a typo,
// and so the closed set is greppable.

pub const TYPE_METRICS_ROLLUP: &str = "mesh_metrics_rollup";
pub const TYPE_SESSION_SUMMARY: &str = "mesh_session_summary";
pub const TYPE_MESSAGE_DELIVERED: &str = "protocol.message.delivered";
pub const TYPE_MESSAGE_FAILED: &str = "protocol.message.failed";
pub const TYPE_MESSAGE_DEFERRED: &str = "protocol.message.deferred";
pub const TYPE_MESSAGE_RELAYED: &str = "protocol.message.relayed";
pub const TYPE_RELAY_PROMOTED: &str = "protocol.relay.promoted";
pub const TYPE_RELAY_DEMOTED: &str = "protocol.relay.demoted";
pub const TYPE_RELAY_DEMOTED_BATTERY: &str = "protocol.relay.demoted_battery";
pub const TYPE_ROUTING_DECISION: &str = "protocol.routing.decision";
pub const TYPE_TRANSPORT_STATE: &str = "protocol.transport.state_changed";
pub const TYPE_DEVICE_CAPABILITY: &str = "protocol.device.capability_changed";
pub const TYPE_MLS_DECRYPTION_FAILED: &str = "mls.decryption_failed";
pub const TYPE_MLS_SESSION_MISSING: &str = "mls.session_missing";

// --- Shared nested wire shapes -----------------------------------------------

/// Nested `transport_time_ms` object (plan §2.1 / §2.2). The server flattens
/// these into the per-transport `*_time_ms` columns. `none` is a real bucket in
/// the rollup ("no transport currently selected") but has no column in
/// `raw_session_summary`, so the session-summary insert ignores it.
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct TransportTimeMs {
    pub ble: Option<i32>,
    pub wifi_direct: Option<i32>,
    pub internet: Option<i32>,
    pub none: Option<i32>,
}

/// Nested `escalation_reasons` object on `mesh_session_summary` (plan §2.2).
/// Flattened into the `escalation_*` columns server-side.
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct EscalationReasons {
    pub poor_signal: Option<i32>,
    pub low_success_rate: Option<i32>,
    pub retry_threshold: Option<i32>,
    pub congestion: Option<i32>,
    pub low_ttl: Option<i32>,
    pub current_unavailable: Option<i32>,
}

// --- Per-type `data` payloads ------------------------------------------------

/// `mesh_metrics_rollup` (plan §2.1, Appendix A). Wire→column renames:
/// `retry_queue_critical_count_max`→`retry_queue_critical_max`, the
/// `sends_*_sum`/`bytes_*_sum` suffixes are dropped, and `transport_time_ms`
/// is flattened. `bucket_start_ms` is on the wire but maps to the row `ts`
/// (taken from the envelope's `ts_ms`), so it has no field here.
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct MetricsRollupData {
    pub bucket_duration_s: Option<i16>,
    pub neighbor_count_avg: Option<f32>,
    pub neighbor_count_max: Option<i32>,
    pub neighbor_count_p50: Option<i32>,
    pub ack_pending_avg: Option<f32>,
    pub ack_pending_max: Option<i32>,
    pub retry_queue_total_avg: Option<f32>,
    pub retry_queue_total_max: Option<i32>,
    #[serde(rename = "retry_queue_critical_count_max")]
    pub retry_queue_critical_max: Option<i32>,
    pub transport_time_ms: Option<TransportTimeMs>,
    pub is_local_relay_duration_s: Option<i32>,
    pub current_transport_at_bucket_end: Option<String>,
    #[serde(rename = "sends_attempted_sum")]
    pub sends_attempted: Option<i32>,
    #[serde(rename = "sends_succeeded_sum")]
    pub sends_succeeded: Option<i32>,
    #[serde(rename = "sends_failed_sum")]
    pub sends_failed: Option<i32>,
    #[serde(rename = "bytes_sent_sum")]
    pub bytes_sent: Option<i64>,
    #[serde(rename = "bytes_received_sum")]
    pub bytes_received: Option<i64>,
}

/// `protocol.message.delivered` — `{ latency_ms, hop_count, transport }`.
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct MessageDeliveredData {
    pub latency_ms: Option<i32>,
    pub hop_count: Option<i16>,
    pub transport: Option<String>,
}

/// `protocol.message.{failed,deferred}` — `{ reason_class, retry_count }`.
/// Both events share this shape (plan §2.3); the `reason_class` enum is
/// classified client-side (Appendix B — 21 buckets at SDK 0.24.1, the
/// original twelve retained so older rows keep their meaning) and arrives as
/// a string. The column is free `TEXT` with no CHECK, so a client that grows
/// a bucket needs no migration here.
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct ReasonRetryData {
    pub reason_class: Option<String>,
    pub retry_count: Option<i16>,
    /// The SDK's own fixed reason token or literal, verbatim. Additive over
    /// wire v1: the SDK sends it in place of classifying on the device, and
    /// `reason_class` is derived from it server-side. Absent from batches
    /// produced by the earlier TypeScript client, so it stays optional.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub reason: Option<String>,
}

/// `protocol.message.relayed` — `{ hop_count, remaining_ttl }`.
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct MessageRelayedData {
    pub hop_count: Option<i16>,
    pub remaining_ttl: Option<i16>,
}

/// `protocol.relay.promoted` — `{ connection_count, battery_level }`.
/// Emitted from the `RelayActivity::Began` arm of `evaluate_relay_role`
/// (SDK PR #110, shipped in 0.11.0 — the release that introduced telemetry,
/// so no SDK the client supports lacks it).
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct RelayPromotedData {
    pub connection_count: Option<i16>,
    pub battery_level: Option<i16>,
}

/// `protocol.relay.demoted` — `{ reason_class }`. Emitted from
/// `evaluate_relay_role` (SDK ≥ 0.11.0): the configuration gate and the
/// `RelayActivity::Ceased` arm. Those two literals are the whole vocabulary —
/// the anticipated `battery_low`/`role_lost`/`shutdown` were never written
/// (plan Appendix B).
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct RelayDemotedData {
    pub reason_class: Option<String>,
    /// The SDK's demotion literal, verbatim; see [`ReasonRetryData::reason`].
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub reason: Option<String>,
}

/// `protocol.relay.demoted_battery` — `{ battery_level, min_required }`.
/// Emitted from the battery gate in `evaluate_relay_role` (SDK ≥ 0.11.0).
/// Battery demotion is its own typed event, which is why it never reaches
/// the `reason_class` classifier.
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct RelayDemotedBatteryData {
    pub battery_level: Option<i16>,
    pub min_required: Option<i16>,
}

/// `protocol.routing.decision` — `{ phase, from?, to?, winning_score?,
/// reason_code? }`. Wire `from`/`to` map to the `from_transport`/`to_transport`
/// columns; the optional `scores[]` array (only present under
/// `routing_diagnostic`) has no column and is ignored.
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct RoutingDecisionData {
    pub phase: Option<String>,
    #[serde(rename = "from")]
    pub from_transport: Option<String>,
    #[serde(rename = "to")]
    pub to_transport: Option<String>,
    pub winning_score: Option<f32>,
    pub reason_code: Option<String>,
}

/// `protocol.transport.state_changed` — `{ transport, previous, current }`.
/// Wire `previous`/`current` map to the `previous_status`/`current_status`
/// columns (per-transport Available/Unavailable status, not selection).
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct TransportStateData {
    pub transport: Option<String>,
    #[serde(rename = "previous")]
    pub previous_status: Option<String>,
    #[serde(rename = "current")]
    pub current_status: Option<String>,
}

/// `protocol.device.capability_changed` — `{ battery_level?, is_charging,
/// relay_role, changed_fields }`. `changed_fields` is the SMALLINT bitmask
/// (`CHANGED_BATTERY=1, CHANGED_CHARGING=2, CHANGED_RELAY_ROLE=4`).
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct DeviceCapabilityData {
    pub battery_level: Option<i16>,
    pub is_charging: Option<bool>,
    pub relay_role: Option<String>,
    pub changed_fields: Option<i16>,
}

/// `mls.decryption_failed` — `{ failure_kind, context }`. Wire `context` maps
/// to the `mls_context` column. All IDs are stripped at the listener.
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct MlsDecryptionFailedData {
    pub failure_kind: Option<String>,
    #[serde(rename = "context")]
    pub mls_context: Option<String>,
}

/// `mls.session_missing` — `{ context }`. Wire `context` maps to `mls_context`.
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct MlsSessionMissingData {
    #[serde(rename = "context")]
    pub mls_context: Option<String>,
}

/// `mesh_session_summary` (plan §2.2 + §2.4) — one row per session, emitted on
/// app-background. The nested `transport_time_ms`/`escalation_reasons` objects
/// are flattened into columns server-side; `mls_decryption_failures_by_kind`
/// is stored as JSONB as-is.
///
/// The client populates six of these: `session_duration_s`,
/// `routing_switches`, `routing_escalations`, `escalation_reasons`,
/// `mls_session_ready_latency_p50_ms`, `mls_encryption_used_count`. Those six
/// land in **eleven** columns — `escalation_reasons` flattens into six — of
/// which the aggregation job reads ten and has no other source for any of
/// them (`pipeline.rs` step 3e, `app_plane.rs`). `session_duration_s` is the
/// eleventh and the exception: stored, but read by neither plane today.
///
/// The rest stay nullable and absent by design, not by omission: each has a
/// second source the job already reads (`mesh_metrics_rollup` or a typed
/// `raw_*` table), so a value here would duplicate a populated column rather
/// than fill an empty one.
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct SessionSummaryData {
    pub session_duration_s: Option<i32>,
    pub delivery_latency_p50_ms: Option<i32>,
    pub delivery_latency_p99_ms: Option<i32>,
    pub messages_sent: Option<i32>,
    pub messages_delivered: Option<i32>,
    pub messages_failed: Option<i32>,
    pub transport_time_ms: Option<TransportTimeMs>,
    pub routing_switches: Option<i32>,
    pub routing_escalations: Option<i32>,
    pub escalation_reasons: Option<EscalationReasons>,
    pub mls_decryption_failures_by_kind: Option<Value>,
    pub mls_session_ready_latency_p50_ms: Option<i32>,
    pub mls_encryption_used_count: Option<i32>,
    pub relay_duration_s: Option<i32>,
    pub relay_battery_at_demote: Option<Vec<i16>>,
    pub battery_start: Option<i16>,
    pub battery_end: Option<i16>,
}

// --- The typed event ---------------------------------------------------------

/// A classified, schema-validated event ready to route to its hypertable.
///
/// One variant per typed `raw_*` table, plus [`Event::Extension`] for any
/// `type` the server doesn't recognise — the forward-compat catchall that
/// keeps a new SDK event from failing the batch (plan §2.6).
#[derive(Debug, Clone)]
pub enum Event {
    MetricsRollup(MetricsRollupData),
    MessageDelivered(MessageDeliveredData),
    MessageFailed(ReasonRetryData),
    MessageDeferred(ReasonRetryData),
    MessageRelayed(MessageRelayedData),
    RelayPromoted(RelayPromotedData),
    RelayDemoted(RelayDemotedData),
    RelayDemotedBattery(RelayDemotedBatteryData),
    RoutingDecision(RoutingDecisionData),
    TransportState(TransportStateData),
    DeviceCapability(DeviceCapabilityData),
    MlsDecryptionFailed(MlsDecryptionFailedData),
    MlsSessionMissing(MlsSessionMissingData),
    SessionSummary(SessionSummaryData),
    /// Unknown `type` — stored verbatim in `raw_extension_events`. `ext_name`
    /// is the wire `type` string; `payload` is the untouched `data`.
    Extension {
        ext_name: String,
        payload: Value,
    },
}

/// Deserialize `data` into `T`, tagging the error with the event type so a
/// per-event rejection reason names which event and field failed.
///
/// Deserializes straight from `&Value` (which implements `Deserializer`)
/// rather than `serde_json::from_value(data.clone())` — on the hot ingest
/// path that avoids cloning the entire `data` JSON (including fields with no
/// column, like a routing `scores[]` array) just to throw the clone away
/// after the typed struct is built. The `DeserializeOwned` bound still holds:
/// it is `for<'de> Deserialize<'de>`, so it satisfies the borrowed `'de` of
/// `&Value`.
fn parse<T: DeserializeOwned>(event_type: &str, data: &Value) -> Result<T, String> {
    T::deserialize(data).map_err(|e| format!("{event_type}: {e}"))
}

/// Classify a wire `(type, data)` pair into a typed [`Event`].
///
/// Known types are validated against their `*Data` schema; an `Err` is a
/// per-event rejection reason (the batch still succeeds for the rest, per
/// plan §7.5). Unknown types never error — they become [`Event::Extension`].
pub fn classify(event_type: &str, data: &Value) -> Result<Event, String> {
    Ok(match event_type {
        TYPE_METRICS_ROLLUP => Event::MetricsRollup(parse(event_type, data)?),
        TYPE_SESSION_SUMMARY => Event::SessionSummary(parse(event_type, data)?),
        TYPE_MESSAGE_DELIVERED => Event::MessageDelivered(parse(event_type, data)?),
        TYPE_MESSAGE_FAILED => Event::MessageFailed(parse(event_type, data)?),
        TYPE_MESSAGE_DEFERRED => Event::MessageDeferred(parse(event_type, data)?),
        TYPE_MESSAGE_RELAYED => Event::MessageRelayed(parse(event_type, data)?),
        TYPE_RELAY_PROMOTED => Event::RelayPromoted(parse(event_type, data)?),
        TYPE_RELAY_DEMOTED => Event::RelayDemoted(parse(event_type, data)?),
        TYPE_RELAY_DEMOTED_BATTERY => Event::RelayDemotedBattery(parse(event_type, data)?),
        TYPE_ROUTING_DECISION => Event::RoutingDecision(parse(event_type, data)?),
        TYPE_TRANSPORT_STATE => Event::TransportState(parse(event_type, data)?),
        TYPE_DEVICE_CAPABILITY => Event::DeviceCapability(parse(event_type, data)?),
        TYPE_MLS_DECRYPTION_FAILED => Event::MlsDecryptionFailed(parse(event_type, data)?),
        TYPE_MLS_SESSION_MISSING => Event::MlsSessionMissing(parse(event_type, data)?),
        other => Event::Extension {
            ext_name: other.to_string(),
            payload: data.clone(),
        },
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn classifies_metrics_rollup_with_renames_and_nesting() {
        let data = json!({
            "bucket_start_ms": 1_747_000_000_000_i64,
            "bucket_duration_s": 60,
            "neighbor_count_avg": 4.5,
            "neighbor_count_max": 9,
            "neighbor_count_p50": 4,
            "ack_pending_avg": 1.25,
            "ack_pending_max": 3,
            "retry_queue_total_avg": 2.0,
            "retry_queue_total_max": 5,
            "retry_queue_critical_count_max": 1,
            "transport_time_ms": { "ble": 40000, "wifi_direct": 15000, "internet": 5000, "none": 0 },
            "is_local_relay_duration_s": 12,
            "current_transport_at_bucket_end": "ble",
            "sends_attempted_sum": 100,
            "sends_succeeded_sum": 98,
            "sends_failed_sum": 2,
            "bytes_sent_sum": 5_000_000_000_i64,
            "bytes_received_sum": 6_000_000_000_i64
        });
        let Event::MetricsRollup(d) = classify(TYPE_METRICS_ROLLUP, &data).unwrap() else {
            panic!("expected MetricsRollup");
        };
        assert_eq!(d.retry_queue_critical_max, Some(1));
        assert_eq!(d.sends_attempted, Some(100));
        assert_eq!(d.bytes_received, Some(6_000_000_000));
        let t = d.transport_time_ms.unwrap();
        assert_eq!(t.ble, Some(40000));
        assert_eq!(t.none, Some(0));
        assert_eq!(d.current_transport_at_bucket_end.as_deref(), Some("ble"));
    }

    #[test]
    fn classifies_routing_decision_from_to_renames() {
        let data = json!({
            "phase": "select",
            "from": "ble",
            "to": "internet",
            "winning_score": 0.87,
            "reason_code": "poor_signal",
            "scores": [0.1, 0.87]  // no column — must be ignored, not rejected
        });
        let Event::RoutingDecision(d) = classify(TYPE_ROUTING_DECISION, &data).unwrap() else {
            panic!("expected RoutingDecision");
        };
        assert_eq!(d.from_transport.as_deref(), Some("ble"));
        assert_eq!(d.to_transport.as_deref(), Some("internet"));
    }

    #[test]
    fn classifies_mls_context_rename() {
        let data = json!({ "context": "receive" });
        let Event::MlsSessionMissing(d) = classify(TYPE_MLS_SESSION_MISSING, &data).unwrap() else {
            panic!("expected MlsSessionMissing");
        };
        assert_eq!(d.mls_context.as_deref(), Some("receive"));
    }

    #[test]
    fn transport_state_status_renames() {
        let data = json!({ "transport": "ble", "previous": "available", "current": "unavailable" });
        let Event::TransportState(d) = classify(TYPE_TRANSPORT_STATE, &data).unwrap() else {
            panic!("expected TransportState");
        };
        assert_eq!(d.previous_status.as_deref(), Some("available"));
        assert_eq!(d.current_status.as_deref(), Some("unavailable"));
    }

    #[test]
    fn empty_data_object_is_all_none_not_rejected() {
        // Absent optional fields are valid — the columns are nullable.
        let Event::MessageDelivered(d) = classify(TYPE_MESSAGE_DELIVERED, &json!({})).unwrap()
        else {
            panic!("expected MessageDelivered");
        };
        assert!(d.latency_ms.is_none() && d.hop_count.is_none() && d.transport.is_none());
    }

    #[test]
    fn out_of_range_smallint_is_rejected() {
        // hop_count is SMALLINT (i16, max 32767). A larger value must reject
        // the event at classify time so it never reaches the bulk INSERT.
        let data = json!({ "hop_count": 40000 });
        let err = classify(TYPE_MESSAGE_DELIVERED, &data).unwrap_err();
        assert!(
            err.contains(TYPE_MESSAGE_DELIVERED),
            "reason names the type: {err}"
        );
    }

    #[test]
    fn wrong_field_type_is_rejected() {
        // latency_ms must be a number, not a string.
        let data = json!({ "latency_ms": "fast" });
        assert!(classify(TYPE_MESSAGE_DELIVERED, &data).is_err());
    }

    #[test]
    fn non_object_data_for_known_type_is_rejected() {
        assert!(classify(TYPE_MESSAGE_FAILED, &Value::Null).is_err());
        assert!(classify(TYPE_MESSAGE_FAILED, &json!("nope")).is_err());
    }

    #[test]
    fn unknown_type_falls_through_to_extension() {
        let data = json!({ "anything": 1 });
        let Event::Extension { ext_name, payload } =
            classify("protocol.brand.new_event", &data).unwrap()
        else {
            panic!("expected Extension");
        };
        assert_eq!(ext_name, "protocol.brand.new_event");
        assert_eq!(payload, data);
    }

    #[test]
    fn failed_and_deferred_share_reason_retry_shape() {
        let data = json!({ "reason_class": "retries_exhausted", "retry_count": 3 });
        assert!(matches!(
            classify(TYPE_MESSAGE_FAILED, &data).unwrap(),
            Event::MessageFailed(_)
        ));
        assert!(matches!(
            classify(TYPE_MESSAGE_DEFERRED, &data).unwrap(),
            Event::MessageDeferred(_)
        ));
    }

    #[test]
    fn session_summary_partial_payload() {
        // A partial payload is legal at every field: an older client (or one
        // whose session saw no routing) sends a subset, and every field is
        // `Option`. The full set the current client emits is pinned by
        // `tests/wire_contract.rs` against the shared fixture.
        let data = json!({ "session_duration_s": 312, "mls_session_ready_latency_p50_ms": 88 });
        let Event::SessionSummary(d) = classify(TYPE_SESSION_SUMMARY, &data).unwrap() else {
            panic!("expected SessionSummary");
        };
        assert_eq!(d.session_duration_s, Some(312));
        assert_eq!(d.mls_session_ready_latency_p50_ms, Some(88));
        assert!(d.messages_sent.is_none());
    }

    #[test]
    fn negative_value_within_column_range_is_accepted() {
        // Contract pin (see module doc "Negative values are accepted within the
        // column range"): the signed columns have no CHECK (>= 0), so a
        // logically-impossible negative is in-range for SMALLINT and is
        // accepted, not rejected. Sanitising nonsense is M4's job, not the
        // landing zone's. If this ever needs to flip to rejection, this test —
        // and the module doc — must change together.
        let data = json!({ "hop_count": -1, "latency_ms": -5 });
        let Event::MessageDelivered(d) = classify(TYPE_MESSAGE_DELIVERED, &data).unwrap() else {
            panic!("expected MessageDelivered");
        };
        assert_eq!(d.hop_count, Some(-1));
        assert_eq!(d.latency_ms, Some(-5));
    }

    #[test]
    fn smallint_min_and_max_round_trip_through_classify() {
        // The struct width must match the SMALLINT column width exactly: i16
        // bounds classify cleanly, one past either bound rejects (proving the
        // type isn't accidentally wider than the column).
        for v in [i16::MIN, i16::MAX] {
            let data = json!({ "hop_count": v });
            let Event::MessageDelivered(d) = classify(TYPE_MESSAGE_DELIVERED, &data).unwrap()
            else {
                panic!("expected MessageDelivered for hop_count={v}");
            };
            assert_eq!(d.hop_count, Some(v));
        }
        assert!(classify(
            TYPE_MESSAGE_DELIVERED,
            &json!({ "hop_count": i16::MAX as i64 + 1 })
        )
        .is_err());
        assert!(classify(
            TYPE_MESSAGE_DELIVERED,
            &json!({ "hop_count": i16::MIN as i64 - 1 })
        )
        .is_err());
    }
}
