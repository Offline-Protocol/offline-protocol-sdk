use criterion::{black_box, criterion_group, criterion_main, Criterion, BenchmarkId};
use offline_protocol_core::{AppId, Message, UserId};
use offline_protocol_transport::{Transport, TransportType};

// MockTransport is only available via module path
#[cfg(test)]
use offline_protocol_transport::mock::MockTransport;

fn create_test_message(size: usize) -> Message {
    let content = "x".repeat(size);
    Message::new(
        UserId::new("alice").unwrap(),
        UserId::new("bob").unwrap(),
        AppId::new("bench").unwrap(),
        &content,
    )
}

fn bench_message_creation(c: &mut Criterion) {
    let mut group = c.benchmark_group("message_creation");
    
    for size in [10, 100, 1000, 10000].iter() {
        group.bench_with_input(BenchmarkId::from_parameter(size), size, |b, &size| {
            b.iter(|| {
                black_box(create_test_message(size));
            });
        });
    }
    
    group.finish();
}

fn bench_message_serialization(c: &mut Criterion) {
    let mut group = c.benchmark_group("message_serialization");
    
    for size in [10, 100, 1000, 10000].iter() {
        let message = create_test_message(*size);
        
        group.bench_with_input(BenchmarkId::from_parameter(size), size, |b, _| {
            b.iter(|| {
                black_box(serde_json::to_vec(&message).unwrap());
            });
        });
    }
    
    group.finish();
}

// Note: This benchmark is disabled in non-test builds since it requires MockTransport
// In production, benchmarking would use real transports
#[cfg(test)]
fn bench_transport_send_receive(c: &mut Criterion) {
    let mut group = c.benchmark_group("transport_send_receive");
    
    for size in [10, 100, 1000].iter() {
        group.bench_with_input(BenchmarkId::from_parameter(size), size, |b, &size| {
            b.iter(|| {
                let mut transport = MockTransport::new(TransportType::BLE);
                transport.start().unwrap();
                
                let message = create_test_message(size);
                transport.send(&message).unwrap();
                let received = transport.receive().unwrap();
                
                black_box(received);
            });
        });
    }
    
    group.finish();
}

#[cfg(not(test))]
fn bench_transport_send_receive(_c: &mut Criterion) {
    // Disabled - requires MockTransport which is test-only
}

criterion_group!(
    benches,
    bench_message_creation,
    bench_message_serialization,
    bench_transport_send_receive
);
criterion_main!(benches);

