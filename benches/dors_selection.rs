use criterion::{criterion_group, criterion_main, BenchmarkId, Criterion};
use offline_protocol_core::{AppId, Message, MessagePriority, UserId};
use offline_protocol_router::{DorsConfig, TransportSelector};
use offline_protocol_transport::{TransportMetrics, TransportType};
use std::collections::HashMap;
use std::hint::black_box;

fn create_test_message(priority: MessagePriority) -> Message {
    let mut message = Message::new(
        UserId::new("alice").unwrap(),
        UserId::new("bob").unwrap(),
        AppId::new("bench").unwrap(),
        "Test message",
    );
    message.priority = priority;
    message
}

fn create_transport_metrics() -> HashMap<TransportType, TransportMetrics> {
    let mut metrics = HashMap::new();

    metrics.insert(
        TransportType::BLE,
        TransportMetrics {
            rssi: Some(-60),
            latency_ms: Some(50),
            bandwidth_bps: Some(100_000),
            congestion: 0.2,
            queue_depth: 5,
            success_count: 95,
            failure_count: 5,
            battery_level: Some(78),
            is_charging: false,
            relay_connection_count: 4,
            is_active_relay: true,
            delivery_ratio: Some(0.95),
            drop_rate: Some(0.05),
            average_hop_count: Some(2.1),
            energy_cost: Some(0.15),
        },
    );

    metrics.insert(
        TransportType::Internet,
        TransportMetrics {
            rssi: Some(-40),
            latency_ms: Some(20),
            bandwidth_bps: Some(10_000_000),
            congestion: 0.1,
            queue_depth: 0,
            success_count: 99,
            failure_count: 1,
            battery_level: Some(82),
            is_charging: true,
            relay_connection_count: 1,
            is_active_relay: false,
            delivery_ratio: Some(0.99),
            drop_rate: Some(0.01),
            average_hop_count: Some(1.1),
            energy_cost: Some(0.05),
        },
    );

    metrics.insert(
        TransportType::WiFiDirect,
        TransportMetrics {
            rssi: Some(-50),
            latency_ms: Some(30),
            bandwidth_bps: Some(50_000_000),
            congestion: 0.15,
            queue_depth: 2,
            success_count: 98,
            failure_count: 2,
            battery_level: Some(76),
            is_charging: false,
            relay_connection_count: 5,
            is_active_relay: true,
            delivery_ratio: Some(0.96),
            drop_rate: Some(0.04),
            average_hop_count: Some(1.5),
            energy_cost: Some(0.25),
        },
    );

    metrics
}

fn bench_transport_selection(c: &mut Criterion) {
    let mut group = c.benchmark_group("dors_selection");

    let priorities = [
        MessagePriority::Low,
        MessagePriority::Medium,
        MessagePriority::High,
    ];

    for priority in priorities.iter() {
        group.bench_with_input(
            BenchmarkId::from_parameter(format!("{:?}", priority)),
            priority,
            |b, &priority| {
                let config = DorsConfig::default();
                let mut selector = TransportSelector::with_config(config);
                let message = create_test_message(priority);
                let metrics = create_transport_metrics();

                b.iter(|| {
                    black_box(selector.select_transport(&message, &metrics));
                });
            },
        );
    }

    group.finish();
}

criterion_group!(benches, bench_transport_selection);
criterion_main!(benches);
