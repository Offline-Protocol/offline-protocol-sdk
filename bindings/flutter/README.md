# Flutter bindings

Flutter plugin for the [Offline Protocol SDK](https://github.com/Offline-Protocol/offline-protocol-sdk): offline-first mesh messaging with MLS encryption on iOS and Android.

## Requirements

| Platform | Version |
|----------|---------|
| Flutter | >= 3.10 |
| iOS | >= 13.0 |
| Android | API 24+ (Android 7.0) |

## Add the plugin

**From pub.dev** (when published for your release):

```yaml
dependencies:
  offline_protocol: ^0.30.0   # match your SDK release
```

Published packages include prebuilt native libraries. See [Flutter integration](../../docs/flutter-integration.md) for permissions and platform setup.

**From this repository** (development):

```yaml
dependencies:
  offline_protocol:
    path: ../bindings/flutter/offline_protocol   # adjust for your layout
```

Build UniFFI mobile artifacts once, then stage them into the plugin:

```bash
./scripts/generate-bindings.sh
cd bindings/react-native && npm run build:uniffi:all
bash bindings/flutter/scripts/stage-plugin-native-artifacts.sh
```

## Platform setup

Use the same permissions and manifests as React Native:

- **iOS:** Bluetooth usage descriptions and `UIBackgroundModes` (`bluetooth-central`, `bluetooth-peripheral`) before BLE starts. See [React Native iOS permissions](../react-native/README.md#ios-permissions).
- **Android:** Runtime Bluetooth and (where applicable) location / nearby Wi‑Fi permissions. See [React Native Android permissions](../react-native/README.md#android-permissions).

**iOS:** After upgrading the plugin, run `pod install` in your app’s `ios/` directory.

## Quick start

Subscribe to events **before** `createFromConfig` if you need every event or gap recovery via native replay.

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

Engine work runs on a dedicated native thread; method-channel and event-channel callbacks arrive on the platform main thread.

## API surface

| Area | Dart API |
|------|----------|
| Lifecycle | `createFromConfig`, `initializeMlsWithSecureStorage`, `start`, `stop`, `destroy` |
| Identity | `localAddress` |
| Messaging | `sendMessage` |
| Events | `OfflineProtocol.events` (sequence numbers and `replayEvents` when you subscribe early) |
| Full native surface | `MeshSdk` (parity with React Native native methods) |

Internet, BLE, Wi‑Fi Direct, Nostr, and Reticulum follow the same configuration model as React Native. Optional top-level `dors` and `relay` JSON use the shared runtime config applier.

**Android:** `MeshForegroundService` keeps mesh work alive while the engine is running; the notification Stop action matches React Native teardown.

**Not in Flutter:** React Native `MeshHeadlessWakeService` (opt-in wake after process kill). Use React Native if you need that behavior.

For every config field and transport, see [Configuration](../../docs/configuration.md) and the [React Native README](../react-native/README.md).

## Example app

**Android:**

```bash
cd bindings/flutter/offline_protocol/example
flutter pub get
flutter run -d android
```

**iOS** (generate the Xcode project once):

```bash
bindings/flutter/scripts/bootstrap-example-platforms.sh
cd bindings/flutter/offline_protocol/example
flutter pub get
flutter run -d <device-or-simulator-id>
```

The example uses a minimal config, listens for events, and prints the device `off1…` address. See [example/README.md](offline_protocol/example/README.md).

## Contributors

Bridge layout, CI, and generator rules: [docs/bridges/flutter.md](../../docs/bridges/flutter.md).
