# Flutter bindings

Flutter plugin for the [Offline Protocol SDK](https://github.com/Offline-Protocol/offline-protocol-sdk): offline-first mesh messaging with MLS encryption.

Hand-written native bridge code is shared with the React Native module under
`bindings/react-native/{ios,android}`
([ADR 0025](../../docs/adr/0025-native-packages-are-assembled-in-place.md)).
The plugin links the same UniFFI artifacts (Kotlin/Swift generated from the UDL)
as React Native, not a separate bindgen output.

Bridge contract (for contributors changing the boundary):
[docs/bridges/flutter.md](../../docs/bridges/flutter.md).

## Requirements

| Platform | Version |
|----------|---------|
| Flutter | >= 3.10 |
| iOS | >= 13.0 |
| Android | >= API 24 (Android 7.0) |

You also need the Rust toolchain and a one-time build of the mobile UniFFI
libraries (same steps as React Native).

## Native libraries

From the repository root:

```bash
./scripts/generate-bindings.sh
cd bindings/react-native
npm run build:uniffi:all
```

This produces:

- `bindings/react-native/android/src/main/jniLibs/**/*.so`
- `bindings/react-native/ios/libs/offline_protocol_uniffi.xcframework`
- Generated Kotlin/Swift under the React Native tree

## Add the plugin

Until the plugin is published to pub.dev, depend on the path in this repository:

```yaml
dependencies:
  offline_protocol:
    path: ../bindings/flutter/offline_protocol   # adjust for your layout
```

## Platform setup

Use the same host permissions and manifests as React Native:

- **iOS:** Bluetooth usage descriptions and `UIBackgroundModes` (`bluetooth-central`,
  `bluetooth-peripheral`) before BLE starts. See
  [React Native iOS permissions](../react-native/README.md#ios-permissions).
- **Android:** Runtime Bluetooth and (where applicable) location / nearby Wi‑Fi
  permissions. See
  [React Native Android permissions](../react-native/README.md#android-permissions).

### iOS: shared bridge copy (`BridgeRN`)

CocoaPods indexes sources only under the pod root. On `pod install`, the plugin
copies shared Swift bridge files from `bindings/react-native/ios` into
`offline_protocol/ios/BridgeRN/` (gitignored). After you change shared iOS
bridge sources, refresh and reinstall:

```bash
bash bindings/flutter/offline_protocol/ios/sync_bridge_rn.sh
cd your_app/ios && pod install
```

## Quick start

Subscribe to events **before** `createFromConfig` if you need every event or
gap recovery via native replay (same contract as React Native).

```dart
import 'package:offline_protocol/offline_protocol.dart';

final sub = OfflineProtocol.events.listen((e) {
  final type = e.decodeEvent()['type'];
  // message_received, diagnostics, …
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

final messageId = await OfflineProtocol.sendMessage(
  recipient: 'off1…',
  content: 'Hello',
);
```

UniFFI and transport work run on a dedicated engine thread; method-channel and
event-channel callbacks are delivered on the main isolate’s platform thread
([F2](../../docs/bridges/flutter.md#f2-engine-thread-and-main-thread-callbacks-k2-parity)).

## API surface

| Area | Dart API |
|------|----------|
| Lifecycle | `createFromConfig`, `initializeMlsWithSecureStorage`, `start`, `stop`, `destroy` |
| Identity | `localAddress` |
| Messaging | `sendMessage` |
| Events | `OfflineProtocol.events`, native `replayEvents` via `NativeEventPipeline` |
| Internet | `enableInternetTransport`, auto-enable after `start()` when `transports.internet` has `serverAddress` |
| BLE | Wired when `bleEnabled` is true; `enableTransport('ble', …)` |
| Wi‑Fi Direct / peer stream | `enableTransport('wifiDirect', …)`, auto-enable when configured |
| Config | Optional top-level `dors` and `relay` JSON (shared `InitialRuntimeConfigApplier`) |

**Not in the Flutter plugin yet:** Nostr, Reticulum, telemetry pipe, and the
React Native foreground mesh service. `createFromConfig` refuses a config with
`reticulumEnabled` or `nostrEnabled` set, rather than enabling a transport
nothing serves. Use React Native for those, or host their managers in the
plugin and take them off its exclude lists.

Configuration fields and transport behavior match the core and React Native;
see [Configuration](../../docs/configuration.md) and the
[React Native README](../react-native/README.md).

## Example app

**Android** (minimal Gradle tree is in the repository):

```bash
cd bindings/flutter/offline_protocol/example
flutter pub get
flutter run -d android
```

**iOS** (generate the Xcode `Runner` project once; not committed because Flutter
tooling versions differ):

```bash
bindings/flutter/scripts/bootstrap-example-platforms.sh
cd bindings/flutter/offline_protocol/example
flutter pub get
flutter run -d <device-or-simulator-id>   # flutter devices
```

The example uses a minimal config (Internet transport flag only, no relay or BLE
start), subscribes to events, and prints the device `off1…` address. Details:
[example/README.md](offline_protocol/example/README.md).

## Maintainers

- **CI:** GitHub Actions job `Flutter Binding` (`.github/workflows/ci.yml`):
  `flutter test`, `flutter analyze`, arm64 UniFFI into jniLibs, release AAR
  assembly, and `measure-plugin-aar-size.sh --check`. Path filters skip the
  native gate when only Dart or guard tests change; pushes to `main` run the
  full job. No CI job compiles the iOS plugin yet; the manual typecheck is in
  [docs/bridges/flutter.md](../../docs/bridges/flutter.md#what-ci-runs).
- **Version:** the podspec and the Gradle build read it from
  `bindings/react-native/package.json`. Bump `offline_protocol/pubspec.yaml`
  with it; the release refuses a tag the pubspec does not match.
- **AAR footprint:** [baselines/README.md](baselines/README.md).
- **Rust guards:** `crates/offline-protocol-uniffi/src/tests/flutter_bridge_guards.rs`.
