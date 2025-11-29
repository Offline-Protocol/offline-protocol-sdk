# Offline Protocol SDK Integration Guide

This guide walks you through integrating the Offline Protocol SDK into your native or React Native applications. It covers installation, transport enablement (BLE, Wi‑Fi Direct, and Internet), Dynamic Offline Relay Switch (DORS) tuning, and practical advice for building offline-first or hybrid (offline + online) experiences.

---

## 1. Getting Started

### 1.1 Prerequisites

- **React Native** ≥ 0.71 or native Android/iOS projects.
- Xcode 15+ (iOS) / Android Studio Giraffe+ (Android).
- Rust toolchain (nightly not required), Node.js 18+, Yarn or npm.
- BLE support enabled in project entitlements and manifest.
- Optional: Wi‑Fi Direct requires Android 10+ with the `android.permission.NEARBY_WIFI_DEVICES` permission.

### 1.2 Installation (React Native)

```bash
yarn add @offline-protocol/mesh-sdk
# or
npm install @offline-protocol/mesh-sdk
```

Run the platform-specific setup:

- **iOS**: `cd ios && pod install`
- **Android**: Ensure `minSdkVersion ≥ 24` and Jetpack Compose / enable Kotlin 1.8+.

The native bindings auto-link on both platforms. If you integrate into an existing native project, follow the instructions in `bindings/react-native/README.md`.

---

## 2. Initialising the SDK

Create a single `OfflineProtocol` instance at app start-up, configure transports, and register event listeners.

```ts
import { OfflineProtocol, MessagePriority } from '@offline-protocol/mesh-sdk';

const protocol = new OfflineProtocol({
  appId: 'com.example.app',
  userId: 'user-42',
  network: { initialTtl: 8 },
  transports: {
    ble: { enabled: true },
    wifiDirect: { enabled: Platform.OS === 'android' },
    internet: { enabled: true, serverAddress: 'wss://mesh.example.com/socket' },
  },
  dors: {
    preferOnline: false,
    switchHysteresis: 18,
    switchCooldownSecs: 30,
    bleToWifiRetryThreshold: 2,
    rssiSwitchThreshold: -85,
    congestionQueueThreshold: 40,
    stabilityWindowSecs: 10,
    poorSignalDurationSecs: 12,
    ttlEscalationThreshold: 3,
    congestionDurationSecs: 12,
    ttlEscalationHoldSecs: 25,
    historyWindowSize: 12,
    queueRecoveryRatio: 0.4,
    lowBatteryThreshold: 20,
    relayMinBatteryLevel: 30,
    relayOptimalConnectionCount: 4,
  },
  relay: {
    allowRelay: true,
    minBatteryForRelay: 35,
    relayThreshold: 3,
    relayPriority: 'auto',
  },
});

protocol.on('message_received', evt => {
  console.log(`New message ${evt.message_id} from ${evt.sender}`);
});

await protocol.start();
```

Call `await protocol.stop()` during teardown (logout/app exit) to release BLE/Wi‑Fi resources.

---

## 3. Choosing Offline vs Hybrid Modes

| Scenario                 | Recommended `dors.preferOnline` | Enabled Transports                      |
|-------------------------|-----------------------------------|-----------------------------------------|
| Fully offline mesh      | `false`                           | BLE + Wi‑Fi Direct                      |
| Hybrid (Fernweh model)  | `true`                            | Internet (primary), BLE, Wi‑Fi Direct   |
| Emergency response mesh | `false` + aggressive hysteresis   | BLE + Wi‑Fi Direct (lower TTL, retries) |

**Offline apps**: set `internet: { enabled: false }`, bump `initialTtl` to 10 for sparse networks, and reduce `relayMinBatteryLevel` so more relays remain active.

**Hybrid apps**: keep Internet enabled, but specify policies:

```ts
transports: {
  internet: { enabled: true, autoReconnect: true, serverAddress: 'wss://...' },
  ble: { enabled: true },
  wifiDirect: { enabled: true, autoAccept: true },
}
```

DORS will attempt Internet first, fall back to BLE, and escalate to Wi‑Fi Direct when BLE congestion/TTL exhaustion triggers.

---

## 4. Enabling Transports

### 4.1 BLE

- **Permissions**: Bluetooth, Bluetooth Advertise/Connect/Scan (Android 12+), Location (Android), `NSBluetoothAlwaysUsageDescription` (iOS).
- BLE automatically negotiates MTU and handles fragmentation under the hood. The SDK enforces a 512-fragment cap per message and evicts stale reassembly buffers to prevent leaks.

### 4.2 Wi‑Fi Direct (Android)

- Require `android.permission.NEARBY_WIFI_DEVICES` (Android 13+), `ACCESS_FINE_LOCATION`, and `CHANGE_WIFI_STATE`.
- Set `wifiDirect: { enabled: true, autoAccept: true, groupOwnerIntent: 10 }`.
- For offline-only experiences, enable Wi‑Fi Direct to create a high-throughput backbone while BLE handles discovery.

### 4.3 Internet

- Provide `serverAddress` (WebSocket URL). The SDK keeps an auto reconnect loop if `autoReconnect` is `true`.
- Use `preferOnline` to prioritise the Internet; DORS will fall back gracefully when the socket is unavailable.

---

## 5. DORS Tuning Cheatsheet

| Parameter                    | Purpose                                             | Default |
|-----------------------------|------------------------------------------------------|---------|
| `switchHysteresis`          | Minimum score delta required to switch transports   | 15      |
| `switchCooldownSecs`        | Minimum time between switches                        | 20s     |
| `bleToWifiRetryThreshold`   | Failed retries before escalating to Wi‑Fi Direct    | 2       |
| `rssiSwitchThreshold`       | Trigger RSSI (dBm) for escalation                    | -85     |
| `lowBatteryThreshold`       | Battery % below which high-power transports are penalised | 20 |
| `relayMinBatteryLevel`      | Battery % required before becoming a heavy relay     | 30      |
| `relayOptimalConnectionCount` | Preferred connection count before relays considered saturated | 4 |

Hints:

- Drop `switchHysteresis` to 8–10 for emergency networks where latency matters more than stability.
- Increase `queueRecoveryRatio` to 0.7 if you want congestion signals to clear quickly after recovery.
- Decrease `bleToWifiRetryThreshold` to `1` when pushing voice/video fragments.

---

## 6. Handling Lifecycle & Store-and-Forward

The SDK manages a persistent outbox, retry queue, and ACK tracking with exponential backoff. Key methods:

- `protocol.pause()` / `protocol.resume()` for background transitions (Android foreground services recommended).
- `protocol.process()` is called automatically by the native bindings every 100ms; if you embed into a pure native project, schedule it yourself (e.g., `Handler` or `DispatchQueue`).
- The retry queue escalates to Wi‑Fi Direct when TTL is nearing exhaustion or BLE congestion persists, and it prunes expired messages gracefully to avoid disk bloat.

---

## 7. Surfacing Metrics

Use `protocol.getTransportMetrics('ble')` (React Native method) to read runtime health:

```ts
const metrics = await protocol.getTransportMetrics('ble');
// { packetsSent, packetsReceived, bytesSent, bytesReceived, errorRate, avgLatencyMs }
```

In the Rust core, additional telemetry (delivery ratios, hop averages, battery-aware scoring) feeds DORS automatically. Expose them in your UI with badges such as “Relay active”, “Transport switched to Wi‑Fi Direct”, etc., by listening to the `transport_switched`, `relay_promoted`, and `network_metrics` events.

---

## 8. Testing Checklist

1. **Permissions**: Ensure all BLE/Wi‑Fi Direct permissions are granted before starting.
2. **Unit Tests**: Run `cargo test -p offline-protocol-router` to validate DORS logic and `cargo test -p offline-protocol` for reliability flows.
3. **Benchmarks**: Execute `cargo bench dors_selection` to confirm scoring performance after tuning.
4. **Device Scenarios**: Simulate low battery, high congestion, and poor RSSI by manipulating the debug menu in the sample React Native app (`examples/react-native-app`).
5. **Network Split-Brain**: Power off the primary relay and ensure relay promotion events fire and messages recover via alternate paths.

---

## 9. Production Tips

- Run `cargo fmt`, `cargo clippy --workspace --all-targets -- -D warnings`, and the Detox/Jest suites before releasing.
- Use the provided `docs/dors-configuration.md` for deeper algorithm explanations and tune according to venue size.
- For privacy: encrypt payloads at the application layer—the SDK propagates encrypted bytes without inspecting them.
- Monitor battery drain on Wi‑Fi Direct devices; DORS deprioritises high-power transports automatically when the local battery dips below `lowBatteryThreshold`.

---

### Need more?

- Check `docs/configuration.md` for every tunable parameter.
- Reference `examples/react-native-app/src/providers/ProtocolProvider.tsx` to see a full-blown Fernweh hybrid configuration with UI indicators.
- File issues or questions in the repository to share field findings—we continuously refine relay heuristics based on community feedback.

# Offline Protocol SDK Integration Guide

This guide outlines the recommended steps for integrating the Offline Protocol SDK into mobile applications. It covers configuration, transport enablement, reliability features, and React Native bindings, with particular focus on the Dynamic Offline Relay Switch (DORS) system.

## 1. Core Configuration

The SDK is configured using `ProtocolConfig`. Below is a representative setup that highlights the most commonly tuned parameters.

```rust
use offline_protocol::{ProtocolConfig, TransportConfig};
use offline_protocol_router::DorsConfig;

let mut config = ProtocolConfig::new("my-app-id", "local-user-id");

config.transport = TransportConfig {
    ble_enabled: true,
    wifi_direct_enabled: true,
    internet_enabled: true,
};

config.dors = DorsConfig {
    prefer_online: true,
    switch_hysteresis: 15.0,
    switch_cooldown_secs: 20,
    ble_to_wifi_retry_threshold: 2,
    rssi_switch_threshold: -85,
    congestion_queue_threshold: 50,
    stability_window_secs: 8,
    poor_signal_duration_secs: 10,
    ttl_escalation_threshold: 2,
    congestion_duration_secs: 10,
    ttl_escalation_hold_secs: 20,
    history_window_size: 10,
    queue_recovery_ratio: 0.5,
    ..Default::default()
};
```

## 2. Transport Enablement Strategy

- **BLE** is always required for discovery and low-bandwidth messaging.
- **Wi-Fi Direct** (Android only) is optional but provides high throughput links. Enable when devices can afford higher power usage.
- **Internet** can be enabled for hybrid deployments (Fernweh-style) where cloud connectivity is available.

Transports can be toggled at runtime through the React Native or UniFFI bindings:

```ts
await protocol.enableTransport('wifiDirect');
await protocol.forceTransport('ble');    // temporarily pin messages to BLE
await protocol.releaseTransportLock();  // return to automatic routing
```

## 3. Offline-Only vs Hybrid Modes

- **Offline-first** (default): `preferOnline = false` and disable `internetEnabled`. Messages never leave the mesh.
- **Hybrid**: `preferOnline = true` while keeping BLE/Wi-Fi Direct enabled. DORS will route through the internet when reachable and automatically fall back to local transports.
- Use the Control Center screen in the example app or `updateDorsConfig({ preferOnline: true })` via bindings to toggle modes dynamically.

## 4. DORS Tuning

Key parameters to adjust for different environments:

| Parameter | When to adjust | Typical range |
|-----------|----------------|----------------|
| `switch_hysteresis` | Prevent flapping in noisy RF environments | 10 – 25 |
| `switch_cooldown_secs` | Lower for fast-moving peers; raise for static deployments | 10 – 45 s |
| `ble_to_wifi_retry_threshold` | Escalate sooner for time-sensitive traffic | 1 – 3 retries |
| `congestion_queue_threshold` | Set based on acceptable queue depth (messages) | 30 – 70 |
| `congestion_duration_secs` | Require sustained congestion before escalating | 5 – 20 s |
| `queue_recovery_ratio` | Define how much queues must drain before de-escalating | 0.3 – 0.6 |
| `history_window_size` | Larger window smooths DORS metrics, smaller reacts faster | 6 – 16 samples |

All parameters include guardrails in the bindings (minimum/maximum values and rounding).

## 5. Reliability & Store-and-Forward

Recent updates add a persistent outbox and smarter retry pipeline:

- Messages requiring ACKs are stored in the outbox and retried with exponential backoff.
- ACK timeouts automatically enqueue retries and raise DORS retry failure signals.
- Fragment reassembly now tracks latency and drops stale assemblies after `FRAGMENT_TIMEOUT_SECS`.
- Applications can inspect outbox health via `getTransportMetrics('ble')` (queue depth, failure counts).

Call `protocol.process()` periodically (bindings schedule this automatically) to advance retries and cleanup tasks.

## 6. React Native Integration Highlights

```ts
const protocol = new OfflineProtocol({
  appId: 'demo-app',
  userId: 'alice',
  preferOnline: false, // offline-only default
});

await protocol.create();
await protocol.start();

// Update DORS at runtime (values are clamped on native side)
await protocol.updateDorsConfig({
  preferOnline: true,
  congestionDurationSecs: 8,
  queueRecoveryRatio: 0.45,
});

// Read live transport metrics
const bleMetrics = await protocol.getTransportMetrics('ble');
console.log('BLE queue depth', bleMetrics.queueDepth);
```

Bindings ensure all runtime updates are sanitized (for example `historyWindowSize` is limited to 1–100 and ratios are confined to 0–1).

## 7. Native Platform Notes

- **Android**: Wi-Fi Direct setup requires `ACCESS_FINE_LOCATION` and optional `NEARBY_WIFI_DEVICES`. The UniFFI layer exposes `updateDorsConfig` and provides clamping via Kotlin extensions.
- **iOS**: BLE is the only mesh transport. DORS still functions for automatic tuning (congestion timers, TTL escalation). Config values are clamped in `OfflineProtocolModule.swift`.

## 8. Metrics & Instrumentation

- Use `getTransportMetrics` to retrieve queue depth, latency, success/failure counts per transport.
- Subscribe to `transport:switched` and `network:metrics` events to visualize DORS decisions.
- Example app’s analytics screen demonstrates how to surface these metrics within UI.

## 9. Suggested Practices

1. Start with conservative DORS defaults: hysteresis 15, cooldown 20, queue threshold 50.
2. For emergency response scenarios, lower hysteresis/cooldown and increase retry thresholds.
3. Persist protocol state (outbox, deduplicator) across app restarts to preserve store-and-forward guarantees.
4. Schedule a background task (or React Native timer) to call `protocol.refreshMetrics()`/`process()` every few seconds.
5. When exposing configuration sliders/toggles to end-users, route updates through the sanitised binding helpers (as done in the example Control Center screen).

For further customization, review:
- `crates/offline-protocol/src/protocol.rs` for reliability hooks.
- `crates/offline-protocol-router/src/dors.rs` for scoring logic.
- `examples/react-native-app` Control Center implementation for live tuning patterns.

With these pieces in place, you can deliver applications that operate seamlessly offline, take advantage of dynamic transport switching, and provide clear operational telemetry to end-users.


