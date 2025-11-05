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
        rssi: Some(-60),
        latency_ms: Some(50),
        bandwidth_bps: Some(100_000),
        congestion: 0.2,
        queue_depth: 5,
        success_count: 95,
        failure_count: 5,
    });
    
    metrics.insert(TransportType::Internet, TransportMetrics {
        rssi: Some(-40),
        latency_ms: Some(20),
        bandwidth_bps: Some(10_000_000),
        congestion: 0.1,
        queue_depth: 0,
        success_count: 99,
        failure_count: 1,
    });
    
    metrics.insert(TransportType::WiFiDirect, TransportMetrics {
        rssi: Some(-50),
        latency_ms: Some(30),
        bandwidth_bps: Some(50_000_000),
        congestion: 0.15,
        queue_depth: 2,
        success_count: 98,
        failure_count: 2,
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

criterion_group!(benches, bench_transport_selection);
criterion_main!(benches);

