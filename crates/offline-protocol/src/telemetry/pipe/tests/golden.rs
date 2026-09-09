//! The golden test: the pipe against the TypeScript client it replaces.
//!
//! A deterministic scenario of typed records, lifecycle transitions and
//! flush points is built here. Two things come out of it:
//!
//! - `tests/fixtures/pipe-golden-input-v1.json`: the same scenario encoded
//!   in the envelope shape the earlier React Native bridge delivered, which
//!   is the only input the TypeScript client can read. Regenerated with
//!   `UPDATE_FIXTURES=1`.
//! - `tests/fixtures/pipe-golden-expected-v1.json`: the batches the
//!   TypeScript client produced for that input, written by
//!   `tests/tools/golden-oracle.js` against a build of the frozen
//!   `mesh-analytics` checkout.
//!
//! `pipe_matches_the_typescript_oracle` replays the scenario through the
//! pipe with the same clock and compares batch by batch. Two documented
//! divergences are normalized before comparing: the pipe sends the raw
//! `reason` where the client sent a `reason_class`, and the pipe stamps its
//! own `sdk_version`. Everything else, including the session cut, the
//! rollup arithmetic, the summary deltas and the rescue summary, must match.

use std::collections::HashMap;
use std::path::PathBuf;

use serde_json::{json, Map, Value};

use crate::events::Event;
use crate::mls_observability::{DecryptionFailureKind, MlsLifecycleEvent, MlsOperationContext};
use crate::telemetry::device::DeviceCapabilitySnapshot;
use crate::telemetry::metrics_snapshot::MetricsFrame;
use crate::telemetry::pipe::host::AppState;
use crate::telemetry::pipe::store::Backend;
use crate::telemetry::pipe::tests::{inline_pipe, test_config, CapturingClient, FakeClock};
use crate::telemetry::routing::{RoutingDecision, RoutingPhase, RoutingReasonCode};
use crate::telemetry::transport_state::TransportStateEvent;
use crate::telemetry::TelemetryRecord;
use offline_protocol_reliability::{DeduplicatorMode, DeduplicatorStats, RetryQueueStats};
use offline_protocol_router::RelayRole;
use offline_protocol_transport::{TransportStatus, TransportType};

/// Minute-aligned so the first rollup bucket starts at the scenario start.
const T0: i64 = 1_747_000_020_000;

enum Step {
    Record(TelemetryRecord),
    AppState(AppState),
    Flush,
    EndSession,
}

struct Scenario {
    steps: Vec<(i64, Step)>,
}

impl Scenario {
    fn new() -> Self {
        Self { steps: Vec::new() }
    }

    fn at(&mut self, seconds: f64, step: Step) {
        self.steps
            .push((T0 + (seconds * 1000.0).round() as i64, step));
    }

    fn record(&mut self, seconds: f64, record: TelemetryRecord) {
        self.at(seconds, Step::Record(record));
    }

    fn sorted(mut self) -> Vec<(i64, Step)> {
        // Stable: records at the same instant keep insertion order.
        self.steps.sort_by_key(|(t, _)| *t);
        self.steps
    }
}

fn frame(t: f64, i: usize) -> TelemetryRecord {
    let transport = match (i / 8) % 5 {
        0 | 1 => Some(TransportType::BLE),
        2 => Some(TransportType::WiFiDirect),
        3 => Some(TransportType::Internet),
        _ => None,
    };
    TelemetryRecord::MetricsSnapshot(Box::new(MetricsFrame {
        timestamp_ms: T0 + (t * 1000.0) as i64,
        transports: Vec::new(),
        retry_queue: RetryQueueStats {
            total_count: i % 4,
            ready_count: 0,
            critical_priority_count: usize::from(i % 7 == 0),
            high_priority_count: 0,
            medium_priority_count: 0,
            low_priority_count: 0,
        },
        dedup: DeduplicatorStats {
            total_tracked: 0,
            recent_tracked: 0,
            capacity_used_percent: 0,
            false_positive_rate: None,
            mode: DeduplicatorMode::HashMap,
        },
        ack_pending: i % 3,
        neighbor_count: i % 5 + 1,
        is_local_relay: (16..24).contains(&i),
        current_transport: transport,
    }))
}

fn sent(n: u32) -> TelemetryRecord {
    TelemetryRecord::Protocol(Box::new(Event::MessageSent {
        message_id: format!("m{n}"),
        sender: "off1sender".into(),
        recipient: "off1recipient".into(),
        content: "hello".into(),
        priority: "medium".into(),
        requires_ack: true,
        timestamp: 0,
        lamport_clock: n as u64,
        forward_info: None,
    }))
}

fn delivered(n: u32, latency: u64, hops: u8, transport: TransportType) -> TelemetryRecord {
    TelemetryRecord::Protocol(Box::new(Event::MessageDelivered {
        message_id: format!("m{n}"),
        latency_ms: latency,
        hop_count: hops,
        transport: transport.label().to_string(),
    }))
}

fn mls(event: MlsLifecycleEvent) -> TelemetryRecord {
    TelemetryRecord::Mls(event)
}

fn mls_at(t: f64, build: fn(i64, String) -> MlsLifecycleEvent, session: &str) -> TelemetryRecord {
    mls(build(T0 + (t * 1000.0).round() as i64, session.to_string()))
}

fn initialized(ts: i64, sid: String) -> MlsLifecycleEvent {
    MlsLifecycleEvent::Initialized {
        timestamp_ms: ts,
        session_id: sid,
        group_id: None,
        peer_id: Some("peerhash".into()),
        context: MlsOperationContext::Initialize,
        error_category: None,
    }
}

fn ready(ts: i64, sid: String) -> MlsLifecycleEvent {
    MlsLifecycleEvent::SessionReady {
        timestamp_ms: ts,
        session_id: sid,
        group_id: None,
        peer_id: Some("peerhash".into()),
        context: MlsOperationContext::SessionLookup,
        error_category: None,
    }
}

fn encryption_used(ts: i64, sid: String) -> MlsLifecycleEvent {
    MlsLifecycleEvent::EncryptionUsed {
        timestamp_ms: ts,
        session_id: sid,
        group_id: None,
        peer_id: None,
        context: MlsOperationContext::Send,
        error_category: None,
    }
}

fn routing(
    t: f64,
    phase: RoutingPhase,
    from: Option<TransportType>,
    to: Option<TransportType>,
    score: Option<f32>,
    reason: Option<RoutingReasonCode>,
) -> TelemetryRecord {
    TelemetryRecord::Routing(Box::new(RoutingDecision {
        timestamp_ms: T0 + (t * 1000.0).round() as i64,
        phase,
        from,
        to,
        winning_score: score,
        reason_code: reason,
        scores: Vec::new(),
    }))
}

fn transport_state(
    t: f64,
    transport: TransportType,
    previous: TransportStatus,
    current: TransportStatus,
) -> TelemetryRecord {
    TelemetryRecord::TransportState(TransportStateEvent {
        timestamp_ms: T0 + (t * 1000.0).round() as i64,
        transport,
        previous,
        current,
    })
}

fn device(t: f64, battery: Option<u8>, charging: bool, changed: u8) -> TelemetryRecord {
    TelemetryRecord::Device(DeviceCapabilitySnapshot {
        timestamp_ms: T0 + (t * 1000.0).round() as i64,
        battery_level: battery,
        is_charging: charging,
        relay_role: RelayRole::Regular,
        changed_fields: changed,
    })
}

fn scenario() -> Vec<(i64, Step)> {
    let mut s = Scenario::new();
    // Session one: 36 frames at 5 s.
    for i in 0..36 {
        s.record(i as f64 * 5.0, frame(i as f64 * 5.0, i));
    }
    s.record(
        1.0,
        transport_state(
            1.0,
            TransportType::BLE,
            TransportStatus::Disconnected,
            TransportStatus::Available,
        ),
    );
    s.record(1.0, device(1.0, Some(80), false, 7));
    s.record(
        2.0,
        routing(
            2.0,
            RoutingPhase::Selected,
            None,
            Some(TransportType::BLE),
            Some(84.5),
            Some(RoutingReasonCode::InitialSelection),
        ),
    );
    s.record(
        3.0,
        TelemetryRecord::Protocol(Box::new(Event::NeighborDiscovered {
            peer_id: "off1peer".into(),
            transport: format!("{:?}", TransportType::BLE),
            rssi: Some(-60),
        })),
    );
    for (n, (t_sent, latency, hops, transport)) in [
        (6.0, 240, 1, TransportType::BLE),
        (11.0, 310, 2, TransportType::BLE),
        (32.0, 180, 1, TransportType::WiFiDirect),
        (60.0, 90, 1, TransportType::WiFiDirect),
        (89.0, 400, 3, TransportType::BLE),
        (121.0, 75, 1, TransportType::Internet),
        (149.0, 220, 2, TransportType::BLE),
    ]
    .into_iter()
    .enumerate()
    {
        let n = n as u32 + 1;
        s.record(t_sent, sent(n));
        s.record(t_sent + 1.0, delivered(n, latency, hops, transport));
    }
    s.record(38.0, sent(90));
    s.record(
        40.0,
        TelemetryRecord::Protocol(Box::new(Event::MessageFailed {
            message_id: "m90".into(),
            reason: "Max retries exceeded".into(),
            retry_count: 5,
        })),
    );
    s.record(
        50.0,
        TelemetryRecord::Protocol(Box::new(Event::MessageRelayed {
            message_id: "r1".into(),
            sender: "off1a".into(),
            recipient: "off1b".into(),
            hop_count: 2,
            remaining_ttl: 6,
        })),
    );
    s.record(
        80.0,
        TelemetryRecord::Protocol(Box::new(Event::RelayPromoted {
            connection_count: 3,
            battery_level: Some(77),
        })),
    );
    s.record(98.0, sent(91));
    s.record(
        100.0,
        TelemetryRecord::Protocol(Box::new(Event::MessageDeferred {
            message_id: "m91".into(),
            recipient: "off1recipient".into(),
            reason: "recipient_unreachable".into(),
            retry_count: 1,
            next_retry_at: None,
        })),
    );
    s.record(
        140.0,
        TelemetryRecord::Protocol(Box::new(Event::RelayDemoted {
            reason: "no traffic carried for other devices recently".into(),
        })),
    );
    s.record(
        170.0,
        TelemetryRecord::Protocol(Box::new(Event::RelayDemotedBattery {
            battery_level: 12,
            min_required: 20,
        })),
    );
    s.record(170.0, device(170.0, Some(12), false, 1));
    s.record(
        180.0,
        TelemetryRecord::Protocol(Box::new(Event::NeighborLost {
            peer_id: "off1peer".into(),
        })),
    );
    // MLS: three pairs (100, 250, 150 ms), four encryption uses, two failures.
    s.record(20.0, mls_at(20.0, initialized, "s1"));
    s.record(20.1, mls_at(20.1, ready, "s1"));
    s.record(21.0, mls_at(21.0, encryption_used, "s1"));
    s.record(22.0, mls_at(22.0, encryption_used, "s1"));
    s.record(70.0, mls_at(70.0, initialized, "s2"));
    s.record(70.25, mls_at(70.25, ready, "s2"));
    s.record(71.0, mls_at(71.0, encryption_used, "s2"));
    s.record(
        75.0,
        mls(MlsLifecycleEvent::DecryptionFailed {
            timestamp_ms: T0 + 75_000,
            session_id: "s2".into(),
            group_id: None,
            peer_id: None,
            context: MlsOperationContext::Receive,
            error_category: None,
            failure_kind: DecryptionFailureKind::InvalidCiphertext,
        }),
    );
    s.record(
        76.0,
        mls(MlsLifecycleEvent::SessionMissing {
            timestamp_ms: T0 + 76_000,
            session_id: "s9".into(),
            group_id: None,
            peer_id: None,
            context: MlsOperationContext::Send,
            error_category: None,
        }),
    );
    s.record(130.0, mls_at(130.0, initialized, "s3"));
    s.record(130.15, mls_at(130.15, ready, "s3"));
    s.record(131.0, mls_at(131.0, encryption_used, "s3"));
    // Routing: switches and escalations, including the apply confirmations
    // that must not be counted.
    s.record(
        44.0,
        transport_state(
            44.0,
            TransportType::WiFiDirect,
            TransportStatus::Unavailable,
            TransportStatus::Available,
        ),
    );
    s.record(
        45.0,
        routing(
            45.0,
            RoutingPhase::Escalated,
            Some(TransportType::BLE),
            Some(TransportType::WiFiDirect),
            None,
            Some(RoutingReasonCode::PoorSignal),
        ),
    );
    s.record(
        46.0,
        routing(
            46.0,
            RoutingPhase::Switched,
            Some(TransportType::BLE),
            Some(TransportType::WiFiDirect),
            Some(72.25),
            Some(RoutingReasonCode::EscalationApplied),
        ),
    );
    s.record(
        47.0,
        routing(
            47.0,
            RoutingPhase::Escalated,
            Some(TransportType::BLE),
            Some(TransportType::WiFiDirect),
            None,
            Some(RoutingReasonCode::FallbackSuccess),
        ),
    );
    s.record(
        108.0,
        transport_state(
            108.0,
            TransportType::Internet,
            TransportStatus::Disconnected,
            TransportStatus::Available,
        ),
    );
    s.record(
        110.0,
        routing(
            110.0,
            RoutingPhase::Switched,
            Some(TransportType::WiFiDirect),
            Some(TransportType::Internet),
            Some(91.0),
            Some(RoutingReasonCode::PrimarySuccess),
        ),
    );
    s.record(
        160.0,
        routing(
            160.0,
            RoutingPhase::Escalated,
            Some(TransportType::Internet),
            Some(TransportType::BLE),
            None,
            Some(RoutingReasonCode::RetryThreshold),
        ),
    );
    s.record(
        165.0,
        routing(
            165.0,
            RoutingPhase::ScoreUpdated,
            None,
            None,
            Some(66.5),
            None,
        ),
    );
    // Flush points, then the background.
    s.at(30.0, Step::Flush);
    s.at(95.0, Step::Flush);
    s.at(200.0, Step::Flush);
    s.at(205.0, Step::AppState(AppState::Background));
    s.at(206.0, Step::AppState(AppState::Background));
    s.at(207.0, Step::AppState(AppState::Inactive));
    // Observed in the background, after the boundary summary.
    s.record(210.0, mls_at(210.0, encryption_used, "s3"));
    s.record(
        215.0,
        routing(
            215.0,
            RoutingPhase::Switched,
            Some(TransportType::Internet),
            Some(TransportType::BLE),
            Some(60.0),
            Some(RoutingReasonCode::CurrentUnavailable),
        ),
    );
    s.at(260.0, Step::AppState(AppState::Active));
    s.at(262.0, Step::Flush);
    // Session two.
    for (k, i) in (36..40).enumerate() {
        let t = 265.0 + k as f64 * 5.0;
        s.record(t, frame(t, i));
    }
    s.record(266.0, sent(200));
    s.record(267.0, delivered(200, 130, 1, TransportType::BLE));
    s.at(300.0, Step::EndSession);
    s.at(301.0, Step::Flush);
    s.sorted()
}

// ---- The old envelope, for the TypeScript oracle -------------------------

fn transport_camel(t: TransportType) -> &'static str {
    match t {
        TransportType::Internet => "internet",
        TransportType::BLE => "ble",
        TransportType::WiFiDirect => "wifiDirect",
        TransportType::Reticulum => "reticulum",
        TransportType::Nostr => "nostr",
    }
}

fn status_camel(s: TransportStatus) -> &'static str {
    match s {
        TransportStatus::Available => "available",
        TransportStatus::Unavailable => "unavailable",
        TransportStatus::Connecting => "connecting",
        TransportStatus::Disconnected => "disconnected",
        TransportStatus::Error => "error",
    }
}

fn phase_camel(p: RoutingPhase) -> &'static str {
    match p {
        RoutingPhase::ScoreUpdated => "scoreUpdated",
        RoutingPhase::Selected => "selected",
        RoutingPhase::Switched => "switched",
        RoutingPhase::Escalated => "escalated",
    }
}

fn reason_camel(r: RoutingReasonCode) -> &'static str {
    match r {
        RoutingReasonCode::InitialSelection => "initialSelection",
        RoutingReasonCode::PrimarySelected => "primarySelected",
        RoutingReasonCode::PrimarySuccess => "primarySuccess",
        RoutingReasonCode::FallbackSuccess => "fallbackSuccess",
        RoutingReasonCode::EscalationApplied => "escalationApplied",
        RoutingReasonCode::CurrentUnavailable => "currentUnavailable",
        RoutingReasonCode::RetryThreshold => "retryThreshold",
        RoutingReasonCode::PoorSignal => "poorSignal",
        RoutingReasonCode::Congestion => "congestion",
        RoutingReasonCode::LowTtl => "lowTtl",
        RoutingReasonCode::LowSuccessRate => "lowSuccessRate",
    }
}

/// Encodes a record the way the 0.25 bridge adapter did: the shape
/// `normalizeRecord` in the TypeScript client consumes.
fn legacy_envelope(record: &TelemetryRecord) -> Value {
    match record {
        TelemetryRecord::Protocol(event) => json!({
            "category": "protocol",
            "eventJson": event.to_json().expect("event serializes"),
        }),
        TelemetryRecord::Mls(event) => json!({
            "category": "mls",
            "eventJson": serde_json::to_string(event).expect("mls event serializes"),
        }),
        TelemetryRecord::MetricsSnapshot(f) => {
            let mut frame = json!({
                "timestampMs": f.timestamp_ms,
                "transports": [],
                "retryQueue": {
                    "totalCount": f.retry_queue.total_count,
                    "readyCount": f.retry_queue.ready_count,
                    "criticalPriorityCount": f.retry_queue.critical_priority_count,
                    "highPriorityCount": f.retry_queue.high_priority_count,
                    "mediumPriorityCount": f.retry_queue.medium_priority_count,
                    "lowPriorityCount": f.retry_queue.low_priority_count,
                },
                "dedup": {
                    "totalTracked": f.dedup.total_tracked,
                    "recentTracked": f.dedup.recent_tracked,
                    "capacityUsedPercent": f.dedup.capacity_used_percent,
                    "mode": "hashMap",
                },
                "ackPending": f.ack_pending,
                "neighborCount": f.neighbor_count,
                "isLocalRelay": f.is_local_relay,
            });
            if let Some(t) = f.current_transport {
                frame["currentTransport"] = json!(transport_camel(t));
            }
            json!({ "category": "metricsFrame", "frame": frame })
        }
        TelemetryRecord::TransportState(e) => json!({
            "category": "transportState",
            "event": {
                "timestampMs": e.timestamp_ms,
                "transport": transport_camel(e.transport),
                "previous": status_camel(e.previous),
                "current": status_camel(e.current),
            }
        }),
        TelemetryRecord::Routing(d) => {
            let mut decision = json!({
                "timestampMs": d.timestamp_ms,
                "phase": phase_camel(d.phase),
                "scores": [],
            });
            if let Some(t) = d.from {
                decision["from"] = json!(transport_camel(t));
            }
            if let Some(t) = d.to {
                decision["to"] = json!(transport_camel(t));
            }
            if let Some(s) = d.winning_score {
                decision["winningScore"] = json!(s);
            }
            if let Some(r) = d.reason_code {
                decision["reasonCode"] = json!(reason_camel(r));
            }
            json!({ "category": "routingDecision", "decision": decision })
        }
        TelemetryRecord::Device(s) => {
            let mut snapshot = json!({
                "timestampMs": s.timestamp_ms,
                "isCharging": s.is_charging,
                "relayRole": match s.relay_role { RelayRole::Relay => "relay", RelayRole::Regular => "regular" },
                "changedFields": s.changed_fields,
            });
            if let Some(b) = s.battery_level {
                snapshot["batteryLevel"] = json!(b);
            }
            json!({ "category": "deviceCapability", "snapshot": snapshot })
        }
    }
}

fn input_fixture(steps: &[(i64, Step)]) -> Value {
    let steps: Vec<Value> = steps
        .iter()
        .map(|(t, step)| match step {
            Step::Record(record) => {
                json!({ "t_ms": t, "kind": "record", "record": legacy_envelope(record) })
            }
            Step::AppState(state) => json!({ "t_ms": t, "kind": "app_state", "state": match state {
                AppState::Active => "active",
                AppState::Background => "background",
                AppState::Inactive => "inactive",
            }}),
            Step::Flush => json!({ "t_ms": t, "kind": "flush" }),
            Step::EndSession => json!({ "t_ms": t, "kind": "end_session" }),
        })
        .collect();
    json!({
        "config": {
            "apiKey": "mp_test_key",
            "appId": "app_test",
            "appVersion": "1.4.2",
            "sdkVersion": "<sdk>",
            "routingDiagnostic": false,
        },
        "platform": { "os": "ios", "osMajor": 18 },
        "start_ms": T0,
        "steps": steps,
    })
}

fn fixtures_dir() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures")
}

#[test]
fn golden_input_fixture_is_current() {
    let generated =
        serde_json::to_string_pretty(&input_fixture(&scenario())).expect("serializes") + "\n";
    let path = fixtures_dir().join("pipe-golden-input-v1.json");
    if std::env::var_os("UPDATE_FIXTURES").is_some() {
        std::fs::create_dir_all(fixtures_dir()).expect("fixtures dir");
        std::fs::write(&path, &generated).expect("write input fixture");
    }
    // Compared with the carriage returns removed. `.gitattributes` pins this
    // file to LF, which is the real fix, but a working tree checked out before
    // that rule landed still holds CRLF, and every line would then differ for
    // a reason that has nothing to do with the scenario.
    let committed = std::fs::read_to_string(&path)
        .unwrap_or_else(|e| panic!("cannot read {}: {e}", path.display()))
        .replace("\r\n", "\n");
    assert_eq!(
        committed, generated,
        "the golden input fixture no longer matches the scenario; regenerate with \
         UPDATE_FIXTURES=1, re-run tests/tools/golden-oracle.js, and commit both"
    );
}

// ---- The replay and the comparison ---------------------------------------

fn replay_through_the_pipe(steps: &[(i64, Step)]) -> Vec<Value> {
    let clock = FakeClock::at(T0);
    let client = CapturingClient::accepting();
    let pipe = inline_pipe(&test_config(), &clock, client.clone(), Backend::Memory);
    let sink = pipe.sink();
    for (t, step) in steps {
        clock.set(*t);
        match step {
            Step::Record(record) => match record {
                TelemetryRecord::Protocol(event) => {
                    assert!(sink.try_emit_protocol_event(event));
                }
                other => sink.emit(other),
            },
            Step::AppState(state) => pipe.notify_app_state(*state),
            Step::Flush => pipe.flush(),
            Step::EndSession => pipe.end_session(),
        }
    }
    client.batches()
}

/// Ordinal ids, a placeholder SDK version, and the documented `reason`
/// divergence removed from both sides.
fn normalize(batches: &[Value], drop_key: &str) -> Vec<Value> {
    let mut sessions: HashMap<String, String> = HashMap::new();
    batches
        .iter()
        .enumerate()
        .map(|(i, batch)| {
            let mut batch = batch.clone();
            let obj = batch.as_object_mut().expect("batch is an object");
            obj.insert("batch_id".into(), json!(format!("batch-{}", i + 1)));
            let session = obj["session_id"].as_str().expect("session id").to_string();
            let next = sessions.len() + 1;
            let ordinal = sessions
                .entry(session)
                .or_insert_with(|| format!("session-{next}"))
                .clone();
            obj.insert("session_id".into(), json!(ordinal));
            obj.insert("sdk_version".into(), json!("<sdk>"));
            if let Some(events) = obj.get_mut("events").and_then(Value::as_array_mut) {
                for event in events {
                    if let Some(data) = event.get_mut("data").and_then(Value::as_object_mut) {
                        data.remove(drop_key);
                    }
                }
            }
            batch
        })
        .collect()
}

fn assert_close(ours: &Value, theirs: &Value, path: &str) {
    match (ours, theirs) {
        (Value::Number(a), Value::Number(b)) => {
            let (a, b) = (a.as_f64().unwrap_or(0.0), b.as_f64().unwrap_or(0.0));
            let tolerance = 1e-6 * a.abs().max(b.abs()).max(1.0);
            assert!((a - b).abs() <= tolerance, "{path}: {a} vs {b}");
        }
        (Value::Object(a), Value::Object(b)) => {
            let mut keys: Vec<&String> = a.keys().chain(b.keys()).collect();
            keys.sort();
            keys.dedup();
            for key in keys {
                let child = format!("{path}.{key}");
                match (a.get(key), b.get(key)) {
                    (Some(x), Some(y)) => assert_close(x, y, &child),
                    (x, y) => panic!("{child}: pipe={x:?} oracle={y:?}"),
                }
            }
        }
        (Value::Array(a), Value::Array(b)) => {
            assert_eq!(
                a.len(),
                b.len(),
                "{path}: length {} vs {}",
                a.len(),
                b.len()
            );
            for (i, (x, y)) in a.iter().zip(b).enumerate() {
                assert_close(x, y, &format!("{path}[{i}]"));
            }
        }
        (a, b) => assert_eq!(a, b, "{path}"),
    }
}

#[test]
fn pipe_matches_the_typescript_oracle() {
    let path = fixtures_dir().join("pipe-golden-expected-v1.json");
    let expected: Value = serde_json::from_str(
        &std::fs::read_to_string(&path)
            .unwrap_or_else(|e| panic!("cannot read {}: {e}", path.display())),
    )
    .expect("expected fixture parses");
    let oracle = expected["batches"].as_array().expect("batches array");

    let ours = replay_through_the_pipe(&scenario());
    assert_eq!(
        ours.len(),
        oracle.len(),
        "batch count: pipe {} vs oracle {}",
        ours.len(),
        oracle.len()
    );

    // The divergence the plan records: the client classified `reason` on
    // the device; the pipe sends it raw.
    for (i, batch) in ours.iter().enumerate() {
        for (j, event) in batch["events"]
            .as_array()
            .expect("events")
            .iter()
            .enumerate()
        {
            let event_type = event["type"].as_str().expect("type");
            let data = event["data"].as_object().expect("data");
            if matches!(
                event_type,
                "protocol.message.failed" | "protocol.message.deferred" | "protocol.relay.demoted"
            ) {
                assert!(
                    data["reason"].is_string(),
                    "batch {i} event {j}: raw reason"
                );
                assert!(data.get("reason_class").is_none(), "batch {i} event {j}");
                assert_eq!(
                    oracle[i]["events"][j]["data"]["reason_class"]
                        .as_str()
                        .is_some(),
                    true,
                    "the oracle classified the same event"
                );
            }
        }
    }

    let ours = normalize(&ours, "reason");
    let theirs = normalize(oracle, "reason_class");
    for (i, (a, b)) in ours.iter().zip(theirs.iter()).enumerate() {
        assert_close(a, b, &format!("batches[{i}]"));
    }
}

#[test]
fn the_scenario_exercises_every_wire_type_and_both_sessions() {
    let batches = replay_through_the_pipe(&scenario());
    let mut types: Vec<String> = batches
        .iter()
        .flat_map(|b| b["events"].as_array().expect("events").clone())
        .map(|e| e["type"].as_str().expect("type").to_string())
        .collect();
    types.sort();
    types.dedup();
    let expected = [
        "mesh_metrics_rollup",
        "mesh_session_summary",
        "mls.decryption_failed",
        "mls.session_missing",
        "protocol.device.capability_changed",
        "protocol.message.deferred",
        "protocol.message.delivered",
        "protocol.message.failed",
        "protocol.message.relayed",
        "protocol.relay.demoted",
        "protocol.relay.demoted_battery",
        "protocol.relay.promoted",
        "protocol.routing.decision",
        "protocol.transport.state_changed",
    ];
    assert_eq!(types, expected);
    let sessions: std::collections::BTreeSet<&str> = batches
        .iter()
        .map(|b| b["session_id"].as_str().expect("session"))
        .collect();
    // Session one, session two; the session `end_session` opens after them
    // sees no event, so no batch carries it.
    assert_eq!(sessions.len(), 2);

    let summaries: Vec<&Value> = batches
        .iter()
        .flat_map(|b| b["events"].as_array().expect("events"))
        .filter(|e| e["type"] == "mesh_session_summary")
        .collect();
    assert_eq!(
        summaries.len(),
        3,
        "background, rescue on foreground, end_session"
    );
    let first = &summaries[0]["data"];
    assert_eq!(first["mls_session_ready_latency_p50_ms"], json!(150));
    assert_eq!(first["mls_encryption_used_count"], json!(4));
    assert_eq!(first["routing_switches"], json!(2));
    assert_eq!(first["routing_escalations"], json!(2));
    assert_eq!(first["escalation_reasons"]["poor_signal"], json!(1));
    assert_eq!(first["escalation_reasons"]["retry_threshold"], json!(1));
    assert_eq!(first["session_duration_s"], json!(205));
    let rescue = &summaries[1]["data"];
    assert_eq!(rescue["mls_encryption_used_count"], json!(1));
    assert_eq!(rescue["routing_switches"], json!(1));
    assert!(rescue.get("mls_session_ready_latency_p50_ms").is_none());
}

#[allow(dead_code)]
fn _map_is_used(_: Map<String, Value>) {}
