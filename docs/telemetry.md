# Telemetry

Runtime observability for the Offline Protocol SDK. Telemetry is opt-in: the SDK emits nothing until an app installs a sink. Once installed, a single stream carries protocol events, MLS lifecycle, periodic metrics, transport-state transitions, routing decisions, and device-capability changes.

This guide covers how to wire a sink up from React Native, Rust, and the native iOS/Android layers, plus the configuration knobs that control cadence, verbosity, and identifier scrubbing.

## What you get

A telemetry sink receives a stream of `TelemetryRecord`s. Each record is one of:

| Category | When it fires | Payload |
|----------|---------------|---------|
| `metricsFrame` | Periodic (`metricsCadenceMs`) | Per-transport metrics, retry-queue depth, dedup stats, ACK-pending count, neighbor count, current transport |
| `transportState` | A transport changes status | `{previous, current}` for a single `TransportType` |
| `routingDecision` | DORS selects/switches/escalates | Phase, from/to, winning score, reason code, per-transport score breakdowns (diagnostic tier only) |
| `deviceCapability` | Battery / charging / relay-role change | Current values + changed-fields bitmask |
| `mls` | MLS session lifecycle | JSON event string (gated by `mlsVerbosity`) |
| `protocol` | Legacy protocol events | JSON event string |
| `extension` | Forward-compat fallback | `{name, payloadJson}` for variants added after the client's binding was built |

The stream is **push-based by default** (your listener is called synchronously from the Rust side) with an optional **bounded pull queue** (1024 slots, FIFO, drops oldest on overflow) for consumers that prefer polling.

## React Native

Install the sink after `start()` and pass a listener. The listener is registered before the native install resolves, so no records can slip past.

```typescript
import {
  OfflineProtocol,
  type TelemetryRecord,
  type TelemetryConfig,
} from 'offline-protocol-react-native';

const proto = new OfflineProtocol(config);
await proto.start();

const handleTelemetry = (rec: TelemetryRecord) => {
  switch (rec.category) {
    case 'metricsFrame':
      // rec.frame.retryQueue.totalCount, rec.frame.transports, ...
      break;
    case 'transportState':
      // rec.event.previous, rec.event.current
      break;
    case 'routingDecision':
      // rec.decision.phase, rec.decision.reasonCode, rec.decision.scores
      break;
    case 'deviceCapability':
      // rec.snapshot.batteryLevel, rec.snapshot.relayRole
      break;
    case 'mls':
      // JSON.parse(rec.eventJson)
      break;
    case 'protocol':
      // JSON.parse(rec.eventJson)
      break;
    case 'extension':
      // forward-compat — newer SDK emitted a variant this client doesn't type yet
      break;
  }
};

const telemetryConfig: TelemetryConfig = {
  metricsCadenceMs: 5000,
  mlsVerbosity: 'lifecycle',
  routingDiagnostic: false,
  scrubIds: true,
  enablePollQueue: false, // push-only; skip the per-emit JSON envelope cost
};

const unsubscribe = await proto.installTelemetrySink(
  telemetryConfig,
  handleTelemetry,
);

// later, during teardown:
unsubscribe();                    // drop the JS listener
await proto.uninstallTelemetrySink(); // detach the native sink + drain pull queue
```

### Pull mode

Leave `enablePollQueue` at its default (`true` / omitted) and drain the buffer on a timer instead of using a push listener:

```typescript
await proto.installTelemetrySink({ metricsCadenceMs: 5000 });

setInterval(async () => {
  let rec;
  while ((rec = await proto.pollTelemetry()) !== null) {
    process(rec);
  }
}, 1000);
```

Mix-and-match works too: a push listener plus `pollTelemetry()` will see the same records.

## Rust (core)

Implement `TelemetrySink` and hand it to the protocol:

```rust
use std::sync::Arc;
use offline_protocol::telemetry::{
    MlsVerbosity, TelemetryConfig, TelemetryRecord, TelemetrySink,
};
use offline_protocol::OfflineProtocol;

struct StdoutSink;

impl TelemetrySink for StdoutSink {
    fn emit(&self, record: &TelemetryRecord) {
        match record {
            TelemetryRecord::MetricsSnapshot(frame) => {
                println!("retry_queue={}", frame.retry_queue.total_count);
            }
            TelemetryRecord::Routing(decision) => {
                println!("routing {:?} -> {:?}", decision.from, decision.to);
            }
            _ => {}
        }
    }
}

let mut proto = OfflineProtocol::new(config)?;
let sink: Arc<dyn TelemetrySink> = Arc::new(StdoutSink);
let cfg = TelemetryConfig::default()
    .with_metrics_cadence(Some(std::time::Duration::from_secs(5)))
    .with_mls_verbosity(MlsVerbosity::Lifecycle)
    .with_routing_diagnostic(false);

proto.install_telemetry_sink(sink, cfg)?;
proto.start()?;
```

`emit()` runs on SDK hot paths. It must not block, panic, or re-enter the SDK. Do any heavy work on a channel and handle it elsewhere.

You can install the sink **before or after** `start()`. The wiring persists across `stop() → start()` cycles, so you don't need to re-install.

## Native (Swift / Kotlin)

The React Native bindings already install a `TelemetrySink` implementation that forwards to JS — most apps don't need to touch this layer. If you're integrating UniFFI directly from Swift or Kotlin, implement the UniFFI-exported `TelemetrySink` trait and pass it to `installTelemetrySink`. The callbacks are:

```
on_protocol_event(event_json: String)
on_mls_event(event_json: String)
on_metrics_frame(frame: MetricsFrame)
on_transport_state(event: TransportStateEvent)
on_routing_decision(decision: RoutingDecision)
on_device_capability(snapshot: DeviceCapabilitySnapshot)
on_extension(name: String, payload_json: String)
```

Implementations must be thread-safe and non-blocking. See `bindings/react-native/ios/OfflineProtocolModule.swift` (`TelemetrySinkImpl`) and `android/src/main/java/com/offlineprotocol/OfflineProtocolModule.kt` (`TelemetrySinkImpl`) for reference implementations that forward to the RN event emitter.

## TelemetryConfig reference

| Field | Type | Default | Effect |
|-------|------|---------|--------|
| `metricsCadenceMs` | number | `5000` | Period for `metricsFrame` emission. Pass `null` in Rust (`None`) to disable periodic metrics entirely. |
| `mlsVerbosity` | `'off'` \| `'lifecycle'` \| `'diagnostic'` | `'lifecycle'` | Gates MLS record emission. See below. |
| `routingDiagnostic` | boolean | `false` | When `true`, `RoutingDecision.scores` carries per-factor breakdowns (signal, proximity, bandwidth, congestion, energy, reliability, load). Off by default to avoid per-emit allocation on the DORS hot path. |
| `scrubIds` | boolean | `true` | Hash long-lived identifiers (`peer_id`, `user_id`, `group_id`, actor fields) with SHA-256 before emission. |
| `enablePollQueue` | boolean | `true` | When `false`, the Rust adapter skips the per-emit JSON envelope used by `pollTelemetry()`. Push listeners still fire. Leave `true` if any consumer calls `pollTelemetry()`. |
| `mlsSamplingBypass` | boolean | `false` | When `true`, a telemetry-grade sink opts out of MLS event sampling so every MLS lifecycle record is emitted (not rate-limited). Leave `false` for normal dashboards. |

Rust also exposes `with_scrub_secret([u8; 16])` for a deterministic hashing key — without it, the SDK generates a random per-instance fallback so scrubbed IDs are stable for the lifetime of one protocol instance but not across restarts.

### MLS verbosity

| Level | What's emitted |
|-------|----------------|
| `off` | No MLS records at all. |
| `lifecycle` *(default)* | Session init, session ready, decrypt failures, session-missing events. Enough to trace handshake and key-rotation health. |
| `diagnostic` | Lifecycle plus per-operation diagnostics. Use when investigating MLS-specific issues; noisier. |

Before this runtime knob existed, MLS telemetry was gated by the `mls-observability` Cargo feature — that feature is retired. Set `mlsVerbosity` at install time instead.

## MetricsFrame quick reference

`MetricsFrame` is emitted on `metricsCadenceMs` and is the main surface most dashboards consume.

| Field | Meaning |
|-------|---------|
| `timestampMs` | Emission timestamp (ms since epoch). |
| `currentTransport` | Transport DORS has selected right now (may be absent if nothing is viable). |
| `transports[]` | Per-transport `TransportMetrics` — counters plus RSSI, congestion, queue depth, battery, relay state, delivery ratio, etc. |
| `retryQueue.{total,ready,critical,high,medium,low}Count` | Current heap depth, broken down by priority. `total` is instantaneous, not cumulative. |
| `dedup` | Dedup window size, capacity used, mode. |
| `ackPending` | Messages sent but awaiting ACK. |
| `neighborCount` | Known neighbors in the mesh. |
| `isLocalRelay` | Whether this device is currently acting as a relay. |

See [Message Delivery](message-delivery.md) for retry-queue semantics and [DORS Deep Dive](dors.md) for what the per-transport scores mean.

## Identifier scrubbing

With `scrubIds: true` (the default), identifiers that persist across sessions are hashed before they reach your sink:

- **Scrubbed**: `peer_id`, `user_id`, `group_id`, `sender`, `recipient`, `added_by`, `removed_by`, and similar actor fields.
- **Not scrubbed**: `message_id`, `file_id` (single-use UUIDs), message `content`, enum values (`transport`, `status`, etc.).

The hash is `SHA-256(secret || raw)` truncated to 16 bytes / 32 hex chars. The same raw ID always maps to the same hash within one protocol instance, so you can still correlate events for a given peer without ever seeing the raw identifier. Session-derived tokens are always hashed regardless of this flag.

Turn scrubbing off only when debugging on a trusted device with a trusted sink.

## Tips

- **Install before `start()` when possible.** You'll see routing records from the very first send. Install-after-start is supported; the next tick re-arms diff snapshots so you only see real transitions, not synthetic ones.
- **Replace vs. reinstall.** Calling `installTelemetrySink` again atomically replaces the sink but does not drain the pull queue — a consumer polling immediately after replace will see the previous sink's buffered records first. Call `uninstallTelemetrySink()` first if you need a clean slate.
- **Don't block in `emit`.** Forward heavy work (file I/O, network upload) to a channel or background queue. The sink runs on SDK hot paths.
- **Pick a cadence that matches your UI.** A 2s cadence is fine for a live diagnostics screen; 30s is plenty for background metric shipping. Lower cadence = less JSON serialization on the device.
- **`routingDiagnostic: true` is for debugging.** The per-factor score breakdowns are useful when tuning DORS or investigating transport flapping, but carry per-emit allocation cost. Keep it off in production unless you're actively investigating.

## Worked example

The demo app (`examples/demo-app/src/context/ProtocolContext.tsx`) installs a sink on start with push delivery, a 2-second cadence, and routing diagnostics enabled so its Diagnostics screen can render per-transport score bars. The telemetry handler lives alongside the protocol-event handler and fans records into UI state:

```typescript
const handleTelemetry = useCallback((rec: TelemetryRecord) => {
  switch (rec.category) {
    case 'metricsFrame':
      setLatestMetrics(rec.frame);
      break;
    case 'routingDecision':
      appendRoutingHistory(rec.decision);
      break;
    // ...
  }
}, []);

await proto.installTelemetrySink(
  {
    routingDiagnostic: true,
    metricsCadenceMs: 2000,
    mlsVerbosity: 'lifecycle',
    enablePollQueue: false,
  },
  handleTelemetry,
);
```

Browse `examples/demo-app/src/screens/DiagnosticsScreen.tsx` to see how each record category maps to UI.
