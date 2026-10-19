# Flutter bindings

Flutter plugin for the [Offline Protocol SDK](https://github.com/Offline-Protocol/offline-protocol-sdk).

Native bridge sources are shared with the React Native module under
`bindings/react-native/{ios,android}` ([ADR 0025](../../docs/adr/0025-native-packages-are-assembled-in-place.md)).

## PR1 scope (engine lifecycle)

`create`, MLS initialization with platform secure storage, `start`, `localAddress`,
and `destroy`. UniFFI calls run on a dedicated engine thread; Flutter method-channel
results are posted on the main queue (see [Kotlin bridge K2](../../docs/bridges/kotlin.md#k2-nothing-blocking-on-the-main-looper)).

## PR2 scope (events, messaging, Internet)

- **Events:** subscribe with `OfflineProtocol.events` **before** `createFromConfig`
  if you need every event, including gap recovery via `replayEvents`. Native code
  only numbers and records events while a listener is attached; anything emitted
  before you subscribe is not recoverable. Payloads use the same `eventJson`
  (+ optional `seq`) shape as React Native `OfflineProtocol_Event`.
- **Messaging:** `OfflineProtocol.sendMessage`.
- **Internet:** `OfflineProtocol.enableInternetTransport`, plus auto-enable after
  `start()` when `transports.internet` is enabled with a `serverAddress` (same as
  the React Native JS layer).

## PR3 scope (transports, replay, lifecycle)

- **Event replay:** native `EventReplayRing` + method-channel `replayEvents`; Dart
  `NativeEventPipeline` implements the React Native `admitNativeEvent` gap contract.
- **Session / `seq`:** `createFromConfig` and `stop` reset Dart seq tracking; native
  event numbering restarts from zero only on `createFromConfig` (or `destroy`).
  Call `destroy` before tearing down the plugin; your `events` subscription can
  stay open across destroy → create.
- **BLE:** created when `bleEnabled` is true; started on `start()`; also via
  `OfflineProtocol.enableTransport('ble', …)`.
- **Wi‑Fi Direct / peer stream:** `enableTransport('wifiDirect', …)` and auto-enable
  after `start()` when `transports.wifiDirect.enabled` is true (Android Wi‑Fi Direct,
  iOS Network framework peer stream).
- **Foreground Internet heal:** shared `ForegroundReconnectPolicy` on Android
  (`Activity` lifecycle) and iOS (`UIApplication` notifications).
- **Nostr / Reticulum:** not in the Flutter compile surface yet; use React Native
  until a later binding PR adds them.

PR3 compiles UniFFI, config, secure storage, Internet, relay helpers, BLE, peer
stream, event replay, and teardown helpers. React Native–only services (foreground
mesh service, sticky event dispatcher, full RN module) stay excluded.

**Binary size / build time:** PR3 links the shared BLE and mesh bridge sources into
the Flutter AAR and CocoaPod (same code React Native apps already ship). Expect
longer native compiles and a modest APK/IPA increase versus PR2-only. Transports
stay opt-in via config flags.

Measure the plugin release AAR (direct read on PR3 native surface + UniFFI
`jniLibs`):

```bash
bindings/flutter/scripts/measure-plugin-aar-size.sh          # report
bindings/flutter/scripts/measure-plugin-aar-size.sh --check  # CI-style gate
```

Baseline: [baselines/offline-protocol-release-aar.json](baselines/offline-protocol-release-aar.json)
(see [baselines/README.md](baselines/README.md)).

## PR4 scope (CI and bridge contract)

- **Docs:** [docs/bridges/flutter.md](../../docs/bridges/flutter.md) (F1–F9) linked from
  [docs/bridges/README.md](../../docs/bridges/README.md).
- **CI:** GitHub Actions `Flutter Binding` job (Dart tests, Kotlin compile, AAR size gate).
- **Scripts:** `bindings/flutter/scripts/ci-build-android-jni-arm64.sh` builds arm64 UniFFI
  into React Native `jniLibs` for fast PR checks (full ABI builds remain app/release responsibility).

`createFromConfig` accepts the same optional `dors` and `relay` JSON sections as
the React Native bridge (applied via shared `InitialRuntimeConfigApplier` sources).

Shared Internet wiring lives in `InternetTransportBootstrap` (Kotlin/Swift) for
Flutter; the React Native module keeps its existing paths until a later refactor
(CI pins shared configure invariants via `internet_transport_bootstrap_keeps_rn_configure_invariants`).

## CI

GitHub Actions job **`Flutter Binding`** (`.github/workflows/ci.yml`) runs on every
pull request:

- `flutter test` and `flutter analyze` on this package (Flutter **3.27.4** in CI)
- `measure-plugin-aar-size.sh --check` (`:offline_protocol:assembleRelease` plus
  AAR footprint gate) after building arm64 UniFFI into jniLibs

On pull requests the job is split by path: **Dart tests** run for plugin or
Flutter-doc changes (and when the slow path runs); **NDK + release AAR + size
gate** run only for native paths (shared `bindings/react-native/android/**`
including consumer ProGuard rules, UniFFI UDL/C API/build inputs, root
`Cargo.lock`, engine workspace crates, baselines, Flutter scripts). Flutter
guards live in `crates/offline-protocol-uniffi/src/tests/flutter_bridge_guards.rs`
so guard-only PRs run Dart/`cargo test flutter_` without NDK; production UniFFI
edits in `lib.rs` always run the native gate. Pushes to `main` always run the
full job.

Bridge contract: [docs/bridges/flutter.md](../../docs/bridges/flutter.md).

Rust guards in `offline-protocol-uniffi` pin compile lists, method-channel
parity, and the AAR baseline JSON. Optional later: macOS job for iOS pod smoke
and full example APK/IPA size tracking.

## Prerequisites

1. Rust toolchain (see root `README.md`).
2. UniFFI bindings and native libraries (same artifacts as React Native):

```bash
./scripts/generate-bindings.sh
cd bindings/react-native
npm run build:uniffi:all
```

This produces:

- `bindings/react-native/android/src/main/jniLibs/**/*.so`
- `bindings/react-native/ios/libs/offline_protocol_uniffi.xcframework`
- Generated Kotlin/Swift under the React Native tree

3. Flutter SDK 3.10+.

## Run the example

**Android** (platform files are in the repository):

```bash
cd bindings/flutter/offline_protocol/example
flutter pub get
flutter run -d android
```

**iOS** (generate Runner once):

```bash
bindings/flutter/scripts/bootstrap-example-platforms.sh
cd bindings/flutter/offline_protocol/example
flutter run -d ios
```

See [example/README.md](offline_protocol/example/README.md) for details.

The example boots the engine with all transports disabled, subscribes to core
events, and prints this device's `off1…` address.

## Add to your app

```yaml
dependencies:
  offline_protocol:
    path: ../bindings/flutter/offline_protocol
```

```dart
import 'package:offline_protocol/offline_protocol.dart';

// Before create — required for early events and seq / replay recovery.
final sub = OfflineProtocol.events.listen((e) {
  final type = e.decodeEvent()['type'];
  // handle message_received, diagnostics, …
});

await OfflineProtocol.createFromConfig({
  'appId': 'com.example.app',
  'profile': 'default',
  'bleEnabled': true,
  'internetEnabled': true,
  'transports': {
    'internet': {
      'enabled': true,
      'serverAddress': 'wss://relay.example.com',
    },
    'wifiDirect': {'enabled': true},
  },
});
await OfflineProtocol.initializeMlsWithSecureStorage();
await OfflineProtocol.start();

final id = await OfflineProtocol.sendMessage(
  recipient: 'off1…',
  content: 'Hello',
);
```
