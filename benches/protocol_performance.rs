use criterion::{criterion_group, criterion_main, Criterion};
use offline_protocol::{OfflineProtocol, ProtocolConfig};
use offline_protocol_transport::{mock::MockTransport, Transport, TransportType};
use std::hint::black_box;

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
            let config = ProtocolConfig::new("bench", "user123");
            let mut protocol = OfflineProtocol::new(config).unwrap();

            // Add mock transport
            let mock_transport = MockTransport::new(TransportType::BLE);
            mock_transport.start().unwrap();
            protocol
                .transport_manager_mut()
                .add_transport(TransportType::BLE, Box::new(mock_transport));

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
                let config = ProtocolConfig::new("bench", "user123");
                let mut protocol = OfflineProtocol::new(config).unwrap();

                // Add mock transport
                let mock_transport = MockTransport::new(TransportType::BLE);
                mock_transport.start().unwrap();
                protocol
                    .transport_manager_mut()
                    .add_transport(TransportType::BLE, Box::new(mock_transport));

                protocol.start().unwrap();
                protocol
            },
            |mut protocol| {
                black_box(
                    protocol
                        .send_message("bob", "Hello", None, None::<String>)
                        .ok(),
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
                let config = ProtocolConfig::new("bench", "user123");
                let mut protocol = OfflineProtocol::new(config).unwrap();

                // Add mock transport
                let mock_transport = MockTransport::new(TransportType::BLE);
                mock_transport.start().unwrap();
                protocol
                    .transport_manager_mut()
                    .add_transport(TransportType::BLE, Box::new(mock_transport));

                protocol.start().unwrap();
                protocol
            },
            |mut protocol| {
                black_box(protocol.process().ok());
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
