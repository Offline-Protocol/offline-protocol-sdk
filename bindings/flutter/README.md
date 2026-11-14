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

**App integrators:** use the release on pub.dev when available. See
[docs/flutter-distribution.md](../../docs/flutter-distribution.md).

```bash
flutter pub add offline_protocol
```

**SDK contributors** use a path dependency and must build UniFFI natives first:

```yaml
dependencies:
  offline_protocol:
    path: ../bindings/flutter/offline_protocol   # adjust for your layout
```

## Platform setup

**Canonical guide:** [mobile-permissions.md](../../docs/mobile-permissions.md)
(Info.plist, manifest, grant timing before `start()`, background mesh on Android).

- **iOS:** the same `Info.plist` rules as React Native: usage descriptions,
  `UIBackgroundModes`, and local network keys when needed.
- **Android:** unlike React Native, the plugin's manifest declares only
  `FOREGROUND_SERVICE`, `FOREGROUND_SERVICE_CONNECTED_DEVICE`,
  `CHANGE_WIFI_STATE` and its foreground service. Declare the Bluetooth,
  location (`maxSdkVersion="32"`), Wi‑Fi Direct, LAN and `INTERNET`
  permissions in `android/app/src/main/AndroidManifest.xml` yourself; the
  exact block is in [mobile-permissions.md § Flutter](../../docs/mobile-permissions.md#flutter).
  Then request Bluetooth, and nearby Wi‑Fi or location by API level, at
  runtime before `start()`; the plugin does not prompt. Add
  `ACCESS_LOCAL_NETWORK` to the **app** manifest when targeting API 37+.

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
| Events | `OfflineProtocol.events` (sequence numbers and `replayEvents` when you subscribe early) |
| Full native surface | `MeshSdk` (parity with React Native native methods) |

Internet, BLE, Wi-Fi Direct, Nostr and Reticulum follow the same configuration model as React Native. Optional top-level `dors` and `relay` JSON use the shared runtime config applier.

**Android:** `MeshForegroundService` keeps mesh work alive while the engine is running; the notification Stop action matches React Native teardown.

**Not in Flutter:** React Native `MeshHeadlessWakeService` (opt-in wake after process kill). Use React Native if you need that behavior.

For every config field and transport, see [Configuration](../../docs/configuration.md) and the [React Native README](../react-native/README.md).

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
  full job. `Flutter Binding (iOS bridge)` typechecks the iOS plugin with the
  shared bridge Swift
  ([docs/bridges/flutter.md](../../docs/bridges/flutter.md#what-ci-runs)).
- **Version:** the podspec and the Gradle build read it from
  `bindings/react-native/package.json`. Bump `offline_protocol/pubspec.yaml`
  with it; the release refuses a tag the pubspec does not match.
- **AAR footprint:** [baselines/README.md](baselines/README.md).
- **Rust guards:** `crates/offline-protocol-uniffi/src/tests/flutter_bridge_guards.rs`.
