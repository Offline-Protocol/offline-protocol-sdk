//! The wire contract, pinned twice.
//!
//! `fixtures/wire-batch-v1.json` is the canonical `POST /v1/events` body for
//! `wire_version: 1`. The ingest service keeps a byte-identical copy and tests
//! its deserializer against it; this crate's copy is what the SDK's telemetry
//! pipe is tested against. Each repository can only test its own codec against
//! its own copy, so `canonical_fixture_is_byte_identical_to_the_upstream_copy`
//! pins the bytes with a hash: a change on either side fails one of the two
//! suites instead of drifting silently.
//!
//! `fixtures/wire-batch-v1-reason.json` is this crate's addition: the same
//! envelope carrying the raw `reason` field on the three event types that have
//! one. It is generated from the typed structs and committed, so the ingest
//! team can diff a proposed server change against exactly what a client sends.
//! Regenerate with `UPDATE_FIXTURES=1 cargo test -p offline-protocol-telemetry-wire`
//! and commit the result; the test fails until the committed bytes match.
//!
//! The remaining tests are the upstream contract tests, ported so the copy
//! carries the same guarantees the original did.

use std::collections::BTreeMap;

use offline_protocol_telemetry_wire::batch::{
    Batch, BatchId, OsKind, SessionId, CURRENT_WIRE_VERSION,
};
use offline_protocol_telemetry_wire::event::{
    TYPE_MESSAGE_DEFERRED, TYPE_MESSAGE_FAILED, TYPE_RELAY_DEMOTED,
};
use offline_protocol_telemetry_wire::{classify, Event, RawEvent};
use serde_json::json;
use uuid::Uuid;

const FIXTURE: &str = include_str!("../fixtures/wire-batch-v1.json");
const REASON_FIXTURE_PATH: &str = "fixtures/wire-batch-v1-reason.json";

/// FNV-1a over the upstream `wire-batch-v1.json` at the commit the sources
/// were copied from. Computed over the file's bytes, so any edit to either
/// copy, whitespace included, moves it.
const UPSTREAM_FIXTURE_FNV1A64: u64 = 0x1e7b_4e7d_503d_b025;
const UPSTREAM_FIXTURE_LEN: usize = 1777;

fn fnv1a64(bytes: &[u8]) -> u64 {
    bytes.iter().fold(0xcbf2_9ce4_8422_2325_u64, |hash, byte| {
        (hash ^ u64::from(*byte)).wrapping_mul(0x0100_0000_01b3)
    })
}

fn parse() -> Batch {
    serde_json::from_str(FIXTURE).expect("canonical wire fixture must deserialize into Batch")
}

/// Classify a fixture event and hand back the typed payload it produced.
fn classify_event(event: &RawEvent) -> Event {
    classify(&event.event_type, &event.data)
        .unwrap_or_else(|e| panic!("fixture event `{}` must classify: {e}", event.event_type))
}

#[test]
fn canonical_fixture_is_byte_identical_to_the_upstream_copy() {
    let bytes = FIXTURE.as_bytes();
    assert_eq!(
        (bytes.len(), fnv1a64(bytes)),
        (UPSTREAM_FIXTURE_LEN, UPSTREAM_FIXTURE_FNV1A64),
        "fixtures/wire-batch-v1.json no longer matches the ingest service's copy. \
         The two are the wire contract and must change together: update the \
         upstream fixture first, then copy it here and re-pin the hash."
    );
}

#[test]
fn canonical_fixture_deserializes_with_every_envelope_field_intact() {
    let batch = parse();

    assert_eq!(batch.wire_version, CURRENT_WIRE_VERSION);
    assert_eq!(batch.app_version, "1.4.2");
    assert_eq!(batch.sdk_version, "0.24.0");
    assert_eq!(batch.os_major, 18);
    assert!(matches!(batch.os, OsKind::Ios));
    assert_eq!(
        batch.batch_id.0.to_string(),
        "3f2504e0-4f89-41d3-9a0c-0305e82c3301"
    );
    assert_eq!(
        batch.session_id.0.to_string(),
        "6ba7b810-9dad-11d1-80b4-00c04fd430c8"
    );
    let device = batch
        .device_id
        .as_ref()
        .expect("fixture carries a device_id");
    assert_eq!(device.0, "a3f2b4c5d6e7f8091a2b3c4d5e6f7a8b");

    assert_eq!(batch.events.len(), 3);
    assert_eq!(batch.events[0].event_type, "protocol.message.delivered");
    assert_eq!(batch.events[0].ts_ms, 1_747_000_000_000);
    assert_eq!(batch.events[1].event_type, "mesh_metrics_rollup");
    assert_eq!(batch.events[2].event_type, "mesh_session_summary");
}

#[test]
fn every_fixture_payload_classifies_into_the_fields_the_ingest_binds() {
    let batch = parse();

    let Event::MessageDelivered(delivered) = classify_event(&batch.events[0]) else {
        panic!("expected MessageDelivered");
    };
    assert_eq!(delivered.latency_ms, Some(240));
    assert_eq!(delivered.hop_count, Some(1));
    assert_eq!(delivered.transport.as_deref(), Some("ble"));

    let Event::MetricsRollup(rollup) = classify_event(&batch.events[1]) else {
        panic!("expected MetricsRollup");
    };
    assert_eq!(rollup.bucket_duration_s, Some(60));
    assert_eq!(rollup.neighbor_count_max, Some(9));
    assert_eq!(rollup.ack_pending_max, Some(3));
    assert_eq!(rollup.is_local_relay_duration_s, Some(12));
    assert_eq!(
        rollup.current_transport_at_bucket_end.as_deref(),
        Some("ble")
    );
    // The wire-to-column renames: every one is a name the two sides must
    // agree on that deliberately differs from the struct field.
    assert_eq!(rollup.retry_queue_critical_max, Some(1));
    assert_eq!(rollup.sends_attempted, Some(12));
    assert_eq!(rollup.sends_succeeded, Some(11));
    assert_eq!(rollup.sends_failed, Some(1));
    assert_eq!(rollup.bytes_sent, Some(0));
    assert_eq!(rollup.bytes_received, Some(0));
    let transport = rollup
        .transport_time_ms
        .expect("fixture rollup carries transport_time_ms");
    assert_eq!(transport.ble, Some(40_000));
    assert_eq!(transport.wifi_direct, Some(15_000));
    assert_eq!(transport.internet, Some(5_000));
    assert_eq!(transport.none, Some(0));
    // `bucket_start_ms` has no field of its own: the row's `ts` comes from
    // the envelope, and the client stamps both from the same value.
    assert_eq!(
        batch.events[1].data["bucket_start_ms"], batch.events[1].ts_ms,
        "the client stamps ts_ms from bucket_start_ms; a divergence silently shifts the bucket"
    );

    let Event::SessionSummary(summary) = classify_event(&batch.events[2]) else {
        panic!("expected SessionSummary");
    };
    assert_eq!(summary.session_duration_s, Some(312));
    assert_eq!(summary.mls_session_ready_latency_p50_ms, Some(88));
    assert_eq!(summary.routing_switches, Some(14));
    assert_eq!(summary.routing_escalations, Some(6));
    assert_eq!(summary.mls_encryption_used_count, Some(47));
    let reasons = summary
        .escalation_reasons
        .as_ref()
        .expect("escalation_reasons is nested on the wire, not flat");
    assert_eq!(reasons.poor_signal, Some(3));
    assert_eq!(reasons.low_success_rate, Some(1));
    assert_eq!(reasons.retry_threshold, Some(2));
    assert_eq!(reasons.congestion, Some(0));
    assert_eq!(reasons.low_ttl, Some(0));
    // A real 0, not an absent key: `current_unavailable` is a switch reason
    // in the SDK and never an escalation one, so the client sends the bucket
    // rather than dropping it.
    assert_eq!(reasons.current_unavailable, Some(0));
    assert_eq!(
        summary.routing_escalations,
        Some(
            reasons.poor_signal.unwrap_or(0)
                + reasons.low_success_rate.unwrap_or(0)
                + reasons.retry_threshold.unwrap_or(0)
                + reasons.congestion.unwrap_or(0)
                + reasons.low_ttl.unwrap_or(0)
                + reasons.current_unavailable.unwrap_or(0)
        )
    );
}

/// Wire keys that are deliberately not bound to a column, keyed by event
/// type. Anything else absent from the typed struct is a mistake.
const COLUMNLESS_WIRE_KEYS: &[(&str, &str)] = &[("mesh_metrics_rollup", "bucket_start_ms")];

/// The typed payload, re-serialized under its wire names; `None` for
/// `Extension`, whose catchall stores `data` verbatim and drops nothing.
fn wire_shape_bound_by(event: &RawEvent) -> Option<serde_json::Value> {
    use serde_json::to_value;
    let json = match classify_event(event) {
        Event::MetricsRollup(d) => to_value(d),
        Event::MessageDelivered(d) => to_value(d),
        Event::MessageFailed(d) | Event::MessageDeferred(d) => to_value(d),
        Event::MessageRelayed(d) => to_value(d),
        Event::RelayPromoted(d) => to_value(d),
        Event::RelayDemoted(d) => to_value(d),
        Event::RelayDemotedBattery(d) => to_value(d),
        Event::RoutingDecision(d) => to_value(d),
        Event::TransportState(d) => to_value(d),
        Event::DeviceCapability(d) => to_value(d),
        Event::MlsDecryptionFailed(d) => to_value(d),
        Event::MlsSessionMissing(d) => to_value(d),
        Event::SessionSummary(d) => to_value(d),
        Event::Extension { .. } => return None,
    }
    .expect("typed payload must serialize");
    Some(json)
}

fn assert_every_key_is_bound(
    event_type: &str,
    path: &str,
    data: &serde_json::Map<String, serde_json::Value>,
    shape: &serde_json::Map<String, serde_json::Value>,
) {
    for (key, value) in data {
        let full = if path.is_empty() {
            key.clone()
        } else {
            format!("{path}.{key}")
        };
        let bound = shape.get(key);
        let columnless = COLUMNLESS_WIRE_KEYS.contains(&(event_type, full.as_str()));
        assert!(
            bound.is_some() || columnless,
            "`{full}` in the `{event_type}` payload is not a field the ingest binds; \
             it would parse as raw JSON and be discarded by `classify`. Fix the name \
             to one of {:?}, or add it to COLUMNLESS_WIRE_KEYS if it has no column.",
            shape.keys().collect::<Vec<_>>()
        );
        if let (Some(nested_data), Some(serde_json::Value::Object(nested_shape))) =
            (value.as_object(), bound)
        {
            assert_every_key_is_bound(event_type, &full, nested_data, nested_shape);
        }
    }
}

#[test]
fn no_fixture_payload_key_is_silently_dropped_by_the_ingest() {
    for event in parse().events.iter().chain(reason_batch().events.iter()) {
        let Some(shape) = wire_shape_bound_by(event) else {
            continue;
        };
        assert_every_key_is_bound(
            &event.event_type,
            "",
            event
                .data
                .as_object()
                .expect("fixture payloads are JSON objects"),
            shape.as_object().expect("typed payloads are JSON objects"),
        );
    }
}

#[test]
fn reserializing_the_fixture_reproduces_it_exactly() {
    // Field order differs between the struct and the JSON, which is fine:
    // objects are unordered. Compare as parsed values.
    let ours: serde_json::Value =
        serde_json::to_value(parse()).expect("Batch must serialize back to JSON");
    let theirs: serde_json::Value =
        serde_json::from_str(FIXTURE).expect("fixture must parse as raw JSON");
    assert_eq!(ours, theirs, "round-trip changed the wire shape");
}

#[test]
fn device_id_is_omitted_rather_than_nulled_when_absent() {
    let mut value: serde_json::Value = serde_json::from_str(FIXTURE).expect("fixture parses");
    value
        .as_object_mut()
        .expect("fixture is an object")
        .remove("device_id");
    let batch: Batch =
        serde_json::from_value(value).expect("a batch without device_id must still deserialize");
    assert!(batch.device_id.is_none());
    let reserialized = serde_json::to_value(&batch).expect("batch serializes");
    assert!(
        !reserialized
            .as_object()
            .expect("batch is an object")
            .contains_key("device_id"),
        "device_id must be skipped when None, not emitted as null"
    );
}

#[test]
fn an_explicit_null_device_id_is_accepted_as_absent() {
    let mut value: serde_json::Value = serde_json::from_str(FIXTURE).expect("fixture parses");
    value.as_object_mut().expect("fixture is an object")["device_id"] = serde_json::Value::Null;
    let batch: Batch = serde_json::from_value(value)
        .expect("an explicit null device_id parses as absent, same as omitting it");
    assert!(batch.device_id.is_none());
}

#[test]
fn unknown_fields_are_ignored_so_newer_clients_do_not_break_an_older_ingest() {
    let mut value: serde_json::Value = serde_json::from_str(FIXTURE).expect("fixture parses");
    value.as_object_mut().expect("fixture is an object").insert(
        "some_field_a_future_client_adds".into(),
        serde_json::Value::Bool(true),
    );
    let batch: Batch = serde_json::from_value(value)
        .expect("unknown top-level fields must be ignored, not rejected");
    assert_eq!(batch.events.len(), 3);
}

/// The batch the `reason` fixture is generated from: the three event types
/// that carry a reason, each with the raw SDK token in place of a
/// client-side `reason_class`.
fn reason_batch() -> Batch {
    Batch {
        batch_id: BatchId(
            Uuid::parse_str("3f2504e0-4f89-41d3-9a0c-0305e82c3302").expect("literal uuid"),
        ),
        session_id: SessionId(
            Uuid::parse_str("6ba7b810-9dad-11d1-80b4-00c04fd430c8").expect("literal uuid"),
        ),
        device_id: None,
        wire_version: CURRENT_WIRE_VERSION,
        app_version: "1.4.2".into(),
        os: OsKind::Android,
        os_major: 15,
        sdk_version: "0.26.0".into(),
        events: vec![
            RawEvent {
                event_type: TYPE_MESSAGE_FAILED.into(),
                ts_ms: 1_747_000_000_000,
                data: json!({ "reason": "Max retries exceeded", "retry_count": 5 }),
            },
            RawEvent {
                event_type: TYPE_MESSAGE_DEFERRED.into(),
                ts_ms: 1_747_000_001_000,
                data: json!({ "reason": "recipient_unreachable", "retry_count": 1 }),
            },
            RawEvent {
                event_type: TYPE_RELAY_DEMOTED.into(),
                ts_ms: 1_747_000_002_000,
                data: json!({ "reason": "no traffic carried for other devices recently" }),
            },
        ],
    }
}

#[test]
fn reason_fixture_matches_the_bytes_the_typed_structs_produce() {
    let generated = serde_json::to_string_pretty(&reason_batch()).expect("batch serializes") + "\n";
    let path = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join(REASON_FIXTURE_PATH);
    if std::env::var_os("UPDATE_FIXTURES").is_some() {
        std::fs::write(&path, &generated).expect("write the regenerated fixture");
    }
    let committed = std::fs::read_to_string(&path)
        .unwrap_or_else(|e| panic!("cannot read {}: {e}", path.display()));
    assert_eq!(
        committed,
        generated,
        "{} differs from what the typed structs serialize to; regenerate it with \
         UPDATE_FIXTURES=1 and commit, so the ingest team diffs against real bytes",
        path.display()
    );
}

#[test]
fn reason_rides_beside_reason_class_and_is_read_back_verbatim() {
    let batch: Batch = serde_json::from_str(
        &std::fs::read_to_string(
            std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join(REASON_FIXTURE_PATH),
        )
        .expect("reason fixture is committed"),
    )
    .expect("reason fixture deserializes");
    let mut seen: BTreeMap<&str, Option<String>> = BTreeMap::new();
    for event in &batch.events {
        match classify_event(event) {
            Event::MessageFailed(d) | Event::MessageDeferred(d) => {
                assert!(d.reason_class.is_none(), "the client never classifies");
                seen.insert(
                    if event.event_type == TYPE_MESSAGE_FAILED {
                        "failed"
                    } else {
                        "deferred"
                    },
                    d.reason,
                );
            }
            Event::RelayDemoted(d) => {
                assert!(d.reason_class.is_none());
                seen.insert("demoted", d.reason);
            }
            other => panic!("unexpected event in the reason fixture: {other:?}"),
        }
    }
    assert_eq!(seen["failed"].as_deref(), Some("Max retries exceeded"));
    assert_eq!(seen["deferred"].as_deref(), Some("recipient_unreachable"));
    assert_eq!(
        seen["demoted"].as_deref(),
        Some("no traffic carried for other devices recently")
    );
    // Absent, never null: a pre-`reason` ingest must see wire v1 JSON.
    let serialized = serde_json::to_value(
        classify(TYPE_RELAY_DEMOTED, &json!({}))
            .ok()
            .and_then(|e| match e {
                Event::RelayDemoted(d) => Some(d),
                _ => None,
            })
            .expect("empty demoted payload classifies"),
    )
    .expect("serializes");
    assert!(!serialized
        .as_object()
        .expect("object")
        .contains_key("reason"));
}

#[test]
fn os_kind_names_every_host_the_sdk_ships_a_binding_for() {
    for (kind, wire) in [
        (OsKind::Ios, "ios"),
        (OsKind::Android, "android"),
        (OsKind::Linux, "linux"),
        (OsKind::Macos, "macos"),
        (OsKind::Windows, "windows"),
        (OsKind::Other, "other"),
    ] {
        assert_eq!(serde_json::to_value(kind).expect("serializes"), json!(wire));
        let back: OsKind = serde_json::from_value(json!(wire)).expect("round-trips");
        assert_eq!(back, kind);
    }
}
