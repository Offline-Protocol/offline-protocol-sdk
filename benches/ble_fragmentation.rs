use criterion::{black_box, criterion_group, criterion_main, BenchmarkId, Criterion};
use offline_protocol_core::{AppId, Message, UserId};
use offline_protocol_transport::BleTransport;

fn create_large_message(size: usize) -> Message {
    let content = "x".repeat(size);
    Message::new(
        UserId::new("alice").unwrap(),
        UserId::new("bob").unwrap(),
        AppId::new("bench").unwrap(),
        &content,
    )
}

fn bench_message_fragmentation(c: &mut Criterion) {
    let mut group = c.benchmark_group("ble_fragmentation");

    // Test different message sizes
    for size in [100, 1000, 5000, 10000].iter() {
        group.bench_with_input(BenchmarkId::from_parameter(size), size, |b, &size| {
            let transport = BleTransport::new("test-device");
            let message = create_large_message(size);

            b.iter(|| {
                black_box(transport.fragment_message(&message).unwrap());
            });
        });
    }

    group.finish();
}

fn bench_fragment_reassembly(c: &mut Criterion) {
    let mut group = c.benchmark_group("ble_reassembly");

    for size in [100, 1000, 5000, 10000].iter() {
        group.bench_with_input(BenchmarkId::from_parameter(size), size, |b, &size| {
            let transport = BleTransport::new("test-device");
            let message = create_large_message(size);

            // Pre-fragment the message
            let fragments = transport.fragment_message(&message).unwrap();

            b.iter(|| {
                for fragment in &fragments {
                    black_box(transport.process_fragment(fragment).unwrap());
                }
            });
        });
    }

    group.finish();
}

fn bench_serialization(c: &mut Criterion) {
    c.bench_function("ble_serialize_1kb", |b| {
        let transport = BleTransport::new("test-device");
        let message = create_large_message(1000);

        b.iter(|| {
            black_box(transport.serialize_message(&message).unwrap());
        });
    });
}

criterion_group!(
    benches,
    bench_message_fragmentation,
    bench_fragment_reassembly,
    bench_serialization
);
criterion_main!(benches);
