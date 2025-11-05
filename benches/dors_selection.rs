use criterion::{black_box, criterion_group, criterion_main, Criterion, BenchmarkId};
use offline_protocol_core::{AppId, Message, MessagePriority, UserId};
use offline_protocol_router::{DorsConfig, TransportSelector};
use offline_protocol_transport::{TransportMetrics, TransportType};
use std::collections::HashMap;

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
    
    metrics.insert(TransportType::BLE, TransportMetrics {
        latency_ms: 50.0,
        throughput_bps: 100_000.0,
        error_rate: 0.01,
        queue_depth: 5,
        battery_impact: 0.3,
        signal_strength: 0.8,
    });
    
    metrics.insert(TransportType::Internet, TransportMetrics {
        latency_ms: 20.0,
        throughput_bps: 10_000_000.0,
        error_rate: 0.001,
        queue_depth: 0,
        battery_impact: 0.5,
        signal_strength: 1.0,
    });
    
    metrics.insert(TransportType::WiFiDirect, TransportMetrics {
        latency_ms: 30.0,
        throughput_bps: 50_000_000.0,
        error_rate: 0.005,
        queue_depth: 2,
        battery_impact: 0.6,
        signal_strength: 0.9,
    });
    
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
            }
        );
    }
    
    group.finish();
}

fn bench_transport_scoring(c: &mut Criterion) {
    c.bench_function("transport_scoring", |b| {
        let config = DorsConfig::default();
        let selector = TransportSelector::with_config(config);
        let message = create_test_message(MessagePriority::High);
        let metrics = create_transport_metrics();
        
        b.iter(|| {
            for (transport_type, transport_metrics) in &metrics {
                black_box(selector.calculate_transport_score(
                    *transport_type,
                    &message,
                    transport_metrics
                ));
            }
        });
    });
}

criterion_group!(benches, bench_transport_selection, bench_transport_scoring);
criterion_main!(benches);

