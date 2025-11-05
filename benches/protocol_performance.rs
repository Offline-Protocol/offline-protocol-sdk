use criterion::{black_box, criterion_group, criterion_main, Criterion};
use offline_protocol::{OfflineProtocol, ProtocolConfig};
use offline_protocol_transport::{MockTransport, Transport, TransportType};

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
            let mut mock_transport = MockTransport::new(TransportType::BLE);
            mock_transport.start().unwrap();
            protocol.transport_manager_mut().add_transport(
                TransportType::BLE,
                Box::new(mock_transport)
            );
            
            protocol.start().unwrap();
            protocol.stop().unwrap();
            black_box(protocol);
        });
    });
}

fn bench_send_message(c: &mut Criterion) {
    c.bench_function("send_message", |b| {
        let config = ProtocolConfig::new("bench", "user123");
        let mut protocol = OfflineProtocol::new(config).unwrap();
        
        // Add mock transport
        let mut mock_transport = MockTransport::new(TransportType::BLE);
        mock_transport.start().unwrap();
        protocol.transport_manager_mut().add_transport(
            TransportType::BLE,
            Box::new(mock_transport)
        );
        
        protocol.start().unwrap();
        
        b.iter(|| {
            black_box(protocol.send_message("bob", "Hello", None).unwrap());
        });
    });
}

fn bench_process_loop(c: &mut Criterion) {
    c.bench_function("process_loop", |b| {
        let config = ProtocolConfig::new("bench", "user123");
        let mut protocol = OfflineProtocol::new(config).unwrap();
        
        // Add mock transport
        let mut mock_transport = MockTransport::new(TransportType::BLE);
        mock_transport.start().unwrap();
        protocol.transport_manager_mut().add_transport(
            TransportType::BLE,
            Box::new(mock_transport)
        );
        
        protocol.start().unwrap();
        
        b.iter(|| {
            black_box(protocol.process().unwrap());
        });
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

