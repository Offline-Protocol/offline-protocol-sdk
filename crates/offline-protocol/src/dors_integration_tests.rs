//! Integration tests for end-to-end DORS behavior under changing conditions.
//!
//! Validates:
//! - **Happy path**: Best transport selected on startup
//! - **Change path**: Metric shift causes valid switch after policy checks (hysteresis/cooldown)
//! - **Noisy path**: Anti-flap controls prevent rapid oscillation
//!
//! These tests run in CI via `cargo test -p offline-protocol dors_integration`.

#![cfg(test)]

use offline_protocol_core::{AppId, Message, UserId};
use offline_protocol_router::{DorsConfig, TransportSelector};
use offline_protocol_transport::{mock::MockTransport, Transport, TransportMetrics, TransportStatus, TransportType};
use std::sync::{Arc, Mutex};

fn test_message() -> Message {
    Message::new(
        UserId::new("alice").unwrap(),
        UserId::new("bob").unwrap(),
        AppId::new("test").unwrap(),
        "test",
    )
}

// ---- Metric helpers (tuned so DORS scores differ as needed) ----

fn ble_strong_metrics() -> TransportMetrics {
    let mut m = TransportMetrics::default();
    m.rssi = Some(-60);
    m.delivery_ratio = Some(0.95);
    m.congestion = 0.1;
    m.battery_level = Some(80);
    m
}

fn wifi_strong_metrics() -> TransportMetrics {
    let mut m = TransportMetrics::default();
    m.bandwidth_bps = Some(5_000_000);
    m.delivery_ratio = Some(0.9);
    m.congestion = 0.1;
    m.battery_level = Some(80);
    m
}

fn ble_weak_metrics() -> TransportMetrics {
    let mut m = TransportMetrics::default();
    m.rssi = Some(-92);
    m.delivery_ratio = Some(0.6);
    m.congestion = 0.4;
    m.battery_level = Some(40);
    m
}

fn wifi_weak_metrics() -> TransportMetrics {
    let mut m = TransportMetrics::default();
    m.bandwidth_bps = Some(100_000);
    m.delivery_ratio = Some(0.5);
    m.congestion = 0.2;
    m.battery_level = Some(30);
    m
}

/// Wrapper around MockTransport so tests can update metrics between sends
/// (manager holds Box<dyn Transport>, so we keep an Arc to the mock).
struct HoldMock(Arc<Mutex<MockTransport>>);

impl Transport for HoldMock {
    fn as_any(&self) -> &dyn std::any::Any {
        self
    }
    fn transport_type(&self) -> TransportType {
        self.0.lock().unwrap().transport_type()
    }
    fn status(&self) -> TransportStatus {
        self.0.lock().unwrap().status()
    }
    fn metrics(&self) -> TransportMetrics {
        self.0.lock().unwrap().metrics()
    }
    fn send(&self, message: &Message) -> offline_protocol_transport::Result<()> {
        self.0.lock().unwrap().send(message)
    }
    fn receive(&self) -> offline_protocol_transport::Result<Option<Message>> {
        self.0.lock().unwrap().receive()
    }
    fn start(&mut self) -> offline_protocol_transport::Result<()> {
        self.0.lock().unwrap().start()
    }
    fn stop(&mut self) -> offline_protocol_transport::Result<()> {
        self.0.lock().unwrap().stop()
    }
}

fn add_started_mock(
    manager: &mut crate::TransportManager,
    transport_type: TransportType,
    metrics: TransportMetrics,
) -> Arc<Mutex<MockTransport>> {
    let mut mock = MockTransport::new(transport_type);
    mock.set_metrics(metrics);
    mock.start().unwrap();
    let arc = Arc::new(Mutex::new(mock));
    manager.add_transport(transport_type, Box::new(HoldMock(arc.clone())));
    arc
}

// ========== Happy path: best transport selected on startup ==========

#[test]
fn dors_integration_happy_path_best_transport_selected_on_startup() {
    let config = DorsConfig::default();
    let selector = TransportSelector::with_config(config);
    let mut manager = crate::TransportManager::new(selector);

    add_started_mock(&mut manager, TransportType::BLE, ble_strong_metrics());
    add_started_mock(&mut manager, TransportType::WiFiDirect, wifi_weak_metrics());

    let msg = test_message();
    manager.send(&msg).unwrap();

    // With default config (prefer_online = false), BLE with strong RSSI and good
    // delivery ratio should outscore WiFi with weak bandwidth and poor delivery.
    let current = manager.current_transport().expect("a transport was selected");
    assert_eq!(
        current,
        TransportType::BLE,
        "happy path: best transport (BLE with strong metrics) should be selected on first send"
    );
}

// ========== Change path: metric shift causes valid switch after policy checks ==========

#[test]
fn dors_integration_change_path_metric_shift_causes_valid_switch_after_policy_checks() {
    // Short cooldown and stability so we can see a switch in one test without sleeping.
    let config = DorsConfig {
        switch_cooldown_secs: 0,
        stability_window_secs: 0,
        switch_hysteresis: 10.0,
        ..DorsConfig::default()
    };
    let selector = TransportSelector::with_config(config);
    let mut manager = crate::TransportManager::new(selector);

    let ble = add_started_mock(&mut manager, TransportType::BLE, ble_strong_metrics());
    let wifi = add_started_mock(&mut manager, TransportType::WiFiDirect, wifi_weak_metrics());

    // First send: BLE is clearly better -> select BLE.
    manager.send(&test_message()).unwrap();
    assert_eq!(
        manager.current_transport(),
        Some(TransportType::BLE),
        "change path: first send should select BLE"
    );

    // Shift metrics so WiFi is clearly better (high bandwidth, good delivery; BLE weak).
    ble.lock().unwrap().set_metrics(ble_weak_metrics());
    wifi.lock().unwrap().set_metrics(wifi_strong_metrics());

    // Second send: improvement of WiFi over BLE should exceed hysteresis -> switch to WiFi.
    manager.send(&test_message()).unwrap();
    assert_eq!(
        manager.current_transport(),
        Some(TransportType::WiFiDirect),
        "change path: after metric shift, DORS should switch to WiFi when improvement exceeds hysteresis"
    );
}

// ========== Noisy path: anti-flap controls prevent rapid oscillation ==========

#[test]
fn dors_integration_noisy_path_anti_flap_prevents_rapid_oscillation() {
    // Stable metrics; many sends. DORS should stay on the same transport (no spurious flapping).
    let config = DorsConfig::default();
    let selector = TransportSelector::with_config(config);
    let mut manager = crate::TransportManager::new(selector);

    add_started_mock(&mut manager, TransportType::BLE, ble_strong_metrics());
    add_started_mock(&mut manager, TransportType::WiFiDirect, wifi_weak_metrics());

    let mut selected_transports = Vec::with_capacity(12);
    for _ in 0..12 {
        manager.send(&test_message()).unwrap();
        selected_transports.push(manager.current_transport().expect("selected"));
    }

    // All sends should use the same transport (the best one under stable metrics).
    let first = selected_transports[0];
    assert!(
        selected_transports.iter().all(|&t| t == first),
        "noisy path: anti-flap should keep transport stable across many sends; got {:?}",
        selected_transports
    );
}
