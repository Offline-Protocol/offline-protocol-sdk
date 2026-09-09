//! Overhead benchmark for the telemetry emission path.
//!
//! Budget targets (from TODO item #6):
//!   - `<5µs`  per emission at the default (Lifecycle) verbosity
//!   - `<25µs` per emission when `routing_diagnostic` is enabled
//!
//! These benches exercise the sink-facing path with `NoopTelemetrySink` so
//! the measurement isolates the SDK-side work (record construction, enum
//! dispatch, `sink.emit(&record)` indirect call) from whatever a real sink
//! would do.

use std::sync::Arc;

use criterion::{criterion_group, criterion_main, Criterion};
use offline_protocol::telemetry::device::{DeviceCapabilitySnapshot, CHANGED_BATTERY};
use offline_protocol::telemetry::metrics_snapshot::MetricsFrame;
use offline_protocol::telemetry::pipe::testing::start_pipe_for_bench;
use offline_protocol::telemetry::pipe::{TelemetryHost, TelemetryOs};
use offline_protocol::telemetry::routing::{RoutingDecision, RoutingPhase, RoutingReasonCode};
use offline_protocol::telemetry::transport_state::TransportStateEvent;
use offline_protocol::{Event, NoopTelemetrySink, TelemetryConfig, TelemetryRecord, TelemetrySink};
use offline_protocol_reliability::{DeduplicatorMode, DeduplicatorStats, RetryQueueStats};
use offline_protocol_router::RelayRole;
use offline_protocol_transport::{TransportStatus, TransportType};
use std::hint::black_box;

fn empty_retry_stats() -> RetryQueueStats {
    RetryQueueStats {
        total_count: 0,
        ready_count: 0,
        critical_priority_count: 0,
        high_priority_count: 0,
        medium_priority_count: 0,
        low_priority_count: 0,
    }
}

fn empty_dedup_stats() -> DeduplicatorStats {
    DeduplicatorStats {
        total_tracked: 0,
        recent_tracked: 0,
        capacity_used_percent: 0,
        false_positive_rate: None,
        mode: DeduplicatorMode::HashMap,
    }
}

fn sample_metrics_frame() -> MetricsFrame {
    MetricsFrame {
        timestamp_ms: 0,
        transports: Vec::new(),
        retry_queue: empty_retry_stats(),
        dedup: empty_dedup_stats(),
        ack_pending: 0,
        neighbor_count: 0,
        is_local_relay: false,
        current_transport: None,
    }
}

fn sample_transport_state() -> TransportStateEvent {
    TransportStateEvent {
        timestamp_ms: 0,
        transport: TransportType::BLE,
        previous: TransportStatus::Disconnected,
        current: TransportStatus::Available,
    }
}

fn sample_routing_decision() -> RoutingDecision {
    RoutingDecision {
        timestamp_ms: 0,
        phase: RoutingPhase::Selected,
        from: None,
        to: Some(TransportType::BLE),
        winning_score: Some(84.0),
        reason_code: Some(RoutingReasonCode::InitialSelection),
        scores: Vec::new(),
    }
}

fn sample_device_snapshot() -> DeviceCapabilitySnapshot {
    DeviceCapabilitySnapshot {
        timestamp_ms: 0,
        battery_level: Some(80),
        is_charging: true,
        relay_role: RelayRole::Regular,
        changed_fields: CHANGED_BATTERY,
    }
}

fn bench_emit_noop_sink(c: &mut Criterion) {
    let sink: Arc<dyn TelemetrySink> = Arc::new(NoopTelemetrySink);
    let mut group = c.benchmark_group("telemetry_emit_noop");

    group.bench_function("metrics_snapshot", |b| {
        b.iter(|| {
            let record = TelemetryRecord::MetricsSnapshot(Box::new(sample_metrics_frame()));
            sink.emit(black_box(&record));
        });
    });

    group.bench_function("transport_state", |b| {
        b.iter(|| {
            let record = TelemetryRecord::TransportState(sample_transport_state());
            sink.emit(black_box(&record));
        });
    });

    group.bench_function("routing_decision_lifecycle", |b| {
        b.iter(|| {
            let record = TelemetryRecord::Routing(Box::new(sample_routing_decision()));
            sink.emit(black_box(&record));
        });
    });

    group.bench_function("device_snapshot", |b| {
        b.iter(|| {
            let record = TelemetryRecord::Device(sample_device_snapshot());
            sink.emit(black_box(&record));
        });
    });

    group.finish();
}

fn bench_emit_noop_sink_diagnostic(c: &mut Criterion) {
    // Diagnostic-tier routing carries a populated score vector. We simulate
    // it here with 5 entries (one per transport) of mock TransportScore.
    // Even at Diagnostic verbosity the goal is <25µs per emission; the
    // scoring breakdown itself is all primitives so construction remains cheap.
    let sink: Arc<dyn TelemetrySink> = Arc::new(NoopTelemetrySink);

    c.bench_function("telemetry_emit_routing_diagnostic", |b| {
        b.iter(|| {
            let mut decision = sample_routing_decision();
            decision.scores = Vec::with_capacity(5);
            for t in [
                TransportType::Internet,
                TransportType::WiFiDirect,
                TransportType::BLE,
                TransportType::Reticulum,
                TransportType::Nostr,
            ] {
                decision.scores.push((
                    t,
                    offline_protocol_router::TransportScore::from_factors(
                        offline_protocol_router::TransportScoreFactors {
                            signal: 80.0,
                            proximity: 70.0,
                            bandwidth: 60.0,
                            congestion: 90.0,
                            energy: 85.0,
                            reliability: 95.0,
                            load: 75.0,
                            total: 82.0,
                        },
                    ),
                ));
            }
            let record = TelemetryRecord::Routing(Box::new(decision));
            sink.emit(black_box(&record));
        });
    });
}

/// The same records through the telemetry pipe's sink, plus the two
/// protocol-event cases the pipe classifies by name: one it drops (the
/// common case, sixty-odd of the engine's events) and one it forwards. The
/// budgets from the plan: a dropped protocol event under 2 us, a forwarded
/// one under 5 us, and every typed record within 1.5x the no-op sink.
fn bench_emit_pipe_sink(c: &mut Criterion) {
    let config = TelemetryConfig::default()
        .with_api_key("mp_bench")
        .with_app_id("app_bench")
        // Long enough that no flush happens inside a measurement.
        .with_flush_interval(std::time::Duration::from_secs(3_600));
    let (pipe, sink) = start_pipe_for_bench(&config, TelemetryHost::new(TelemetryOs::Linux, 0));
    let mut group = c.benchmark_group("telemetry_emit_pipe");

    group.bench_function("metrics_snapshot", |b| {
        b.iter(|| {
            let record = TelemetryRecord::MetricsSnapshot(Box::new(sample_metrics_frame()));
            sink.emit(black_box(&record));
        });
    });
    group.bench_function("transport_state", |b| {
        b.iter(|| {
            let record = TelemetryRecord::TransportState(sample_transport_state());
            sink.emit(black_box(&record));
        });
    });
    group.bench_function("routing_decision_lifecycle", |b| {
        b.iter(|| {
            let record = TelemetryRecord::Routing(Box::new(sample_routing_decision()));
            sink.emit(black_box(&record));
        });
    });
    group.bench_function("device_snapshot", |b| {
        b.iter(|| {
            let record = TelemetryRecord::Device(sample_device_snapshot());
            sink.emit(black_box(&record));
        });
    });

    let dropped = Event::NetworkMetrics {
        neighbor_count: 3,
        relay_count: 1,
        delivery_ratio: 0.9,
        avg_latency_ms: 40,
    };
    group.bench_function("protocol_dropped", |b| {
        b.iter(|| {
            black_box(sink.try_emit_protocol_event(black_box(&dropped)));
        });
    });
    let forwarded = Event::MessageDelivered {
        message_id: "m".into(),
        latency_ms: 240,
        hop_count: 1,
        transport: "ble".into(),
    };
    group.bench_function("protocol_forwarded", |b| {
        b.iter(|| {
            black_box(sink.try_emit_protocol_event(black_box(&forwarded)));
        });
    });
    group.finish();
    pipe.stop(std::time::Duration::from_secs(1));
}

fn bench_build_metrics_frame(c: &mut Criterion) {
    // Construction cost alone — isolates the cost of building the frame
    // from the emission cost above. This is what the `process()` tick
    // pays on each metrics-cadence interval.
    c.bench_function("telemetry_build_metrics_frame", |b| {
        b.iter(|| {
            let frame = sample_metrics_frame();
            black_box(frame);
        });
    });
}

criterion_group!(
    benches,
    bench_emit_noop_sink,
    bench_emit_noop_sink_diagnostic,
    bench_emit_pipe_sink,
    bench_build_metrics_frame,
);
criterion_main!(benches);
