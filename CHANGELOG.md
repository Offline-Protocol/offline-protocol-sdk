# Changelog

All notable changes to the Offline Protocol SDK are documented in this file. This changelog covers everything after the **v0.7.1** release.

## [Unreleased]

### Breaking Changes

- **`getTransportMetrics` returns real data (or `null`) instead of a zeroed mock** — The UniFFI method `get_transport_metrics(transportType)` (exposed as `getTransportMetrics` in Swift/Kotlin/TypeScript) previously always returned a `TransportMetrics` populated with zeros. It now pulls directly from `Transport::metrics()` and returns `null` when the requested transport is not registered with the `TransportManager`. Callers that relied on the non-null guarantee or zero-valued fields must add a null-check and treat absent transports as "metrics unavailable" rather than "all counters zero".
- **`TransportType` gained `Reticulum` and `Nostr` variants** — `TransportType` is now a five-variant enum (`BLE`, `WiFiDirect`, `Internet`, `Reticulum`, `Nostr`) across Rust, UDL, Swift, Kotlin, and TypeScript. Exhaustive `switch`/`when` statements over `TransportType` must add the two new cases. `ProtocolConfig` gains matching `reticulum_enabled` and `nostr_enabled` fields (default `false`).
- **`mls-observability` Cargo feature retired** ([#92](https://github.com/Offline-Protocol/offline-protocol-sdk/pull/92)) — Compile-time gating is replaced by runtime `TelemetryConfig::mls_verbosity` (`Off` | `Lifecycle` (default) | `Diagnostic`). Workspace Cargo files that pass `--features mls-observability` should drop the flag; behaviour is preserved by the `Lifecycle` default. Opting out at runtime additionally suppresses the legacy `MlsEventEmitter` path. See `docs/telemetry.md`.
- **`BleTransport::set_mtu` / `mtu` global accessors removed** ([#86](https://github.com/Offline-Protocol/offline-protocol-sdk/pull/86)) — Replaced by per-peer `bleSetPeerMtu(deviceId, maxPayload)` / `bleClearPeerMtu(deviceId)` UniFFI methods. Callers must pass the *header-adjusted* maximum usable payload (Android subtracts the 3-byte ATT overhead from `onMtuChanged`; iOS reads `maximumWriteValueLength(for: .withoutResponse)` directly). The platform managers shipped in this release already do this — only direct UniFFI integrations need updating. There is also a strict ordering invariant: call `bleSetPeerMtu` *before* `blePeerDiscovered` for each peer, otherwise the very first fragment falls back to the 185-byte floor.
- **`fragment_message` requires a recipient** — `BleTransport::fragment_message(message)` is now `fragment_message(recipient, message)` so per-peer MTU lookup is keyed correctly. Affects the `offline-protocol-bench` crate and any direct callers; the production protocol path is unaffected.

### Added

#### Telemetry epic ([#89](https://github.com/Offline-Protocol/offline-protocol-sdk/pull/89)–[#96](https://github.com/Offline-Protocol/offline-protocol-sdk/pull/96))

- **`TelemetrySink` and `TelemetryRecord` taxonomy** ([#91](https://github.com/Offline-Protocol/offline-protocol-sdk/pull/91)) — A single observer trait replaces the previous patchwork of `EventCallback`, `MlsEventEmitter`, and pull-only `TransportMetrics`. `TelemetryRecord` is a `#[non_exhaustive]` enum spanning six categories (`Protocol`, `Mls`, `MetricsSnapshot`, `TransportState`, `Routing`, `Device`); the `Event` payload is boxed so non-`Protocol` records do not pay a 368-byte size tax. `TelemetryConfig` ships privacy-preserving defaults: `scrub_ids=true` (long-lived pseudonymous identifiers are SHA-256 hashed via a per-instance secret before crossing the sink), `mls_verbosity=Lifecycle`, and `metrics_cadence_ms=Some(5000)` (aligned with the DORS stability window). `Scrubber` is `pub(crate)`; `TelemetryConfig`'s `Debug` impl redacts the secret.
- **Sink wiring for protocol events and MLS lifecycle** ([#92](https://github.com/Offline-Protocol/offline-protocol-sdk/pull/92)) — `OfflineProtocol::install_telemetry_sink(sink, config) -> Result<()>` plumbs every `Event` and MLS lifecycle event through the sink. Long-lived identifiers (peer/user/group/sender/recipient/members) are scrubbed by default; message IDs and content stay raw. A compile-time exhaustiveness ward in `scrub_event.rs` guarantees new `Event` variants cannot ship without explicit scrubbing decisions. The legacy `EventCallback` continues to fire alongside an installed sink.
- **`MetricsFrame`, `TransportStateEvent`, `RoutingDecision`, `DeviceCapabilitySnapshot` records** ([#93](https://github.com/Offline-Protocol/offline-protocol-sdk/pull/93)) — These four records carry no long-lived identifiers and are emitted from `OfflineProtocol::process()` (metrics cadence, transport-status diff, device-capability diff) and from a new `routing_decision_callback` wired alongside every `Event::Dors*` site. Bench: 2–29 ns per emission against a <5 µs / <25 µs budget. Pure additive — `Event::Dors*` consumers see the same legacy stream.
- **Unified `TelemetrySink` across UniFFI / iOS / Android / RN / Python** ([#94](https://github.com/Offline-Protocol/offline-protocol-sdk/pull/94), [#97](https://github.com/Offline-Protocol/offline-protocol-sdk/pull/97)) — Apps install a single sink that receives every typed `TelemetryRecord` the SDK emits, plus a forward-compatible `on_extension(name, payloadJson)` fallback for variants added to the Rust enum after the FFI was generated. A bounded (1024-slot, drop-oldest) poll queue backs `pollTelemetry()` for apps that prefer pull over push. `TransportMetrics` gained 12 optional fields mirroring the richer Rust struct and flows unchanged through both the pull and push paths.
- **`TelemetryConfig.enablePollQueue` (default `true`)** — Push-only integrations can set this to `false` to skip the per-emit `serde_json` envelope construction inside the Rust adapter. With the opt-out in effect, the typed push callbacks still fire but `pollTelemetry()` returns `null` for records emitted under that config. Flag is local to the UniFFI adapter (not forwarded to `CoreTelemetryConfig`).
- **`uninstallTelemetrySink()`** — Detaches the installed sink in a single call: replaces the core-side sink with a no-op (future emissions are discarded with zero overhead) and drains the pull queue so a subsequent `installTelemetrySink(...)` starts with an empty queue. Idempotent.
- **`installTelemetrySink(config, listener?)` accepts an optional listener** — Registering the listener synchronously before the native install is dispatched closes the window where records emitted between the bridge resolving and the next JS microtask would fan out to an empty listener set and be dropped on the push channel.
- **`RoutingPhase.Unknown` / `RoutingReasonCode.Unknown`** — When the Rust core reports a routing variant the FFI build does not recognise (new-core / old-FFI skew), the adapter now maps to an explicit `Unknown` value, gated by a `std::sync::Once` warn so the routing hot path cannot drown the tracing layer.
- **`AckEvicted` and `FragmentAssemblyEvicted` events** ([#89](https://github.com/Offline-Protocol/offline-protocol-sdk/pull/89)) — Three `Event` variants were defined and reached the FFI but never fired. ACK eviction now returns `Option<AckEvictionInfo>` directly from `register_pending_ack` (no drain buffer); BLE fragment eviction uses an injected callback that emits outside the `fragment_buffers` lock to avoid deadlocks. Apps gain observability into capacity pressure on the ack tracker and the fragment reassembly cache.
- **Telemetry-driven diagnostics in the demo app** ([#95](https://github.com/Offline-Protocol/offline-protocol-sdk/pull/95)) — A new Diagnostics tab and persistent status pill expose transport-time distribution, DORS switch drivers, link-stability flap counts, per-transport delivery/error/latency, hop-count histograms, retry queue by priority, partition duration, and battery drain rate. Aggregate-only — no per-peer data, no message content, no per-decision internals.
- **Telemetry wire-up guide** (`docs/telemetry.md`) — Push and pull paths from React Native, the Rust `TelemetrySink` example, the UniFFI callback shape, the config knobs (cadence, verbosity, scrubIds, pollQueue), and what each `MetricsFrame` field means.

#### New transports

- **Reticulum mesh transport** ([#75](https://github.com/Offline-Protocol/offline-protocol-sdk/pull/75), [#80](https://github.com/Offline-Protocol/offline-protocol-sdk/pull/80), [#84](https://github.com/Offline-Protocol/offline-protocol-sdk/pull/84)) — A long-range, low-bandwidth mesh transport (LoRa, TCP, UDP, serial, I2P). The Rust side mirrors `InternetTransport` (async confirmation loop with 120 s timeout, reconnect logic, `SharedCallback` for platform notification). DORS profile is reliability-weighted (0.30) and energy-efficient (0.25) at tie-break priority 3. Full UniFFI bridge plus `ReticulumManager` for Android (TCP socket) and iOS (NWConnection) following the `InternetManager` lifecycle. `isAvailable()` is gated behind `configure()` so DORS cannot select an unconfigured transport. The Rust scoring layer was refactored alongside this PR — `TransportScoringProfile` per transport replaces ~10 hardcoded match arms in `calculate_*_score` functions. `bandwidth_max_bps` is `2700` (corrected from an aspirational `4700`). Daemon TCP JSON protocol (`Identify`, `SendMessage`, `MessageReceived`, `StatusUpdate`) is documented in `docs/reticulum.md`.
- **Nostr relay transport** ([#82](https://github.com/Offline-Protocol/offline-protocol-sdk/pull/82), [#83](https://github.com/Offline-Protocol/offline-protocol-sdk/pull/83)) — A censorship-resistant decentralized relay layer over WebSockets. The Rust side handles BIP-340 Schnorr signing via `k256`; platform managers (`NostrManager` on iOS via `URLSessionWebSocketTask`, on Android via `OkHttp`) just publish pre-signed `["EVENT", {...}]` JSON to N relays simultaneously and subscribe via NIP-01 REQ filters. Confirmation is deferred to the relay's `["OK", event_id, true]` response (rejections trigger `nostrSendFailedWithReason`). Per-message signing-failure retry capped at 3. DORS profile is reliability-weighted (0.35) at tie-break priority 4 — fallback for when the usual transports are censored. Includes a minimal `examples/nostr-example/` app for end-to-end verification.
- **Adaptive per-peer BLE fragment sizing (MTU negotiation)** ([#86](https://github.com/Offline-Protocol/offline-protocol-sdk/pull/86)) — BLE fragmentation has been nailed to 185 bytes since forever — `set_mtu` was never wired through UniFFI. The Rust side now stores a per-peer map keyed by recipient with clamp `[BLE_MAX_FRAGMENT_SIZE, MAX_REASONABLE_BLE_PAYLOAD]`. Android wires `requestMtu(517)` into the central handshake chain between `onServicesDiscovered` and the device-id read, with a 3 s watchdog so vendor stacks that accept `requestMtu` and never deliver `onMtuChanged` cannot wedge the handshake. iOS reads `maximumWriteValueLength(for: .withoutResponse)` at the moment the device id resolves. Late `onMtuChanged` callbacks that arrive after the watchdog still get forwarded so peers do not stay pinned to the fallback. Two new counters (`ble_fragment_fallback_count`, `ble_undersized_mtu_reports`) surface ordering-invariant regressions and below-floor renegotiations to dashboards. Wire format unchanged.

#### Bindings & platform

- **Python desktop bindings for macOS, Linux, and Windows** ([#76](https://github.com/Offline-Protocol/offline-protocol-sdk/pull/76)) — A full Python SDK under `bindings/python/` mirroring the mobile binding architecture: `SecureStorage` via `keyring` (Keychain / Secret Service / Credential Locker), `InternetManager` via `websockets`, `BleManager` via `bleak` (central role), `BlePeripheral` via `bless` (GATT server, macOS/Linux only), and a high-level `ProtocolManager` with 100 ms processing loop and async-context-manager support. Generated `offline_protocol.py` is checked in; CI runs a `python-bindings` job on every PR that regenerates and fails on drift. Three Python deps audited for supply-chain safety; CI gets a `pip-audit` step. 7 new platform jobs (mac arm64+x86_64, linux x86_64+aarch64, windows x86_64). Known limitations are spelled out in `bindings/python/README.md`.
- **Reticulum and Nostr UniFFI bridges and platform managers** ([#80](https://github.com/Offline-Protocol/offline-protocol-sdk/pull/80), [#83](https://github.com/Offline-Protocol/offline-protocol-sdk/pull/83)) — Wired through `OfflineProtocolModule` (initialize, start, stop, pause, resume, destroy, enableTransport, disableTransport) on iOS and Android with full lifecycle support and DORS failure tracking. iOS uses `NSLock`-backed properties for shared state; Android uses `Atomic*` plus a dedicated `HandlerThread` for I/O so TCP writes never run on the main thread. Both transports follow the same lifecycle: native `initialize()` creates the manager, JS `start()` auto-enables, native `enableTransport` calls `configureAndStart` ([#84](https://github.com/Offline-Protocol/offline-protocol-sdk/pull/84) aligned the lifecycles).
- **Cancel connection requests** ([#70](https://github.com/Offline-Protocol/offline-protocol-sdk/pull/70)) — Apps can cancel an in-flight connection request before the peer responds.
- **BLE backpressure-aware drain** ([#87](https://github.com/Offline-Protocol/offline-protocol-sdk/pull/87)) — Outbound BLE drain now responds to write completion: a 50 ms delayed re-drain replaces the unconditional immediate re-post when writes stall, and `onCharacteristicWrite` triggers an immediate drain so fragments flow as soon as the remote acknowledges. Drain is gated on `GATT_SUCCESS` so failed writes do not enter a tight retry loop. Demo app gains BLE permissions for all API levels and a deep-link to system Settings for `NEVER_ASK_AGAIN` permission denials. Group conversations track per-message delivery/failure status.

### Deprecated

- **`updateTransportMetrics(...)` is a documented no-op, removal targeted for v1.0** — This method predates the per-transport tracking the Rust core now performs internally and has never written to any field the SDK reads from. All passed fields — including the 12 newly-added optional extended fields — are discarded. Use `getTransportMetrics(...)` to read live metrics, or install a `TelemetrySink` to observe push `MetricsFrame`s. A `std::sync::Once`-gated `tracing::warn!` fires on first call so misuse surfaces during development. The method is scheduled for removal in the **v1.0** release.

### Bug Fixes

- **Allow session confirmation when welcome is in `SendAttempted` state** ([#73](https://github.com/Offline-Protocol/offline-protocol-sdk/pull/73)) — `can_confirm_from_source()` only accepted `WelcomeDeliveryState::Sent`, but the welcome lifecycle can stay at `SendAttempted` (DORS falling back from BLE to Internet, or Internet transport awaiting platform confirmation). Encrypted messages silently queued and never flushed. The gate now opens at `SendAttempted`, and `on_transport_send_confirmed()` issues an immediate confirmation probe so Internet sessions don't wait up to 7 s for the next `process()` tick.
- **Strict nullable returns for Kotlin 2.x compat** ([#74](https://github.com/Offline-Protocol/offline-protocol-sdk/pull/74)) — Kotlin 2.x stopped being lenient about Java nullable annotations: every `JSONObject.optString()` and `ReadableArray.getString()` now returns strict `String?`. Introduced `safeOptString` / `optNullableString` extensions in `JsonExtensions.kt` and migrated ~58 call sites across `OfflineProtocolModule.kt`, `InternetManager.kt`, and `BleManager.kt`. Read-receipt IDs filter null/empty entries via `mapNotNull` instead of coercing to empty strings (which the Rust side silently accepted).
- **Resolve BLE peripheral peer identity and broadcast routing** ([#78](https://github.com/Offline-Protocol/offline-protocol-sdk/pull/78)) — Added `resolve_sender_identity()` to map a central UUID to a `user_id` from the first received message, enabling outgoing routing via the BLE peripheral. `send_message("*")` now expands to individual BLE peers (the Rust core treats `"*"` as a literal peer-ID lookup that always fails). Cached the last-known central UUID to prevent sender flipping during fragment reassembly, and clean up resolved `user_id` entries on peer disconnect.
- **Prevent BLE scanner from stealing peripheral's outgoing fragments** ([#79](https://github.com/Offline-Protocol/offline-protocol-sdk/pull/79)) — Scanner and peripheral share a single `ble_get_next_fragment` queue; on `on_fragments_available`, both drain — but the scanner ran first and popped fragments it could not deliver (no bleak client for peripheral-discovered peers), dropping all Mac→Phone messages. Scanner now drains only when it has connected clients.
- **Replace reflection with direct typed UniFFI callbacks on Android** ([#81](https://github.com/Offline-Protocol/offline-protocol-sdk/pull/81)) — BLE and WiFi Direct callbacks were wired via `Class.forName` + `Proxy.newProxyInstance` even though the generated bindings already expose `BleTransportCallback` and `WifiDirectTransportCallback` as public typed interfaces. Replaced with direct object expressions, matching iOS and the existing Reticulum path. Compile-time verification that callback interfaces exist; stale bindings still fall back to polling rather than crashing.
- **Drain session state on BLE status change and WiFi Direct stop** ([#88](https://github.com/Offline-Protocol/offline-protocol-sdk/pull/88)) — `BleTransport::on_status_changed()` was a bare status setter, despite being the *primary* path BLE goes offline in the React Native bindings (user toggles Bluetooth off → `ble_status_changed(false)`). Stale peers, MTUs, fragment buffers, and queues survived. WiFi Direct had the same hole in both `on_status_changed()` and `stop()`. Both transports now drain per-session state when transitioning away from `Available`. Monotonic lifetime counters are intentionally preserved.
- **Isolate `TelemetrySink` panics to prevent `SharedState` poisoning** ([#96](https://github.com/Offline-Protocol/offline-protocol-sdk/pull/96)) — Three families of emit sites invoked `TelemetrySink::emit` while holding the `SharedState` mutex. A panicking sink (realistic for foreign sinks via UniFFI) unwound through the live `MutexGuard`, poisoning the mutex on drop and silently degrading the protocol. A new `telemetry::dispatch::dispatch_record` helper wraps every internal sink dispatch in `catch_unwind(AssertUnwindSafe(...))`. The legacy `EventCallback` fan-out gets the same treatment. Note: `panic = abort` profiles (mobile `minisize`) bypass `catch_unwind`.
- **Unblock Python install and close UDL-drift gaps** ([#97](https://github.com/Offline-Protocol/offline-protocol-sdk/pull/97)) — `bleak` pin bumped from `>=0.21,<1.0` to `>=1.1.1,<2` so install resolves against `bless 0.3.0` (previous pins were mutually exclusive — `pip install -e .` failed). `nostr_enabled=False` added to four `ProtocolConfig` construction sites that predated the field becoming required. `_make_config` defaults `internet_enabled=True` so tests satisfy the "at least one transport enabled" validation. `ProtocolManager.start()` registers stub `NostrTransportCallback` and `ReticulumTransportCallback` impls (mirroring the WifiDirect stub), gated on `config.nostr_enabled` / `config.reticulum_enabled` so apps that drive the transport themselves are not silently swallowed. `install_telemetry_sink`, `uninstall_telemetry_sink`, and `poll_telemetry_frame` passthroughs are added with strict GC pin lifecycle (pin first, call Rust, unpin on failure; `stop()` only clears pins after teardown succeeds). New CI `python-bindings` job regenerates from UDL on every PR and fails on drift.
- **Use `InternetMessage.recipient_id` in outbox drain (Python)** ([#98](https://github.com/Offline-Protocol/offline-protocol-sdk/pull/98)) — UniFFI-generated `InternetMessage` exposes `recipient_id`, not `recipient`. Every outgoing WebSocket send raised `AttributeError`, propagated into `_safe_handle_authenticated` → `_handle_connection_closed`, triggering an auth/reconnect flap on any queued message.
- **Tear down WebSocket and recv loop on disconnect (Python)** ([#99](https://github.com/Offline-Protocol/offline-protocol-sdk/pull/99)) — `_handle_connection_closed` cancelled poll/ping but never closed `self._ws` or cancelled `self._recv_task`. On `AuthError`, the WebSocket stayed open and the recv loop kept iterating it; `_connect` then overwrote `_recv_task` with a fresh loop against a new socket — two live recv tasks, one zombie WS. Added a `_teardown_in_progress` re-entrancy flag and a `_process_tasks` strong-reference set so fire-and-forget tasks cannot be GC'd mid-execution. `_schedule_reconnect` cancels any prior `TimerHandle` before installing a new one.
- **Guard `BlePeripheral` peer maps with existing lock (Python)** ([#100](https://github.com/Offline-Protocol/offline-protocol-sdk/pull/100)) — `self._lock` previously protected only metrics counters even though the bless delegate thread (`_on_write` → `_resolve_sender`) and the asyncio loop (`_peer_monitor_loop`, `resolve_sender_identity`, `stop`) read and wrote `_connected_centrals`, `_central_to_user_id`, and `_last_known_central` concurrently. Iteration-during-mutation surfaced as `RuntimeError`s in production, and single-peer sender attribution could flip between threads on the same message. Lock now covers all three fields under a strict "snapshot under lock, FFI outside" rule.
- **Copy `uniffi.dll` on Windows instead of symlinking** ([#101](https://github.com/Offline-Protocol/offline-protocol-sdk/pull/101)) — `ln -sf` on Git Bash / MSYS requires `SeCreateSymbolicLink` (admin or Developer Mode); without it the symlink is either broken or a regular copy depending on `MSYS2 winsymlinks`. macOS / Linux keep the symlink; Windows uses a straight `cp -f`.
- **Raise on `send_message("*")` with no known BLE peers (Python)** ([#102](https://github.com/Offline-Protocol/offline-protocol-sdk/pull/102), [#104](https://github.com/Offline-Protocol/offline-protocol-sdk/pull/104)) — Previously fell through to `protocol.send_message(recipient="*", ...)` whenever `_get_known_ble_peers()` returned empty; the Rust core's `BleTransport::send()` treats `"*"` as a literal peer-ID lookup and always fails — the caller got a message id back that would never deliver, with no surfaced error. `send_message("*")` is now explicitly documented as a BLE-only wrapper convenience and raises `ValueError` on empty peer sets. `_get_known_ble_peers` is promoted to public `get_known_ble_peers` since it is now part of the documented escape hatch. Internet/Nostr/Reticulum/Wi-Fi Direct broadcasts remain platform-driven.

### Refactoring

- **Android BLE facade split** ([#85](https://github.com/Offline-Protocol/offline-protocol-sdk/pull/85)) — The 3300-line `BleManager.kt` is decomposed into focused classes under `bindings/react-native/android/.../ble/`: `BleTransportFacade` (entry point), `PeripheralGattServer` (GATT server with CCCD descriptor handling and per-central long-read snapshots), `LeAdvertiser` (advertising lifecycle with cooldown/jitter), `CentralGattClient` (per-peer central-role state machine), `OutboundFragmentQueue` and `InboundFragmentBuffer` (main-thread-enforced FIFO buffers with whole-queue drop on overflow — half-message fragments would reassemble into garbage at the receiver). The PR also closes long-standing correctness gaps in BLE: missing CCCD (0x2902) descriptor on the message characteristic (centrals could not subscribe at all), missing `onDescriptorWriteRequest` handler (subscribe attempts silently dropped), missing CCCD write on the central side (notify stream was silent in both directions), `onCharacteristicReadRequest` ignoring the read offset (long reads returned the full value on every call, garbage on iOS↔Android with default 23-byte ATT MTU), binder-thread reads of `characteristic.value` captured by reference (framework reuses the buffer — main-thread handler saw whatever the BLE stack last wrote). All UniFFI calls now run on the main handler. `provideIdentityBytes` is a pure volatile read; identity refresh is scheduled with 500 ms → 10 s exponential backoff capped at 30 attempts. `assertMainThread` runtime guards replace load-bearing comments.
- **Extract `CategorySampler` from `MlsEventRateLimiter`** ([#90](https://github.com/Offline-Protocol/offline-protocol-sdk/pull/90)) — The fixed-window rate-limiting logic is generic (a keyed counter with window reset and eviction) but was welded to MLS event types. Extracted into `CategorySampler` under a new `telemetry` module so the new sink categories can reuse it. `MlsEventRateLimiter` becomes a thin wrapper that maps MLS lifecycle events to string keys.

### Documentation

- **Telemetry wire-up guide** ([#95](https://github.com/Offline-Protocol/offline-protocol-sdk/pull/95)) — New `docs/telemetry.md` covers push/pull paths from React Native, Rust `TelemetrySink` examples, the UniFFI callback shape, config knobs, and `MetricsFrame` field semantics.
- **Reticulum integration guide** ([#75](https://github.com/Offline-Protocol/offline-protocol-sdk/pull/75)) — New `docs/reticulum.md` documents architecture, integration strategies (embedded Python, the emerging `reticulum-rs` crate, HDLC IPC, TCP gateway via `TCPClientInterface`), DORS scoring, the LoRa throughput reference table, daemon setup, RNode hardware, and troubleshooting. Reticulum also added to all 15 existing docs that enumerate transports, scoring weights, config parameters, or platform availability.

### Chores

- **Point repo URLs at the actual canonical slug** ([#105](https://github.com/Offline-Protocol/offline-protocol-sdk/pull/105), [#106](https://github.com/Offline-Protocol/offline-protocol-sdk/pull/106)) — `Cargo.toml` had us at `offline-protocol/sdk`, `cliff.toml` at `nickthecook/offline-protocol-sdk`, and CONTRIBUTING/QUICKSTART/iOS-integration each linked to one of those. None of those slugs exist. The actual remote is `Offline-Protocol/offline-protocol-sdk`. Crate metadata, generated changelogs, clone instructions, the SwiftPM dependency URL, and React Native + Python package metadata now point at something a human or `cargo` can fetch.

## [0.9.1] — 2026-03-20

### Bug Fixes

- **Add missing `RCT_EXTERN_METHOD` declarations for iOS bridge** ([#69](https://github.com/Offline-Protocol/offline-protocol-sdk/pull/69))
  Seven `@objc func` implementations in `OfflineProtocolModule.swift` had no corresponding `RCT_EXTERN_METHOD` macros in the `.m` bridge file, which meant React Native on iOS could not call them at all (Android worked fine via `@ReactMethod`). The missing methods: `sendPresenceUpdate`, `sendTypingIndicator`, `sendReadReceipt`, `getIdentityPublicKey`, `deriveUserIdFromPublicKey`, `signData`, and `verifySignature` — the entire presence/typing block and all identity crypto methods. A full audit of all 120 methods confirmed these were the only ones missing.

### CI/CD

- **Parallelize release and CI workflows** ([#68](https://github.com/Offline-Protocol/offline-protocol-sdk/pull/68))
  Split the Android release build into a matrix of 4 parallel jobs (one per ABI), cutting release time significantly. Cached the `uniffi-bindgen` binary and switched to a pre-built `git-cliff` binary. Moved the release job to `ubuntu-latest` (it only needs Node and git-cliff, not a macOS runner). Split CI check into parallel fmt/clippy/test jobs so PRs get feedback in ~3 min instead of ~6 min. Added concurrency groups to cancel stale PR runs, restricted coverage to main-only pushes, added `workflow_dispatch` with a `dry_run` input for manual pipeline testing, and added artifact verification before publishing. Fixed a script injection vulnerability in version extraction by moving input interpolation into an env binding.

### Documentation

- **Wiring guide for Rust-to-React-Native method flow** — Added a step-by-step guide (`bindings/react-native/WIRING_GUIDE.md`) covering the six layers from Rust to TypeScript, common mistakes, parameter type mappings, and a smoke test snippet to prevent missing iOS bridge declarations.

---

## [0.9.0] — 2026-03-20

### Breaking Changes

#### Low-level MLS group API removed from UniFFI bindings

The following methods exposed raw MLS group operations that bypassed role checks, fan-out, and mesh routing. They have been removed from the UniFFI surface (Swift, Kotlin, React Native). Use the high-level mesh group API instead.

| Removed method | Replacement |
|---|---|
| `mlsCreateGroup(groupName)` | `meshCreateGroup(groupName)` |
| `mlsAddGroupMember(groupId, keyPackage)` | `meshInviteToGroup(groupId, inviteeUserId)` |
| `mlsRemoveGroupMember(groupId, memberId)` | `meshRemoveFromGroup(groupId, memberId)` |
| `mlsLeaveGroup(groupId)` | `meshLeaveGroup(groupId)` |
| `mlsEncryptForGroup(groupId, plaintext)` | `meshSendGroupMessage(groupId, content)` |
| `mlsDecryptFromGroup(encrypted)` | Handled automatically by the protocol engine |
| `mlsJoinGroup(welcome)` | Handled automatically via Welcome processing |
| `mlsListGroups()` | `meshListGroups()` |
| `mlsGetGroupInfo(groupId)` | `meshGetGroupInfo(groupId)` |

#### Transport stub methods removed

`addInternetTransport(serverUrl, port)` and `addWifiDirectTransport()` were no-op stubs that always returned errors. They have been removed from UniFFI, UDL, and all platform bindings (JNI, Kotlin, Swift, TypeScript).

#### `ProtocolState` enum variants removed

`Starting` and `Stopping` were never set by the engine and have been removed. The enum is now `Stopped | Running | Paused`. If you have exhaustive `switch`/`when` statements over `ProtocolState`, remove the `Starting` and `Stopping` cases.

#### Admin-only group operations (behavioral change)

`meshInviteToGroup()`, `meshRemoveFromGroup()`, `meshSetMemberRole()`, and `meshRenameGroup()` now enforce admin-only access. If a non-admin calls these methods, they will throw with `Error::NotGroupAdmin`. The group creator is automatically assigned the `Admin` role. If your app previously allowed any member to invite/remove, you must either promote them to admin first or adjust your UI to reflect the new permission model.

#### New group role management APIs

Three new methods have been added to the high-level mesh group API:

- **`meshSetMemberRole(groupId, userId, role)`** — Change a member's role (admin only). `role` must be `"admin"` or `"member"`.
- **`meshGetMemberRole(groupId, userId)`** — Get a member's current role.
- **`meshGetGroupRoles(groupId)`** — Get all member roles as a `Record<string, string>`.

A new event **`group_role_changed`** is emitted when a role changes:

```typescript
protocol.on('group_role_changed', (event) => {
  console.log(`${event.user_id} is now ${event.new_role} (changed by ${event.changed_by})`);
});
```

If you use exhaustive `switch` statements on `ProtocolEvent['type']`, add a case for `'group_role_changed'`.

#### Rust-level events added

The following events are emitted at the Rust/UniFFI layer. If you are consuming the SDK directly through UniFFI (Swift/Kotlin), you should handle these:

- **`GroupRoleChanged`** — A member's role was changed in a group (also bridged to React Native as `group_role_changed`).
- **`GroupRenamed`** — A group was renamed, includes `group_id`, `new_name`, `old_name`, and `renamed_by`.

### Features

- **Group rename API** — `renameGroup(groupId, newName)` lets admins rename a group and broadcasts the change to all members via a `__GRP_RENAME__` internal message. A `GroupRenamed` event is emitted on all peers with `group_id`, `new_name`, `old_name`, and `renamed_by`. Available via UniFFI (Swift/Kotlin) and React Native (`meshRenameGroup`).

- **`getGroupInfo` on the high-level API** — Replaces the removed low-level `mlsGetGroupInfo`. Returns group metadata including members, epoch, and timestamps. Available via UniFFI as `getGroupInfo(groupId)` and React Native as `meshGetGroupInfo(groupId)`.

- **Dead code and security bypass cleanup** ([#67](https://github.com/Offline-Protocol/offline-protocol-sdk/pull/67))
  Removed unused struct fields (`GroupManager::user_id`, `SessionManager::storage`, `InternetState::server_url`, UniFFI `OfflineProtocol::user_id`), all annotated with `#[allow(dead_code)]`. Simplified `GroupManager::new` signature (no longer takes a `user_id` parameter). Regenerated all UniFFI bindings (Kotlin, Swift, C header, JNI, TypeScript) to reflect the consolidated API surface.

- **Group role management and security hardening** ([#66](https://github.com/Offline-Protocol/offline-protocol-sdk/pull/66))
  Added app-layer role tracking for MLS groups with a typed `GroupRole` enum (`Admin` / `Member`). The group creator is automatically assigned the `Admin` role. Admins can invite/remove members and change roles; non-admins are rejected with a typed error. Key security improvements: last-admin invariant prevents orphaned groups (demoting or removing the last admin is blocked), deterministic admin election on leader departure using lexicographic fallback, phantom member cleanup on group join, and removed member notification via a plaintext `__GRP_REMOVED__` control message so kicked members can clean up local state immediately. Key packages are automatically replenished after member removal so subsequent invites don't fail. Includes new `meshSetMemberRole()`, `meshGetMemberRole()`, and `meshGetGroupRoles()` APIs with full UniFFI and React Native bindings, a `GroupRoleChanged` event, and 1200+ lines of new tests.

### Bug Fixes

- **Reject empty group names** — `create_group` and `rename_group` now validate the group name: whitespace is trimmed and empty strings are rejected with a descriptive error. Previously, an empty name could be broadcast to all group members.

- **Wire `resetTofuForPeer` through all platform bindings** — The TOFU reset API (`resetTofuForPeer`) is now available in React Native (TypeScript), iOS (Swift native module), and Android (Kotlin native module). Previously it was only callable from Rust/UniFFI. After calling this, the next message from the peer will establish a new trust pin.

- **Wire `renameGroup` through all platform bindings** — `meshRenameGroup` is now wired through the iOS Swift native module, iOS Objective-C bridge, Android Kotlin native module, and the UniFFI-generated Kotlin/Swift bindings. The React Native TypeScript wrapper and the Rust/UniFFI layer already had this method.

- **Notify removed members and replenish key packages** — Removed members now receive a plaintext `__GRP_REMOVED__` notification so they can clean up local state immediately instead of silently losing access. After member removal, key packages are automatically replenished so subsequent invites don't fail with a stale key package error.

- **Fix phantom members on group join** — Fixed an issue where the local member list could include stale members after joining a group via Welcome. Member lists are now reconciled from the MLS group state on join.

- **Fix deterministic admin election** — Admin election on leader departure now uses a deterministic lexicographic sort, preventing split-brain scenarios where different nodes elect different admins. Election failures are logged rather than silently swallowed.

- **Close last-admin loopholes** — Prevented several edge cases where a group could become orphaned (no admin): demoting the last admin, removing the last admin, and the last admin leaving are all now blocked with explicit errors.

---

## [0.8.0] — 2026-03-19

### Breaking Changes (React Native Bindings)

If you are building an app with the React Native bindings, the following changes require updates to your code:

#### New error variants: `UserBlocked` and `LockPoisoned`

Two new error variants have been added to `ProtocolError`:

- **`UserBlocked`** — Thrown when attempting to send a message, control signal, or establish a connection with a blocked user. If your app catches `ProtocolError` exhaustively, you must now handle this case.
- **`LockPoisoned`** — Thrown when an internal mutex is in a poisoned state (indicates a prior panic inside the SDK). This replaces the previous behavior where 129 `lock().unwrap()` calls would panic the process. Apps should treat this as a fatal internal error and consider restarting the protocol engine.

#### New `ProtocolConfig` fields

Three new fields have been added to `ProtocolConfig`. They have defaults so existing code will compile, but you should review them:

| Field | Type | Default | Description |
|-------|------|---------|-------------|
| `max_group_members` | `u32` | `256` | Maximum members allowed in a single MLS group |
| `group_relay_enabled` | `boolean` | `true` | Whether relay broadcasting is used for group fan-out |
| `require_transport_identity` | `boolean` | `false` | Enables Ed25519 sender identity binding at the transport layer |

#### New event types in React Native bindings

The following event types have been added to the React Native TypeScript types:

- **`service_discovered`** — A peer advertised a mesh service in response to a discovery query.
- **`service_request_received`** — An incoming service request from another peer.
- **`service_response_received`** — A response to a service request you sent.
- **`presence_updated`** — A peer sent a presence update (Online, Away, Offline).
- **`typing_indicator_received`** — A peer started or stopped typing.
- **`read_receipt_received`** — A peer read one or more of your messages.
- **`message_relayed`** — A message was relayed through this node in the mesh.
- **`message_deferred`** — A message was queued for later delivery because no transport was available.

#### Rust-level events (not yet bridged to React Native)

The following events are emitted at the Rust/UniFFI layer but are not yet exposed in the React Native TypeScript types. If you are consuming the SDK directly through UniFFI (Swift/Kotlin), you should handle these:

- **`UserBlocked`** / **`UserUnblocked`** — A user was blocked or unblocked locally.
- **`MessageDecryptionFailed`** — An incoming message could not be decrypted.
- **`GroupEpochForkDetected`** / **`GroupEpochForkResolved`** — MLS epoch fork lifecycle events.
- **`SecurityWarning`** — A control message failed authentication or replay checks.
- **`TofuReset`** — TOFU trust state was reset for a peer.

#### `ForwardInfo` added to message events

`MessageReceivedEvent`, `MessageSentEvent`, and `GroupMessageReceivedEvent` now include an optional `forward_info` field. If your app displays messages, you should check for this field to show forwarding attribution:

```typescript
interface ForwardInfo {
  original_sender: string;
  original_message_id: string;
  original_timestamp: number;
  forward_count: number;
}
```

#### Native bridge expansion (iOS & Android)

The native modules (`OfflineProtocolModule.swift` and `OfflineProtocolModule.kt`) have been significantly expanded. If you have custom native module extensions or overrides, you will need to add implementations for the new methods: `blockUser`, `unblockUser`, `getBlockedUsers`, `isUserBlocked`, `forwardMessage`, `meshForwardMessageToGroup`, `sendPresenceUpdate`, `sendTypingIndicator`, `sendReadReceipt`, `resetTofuForPeer`, and the full `MeshServices` API surface (`registerService`, `unregisterService`, `discoverServices`, `sendServiceRequest`, `respondToServiceRequest`).

---

### Features

- **Service discovery and request/response** ([#45](https://github.com/Offline-Protocol/offline-protocol-sdk/pull/45))
  Added a new `MeshServices` subsystem that enables peer-to-peer service discovery and typed request/response over the mesh network. Services are advertised via gossip broadcast and discovered without a central registry. Includes auto `not_found` responses for unknown services, known_peers tracking independent of MLS encryption state, configurable max-hops gossip limit to control broadcast radius, payload size limits and capacity bounds to prevent resource exhaustion, sender-based response routing for multi-hop meshes, and the new `OutboundMessage` struct replacing raw tuples throughout the send path. Full UniFFI bindings are included for iOS and Android.

- **Transport-agnostic MLS group messaging** ([#46](https://github.com/Offline-Protocol/offline-protocol-sdk/pull/46))
  Wired MLS group encryption into the protocol engine, enabling encrypted group conversations that work seamlessly across BLE, WiFi Direct, and Internet transports. Messages are encrypted once and fan-out to all group members via DORS-selected transports. Adds configurable `max_group_members` (default 256), relay broadcast optimization for large groups, pending MLS commit buffering with TTL-based expiry, classification of MLS errors into permanent (e.g., bad state) vs retriable (e.g., out-of-order) categories, and extraction of `GroupMeshState` for cleaner state management. Includes 82 new tests in a dedicated test module.

- **Presence, typing indicators, and read receipts** ([#48](https://github.com/Offline-Protocol/offline-protocol-sdk/pull/48))
  Added protocol-level support for three real-time communication signals: presence updates with a `PresenceStatus` enum (Online, Away, Offline), typing indicators with per-conversation granularity, and read receipts supporting batch message IDs. All three are implemented as lightweight internal control messages routed through DORS, meaning they work across any transport without a relay server. Input validation prevents empty recipient/conversation IDs and excessive message ID lists. Full UniFFI and React Native bindings for mobile with 25 tests covering edge cases.

- **User blocking with silent message filtering** ([#54](https://github.com/Offline-Protocol/offline-protocol-sdk/pull/54))
  Added a complete user blocking system with a locally-persisted block list. Blocked users are filtered in the receive pipeline (after dedup but before ACK, so blocked senders never learn they are blocked), with guards on all outbound paths including send, control messages, and connection establishment. Blocking persists across restarts via `MlsStorage`. Unblocking a user cleans up stale MLS sessions. Includes a typed `Error::UserBlocked` variant, outbound presence leak prevention (so blocked users don't see your status), file transfer cleanup, a `MAX_BLOCKED_USERS` cap, and full UniFFI/React Native bindings.

- **User-level message forwarding with attribution** ([#61](https://github.com/Offline-Protocol/offline-protocol-sdk/pull/61))
  Introduced first-class message forwarding as a protocol feature. Forwarded messages carry a `ForwardInfo` struct containing the original sender, original message ID, original timestamp, and a forward count. Both 1:1 (`forward_message()`) and group (`forward_message_to_group()`) forwarding are supported. The pending queue preserves forwarding attribution through retries, relay broadcast handles forwarded group messages, and a `MAX_FORWARD_COUNT=100` cap prevents infinite forwarding chains. Content type and media metadata are preserved through the forwarding path. Full React Native bindings included.

- **Presence, typing, and read receipts wired to React Native** ([#65](https://github.com/Offline-Protocol/offline-protocol-sdk/pull/65))
  Wired `sendPresenceUpdate`, `sendTypingIndicator`, and `sendReadReceipt` through the React Native TypeScript wrapper, iOS Swift native module, iOS Objective-C bridge, and Android Kotlin native module. Also wired into the demo app with UI controls for all three signals.

- **Demo app** — Added a simple demo app (`examples/demo-app/`) showcasing all SDK features including messaging, groups, presence (via `sendPresenceUpdate`), typing indicators, read receipts, service discovery, blocking, forwarding, and message relay/deferral tracking. Uses the production-recommended reliability config (10 retries, 10s ACK timeout).

### Performance

- **Reduce message latency from invitation to delivery** — Overhauled polling and timing across the stack to dramatically reduce the time from MLS invitation to first decryptable message. Replaced the 750ms × 8 fixed-interval MLS establishment polling with a 100ms exponential backoff helper that resolves faster in the common case. Aligned the Android process tick interval with iOS (500ms → 100ms) to eliminate a platform-specific latency gap. Reduced startup delay from 500ms to 100ms, and presence rebroadcast interval from 60s to 15s for faster peer discovery. Tightened reliability config in the example app to match production expectations.

### Bug Fixes

- **Harden mesh group robustness** ([#47](https://github.com/Offline-Protocol/offline-protocol-sdk/pull/47))
  Fixed several issues that caused group messaging to degrade under real-world conditions. Stale relay caches now refresh from MLS membership on each fan-out. Added a leave election fallback with staggered re-election timeouts so groups can recover when the elected leader crashes. Implemented epoch fork detection using Lamport clock comparison and automatic resolution via leader-elected key-update commits. Added a circuit breaker on elections to prevent election storms, tuple-keyed leave elections to handle concurrent leaves, and per-attempt cooldown to prevent rapid-fire retries.

- **Harden control message authentication and sender verification** ([#49](https://github.com/Offline-Protocol/offline-protocol-sdk/pull/49))
  Comprehensive security hardening of the control message path. Added transport-level sender identity binding so peers can verify who sent each control message. Implemented Ed25519 control message signing with TOFU (Trust On First Use) key pinning — the first time you communicate with a peer, their signing key is recorded, and all future control messages are verified against it. Added protections against internal prefix injection (where a malicious peer crafts payloads that look like control messages), LRU TOFU eviction for bounded memory, replay protection via nonce tracking, length-prefixed binary signing payloads with domain separators, and a `SecurityRejected` variant that suppresses ACKs for rejected messages so attackers don't get delivery confirmation.

- **Harden TOFU transport prefixes** ([#50](https://github.com/Offline-Protocol/offline-protocol-sdk/pull/50))
  Hardened prefix handling in the TOFU transport layer to prevent prefix confusion attacks.

- **Harden TOFU storage and validate identity strings** ([#51](https://github.com/Offline-Protocol/offline-protocol-sdk/pull/51))
  Added input validation throughout the identity system. `UserId` and `AppId` constructors now reject storage-hostile characters (path separators, null bytes) and all ASCII control characters to prevent key injection and filesystem traversal. TOFU restore keys are validated before use. TOFU peer restore is capped at `MAX_TOFU_PEERS` with a deterministic secondary sort for consistent truncation behavior.

- **Wire user blocking to native bridges** ([#55](https://github.com/Offline-Protocol/offline-protocol-sdk/pull/55))
  Wired the Rust-level user blocking API through to the Android and iOS native bridge modules and fixed incorrect field mappings in service discovery event payloads.

- **Address 14 bugs from full codebase audit** ([#56](https://github.com/Offline-Protocol/offline-protocol-sdk/pull/56))
  Fixed 14 bugs found during a systematic codebase audit: ACK piggyback overflow was silently dropped (messages lost), `FileChunk` had unbounded memory allocation (DoS vector), `finalize_file` skipped SHA256 checksum verification (integrity gap), `LamportClock` deserialization bypassed value clamping (could overflow), UniFFI storage error variant was mismapped (wrong errors surfaced to apps), `RetryEntry` had an Eq/Ord contract violation causing duplicate enqueues, routing table had a stale reverse index (phantom routes), DORS produced NaN scores when `ttl==0`, `MockTransport` used LIFO instead of FIFO ordering (tests didn't match real behavior), UniFFI event callback could deadlock under contention, `received_messages` used O(n) removal (degraded with message volume), `RetryQueueStats` was missing the Critical priority level, and added `#![deny(unsafe_code)]` to the MLS crate.

- **Production readiness improvements** ([#57](https://github.com/Offline-Protocol/offline-protocol-sdk/pull/57))
  Replaced all 129 `lock().unwrap()` calls across the codebase with poison-recovering lock wrappers, so a panic in one thread no longer takes down the entire SDK. Added a CI pipeline with fmt, clippy, test, cargo-deny license/advisory checking, and code coverage. Added a TOFU reset API for apps that need to clear trust state. Added a 1MB max message size guard at the transport layer to prevent oversized payloads from crashing BLE stacks. Added cargo-deny config and SECURITY.md. Fixed `receive_message()` silently dropping messages when serialization failed (now returns a proper error).

- **Save BLE fragments on missing peripheral** ([#58](https://github.com/Offline-Protocol/offline-protocol-sdk/pull/58))
  BLE fragments are now saved to a buffer when the peripheral connection is temporarily unavailable, instead of being silently dropped. This fixes a data loss issue where BLE messages were lost during brief connection interruptions.

- **Enforce FIFO ordering for BLE fragment queues** ([#59](https://github.com/Offline-Protocol/offline-protocol-sdk/pull/59))
  Fixed BLE fragment queues to enforce strict FIFO ordering. Previously, fragments could be delivered out of order, causing message reassembly failures on the receiving side.

- **Mesh networking fixes** ([#60](https://github.com/Offline-Protocol/offline-protocol-sdk/pull/60))
  A collection of mesh networking improvements: added unicast multi-hop relay forwarding with a `MessageRelayed` event so apps can track relay activity, switched dedup storage to LRU eviction for bounded memory, added composite-score route eviction so stale routes are pruned based on quality rather than age alone, enabled multi-hop service discovery responses with an originator field for correct return routing, reduced epoch fork false positives by tightening detection thresholds, adopted RFC 1982 serial number arithmetic for sequence numbers (handles wrapping correctly at 2^32), and extracted relay logic into a dedicated helper module.

- **Decouple transport retries from ACK retries** ([#62](https://github.com/Offline-Protocol/offline-protocol-sdk/pull/62))
  Fixed a fundamental reliability issue where messages permanently died after just 3 transport send failures, even though the transport was only temporarily unavailable. The root cause was that the `max_retries` limit was applied at enqueue time rather than being purely a scheduling concern. Now, enqueue is always accepted and the retry queue handles scheduling with exponential backoff. Added `drain_all()` and `flush()` methods so messages are sent immediately when a transport becomes available. Fixed ghost re-sends from un-cleaned retry queue entries, double-sends from concurrent flush paths, and zombie entries that never expired. Returns `Ok` with a `MessageDeferred` event when no transport is available (instead of `Err`). Bumped defaults to 10 retries with 10s ACK timeout for better real-world reliability.

- **Fix message forwarding** — Fixed a bug where forwarded message JSON was never parseable because the serialization format didn't match the deserialization expectation.

- **Fix group messaging** ([#63](https://github.com/Offline-Protocol/offline-protocol-sdk/pull/63))
  Six targeted fixes for group messaging: added missing dedup check in the relay group handler (duplicate messages were processed twice), prevented duplicate Welcome messages from overwriting valid MLS state (caused decryption failures for all subsequent messages), stopped raw ciphertext from leaking as `GroupMessageReceived` events on decrypt failure (apps received garbage), added retry logic for commit fan-out (commits to some members were silently lost), fixed `GroupManager` incorrectly defaulting the group display name to the internal group ID, and fixed the demo app not cleaning up local state when the current user is kicked from a group.

- **Add missing dedup to commit, Welcome, and leave handlers** ([#64](https://github.com/Offline-Protocol/offline-protocol-sdk/pull/64))
  Added deduplication to `handle_group_mls_commit`, `handle_group_mls_welcome`, and `handle_group_mls_leave` — the three group control message handlers that had no dedup at all. Without this, duplicate network deliveries caused false epoch fork detection (from double-applied commits), wasted cryptographic operations (from reprocessing Welcomes), and election timer resets (from duplicate leave messages).

### Refactoring

- **Split protocol.rs monolith** ([#52](https://github.com/Offline-Protocol/offline-protocol-sdk/pull/52))
  Split the ~3000-line `protocol.rs` file into focused sub-modules (messaging, groups, security, presence, services, etc.) for better maintainability and faster compilation. No behavioral changes.

- **Extract pending queue module** ([#53](https://github.com/Offline-Protocol/offline-protocol-sdk/pull/53))
  Extracted the pending message queue into its own module with explicit imports, reducing coupling between the queue logic and the main protocol engine.

### Documentation

- **Clean up documentation** — Removed duplicate documentation files (including `bindings/react-native/MESH.md`), fixed outdated information across READMEs and inline docs, and added a documentation index for easier navigation. If you had external references to `MESH.md`, note that it has been removed — the relevant information is now covered in inline documentation and the main README.
