use criterion::{criterion_group, criterion_main, Criterion};
use offline_protocol::{OfflineProtocol, ProtocolConfig};
use offline_protocol_transport::{mock::MockTransport, Transport, TransportType};
use std::hint::black_box;

/// Builds an engine with a live `MockTransport` attached, ready to start.
///
/// Encryption fail-closes by default (SEC-M3) and a benchmark never
/// initializes MLS, so under a stock config every send returns
/// `EncryptFailed` before a message is even constructed. Opting out here is
/// what keeps `send_message` measuring the dispatch path down to the
/// transport rather than the rejection path in front of it.
fn protocol_with_mock_transport() -> OfflineProtocol {
    let mut config = ProtocolConfig::new("bench", "user123");
    config.encryption.require_encryption = false;

    let mut protocol = OfflineProtocol::new(config).unwrap();

    let mock_transport = MockTransport::new(TransportType::BLE);
    mock_transport.start().unwrap();
    protocol
        .transport_manager_mut()
        .add_transport(TransportType::BLE, Box::new(mock_transport));

    protocol
}

fn bench_protocol_creation(c: &mut Criterion) {
    c.bench_function("protocol_creation", |b| {
        b.iter(|| {
            let config = ProtocolConfig::new("bench", "user123");
            black_box(OfflineProtocol::new(config).unwrap());
        });
    });
}

fn bench_protocol_start_stop(c: &mut Criterion) {
    c.bench_function("protocol_start_stop", |b| {
        b.iter(|| {
            let mut protocol = protocol_with_mock_transport();

            protocol.start().unwrap();
            protocol.stop().unwrap();
            black_box(protocol);
        });
    });
}

fn bench_send_message(c: &mut Criterion) {
    c.bench_function("send_message", |b| {
        b.iter_batched(
            || {
                let mut protocol = protocol_with_mock_transport();
                protocol.start().unwrap();
                protocol
            },
            |mut protocol| {
                // `expect`, not `ok`: a send that stops short of the
                // transport must fail the benchmark rather than quietly
                // report the timing of an error return.
                black_box(
                    protocol
                        .send_message("bob", "Hello", None, None::<String>)
                        .expect("bench send must reach the transport"),
                );
            },
            criterion::BatchSize::LargeInput,
        );
    });
}

fn bench_process_loop(c: &mut Criterion) {
    c.bench_function("process_loop", |b| {
        b.iter_batched(
            || {
                let mut protocol = protocol_with_mock_transport();
                protocol.start().unwrap();
                protocol
            },
            |mut protocol| {
                black_box(protocol.process().expect("bench tick must succeed"));
            },
            criterion::BatchSize::LargeInput,
        );
    });
}

criterion_group!(
    benches,
    bench_protocol_creation,
    bench_protocol_start_stop,
    bench_send_message,
    bench_process_loop
);
criterion_main!(benches);
