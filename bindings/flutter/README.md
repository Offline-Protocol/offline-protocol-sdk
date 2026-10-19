# Flutter bindings

Flutter plugin for the [Offline Protocol SDK](https://github.com/Offline-Protocol/offline-protocol-sdk).

Native bridge sources are shared with the React Native module under
`bindings/react-native/{ios,android}` ([ADR 0025](../../docs/adr/0025-native-packages-are-assembled-in-place.md)).

## PR1 scope (engine lifecycle)

`create`, MLS initialization with platform secure storage, `start`, `localAddress`,
and `destroy`. UniFFI calls run on a dedicated engine thread; Flutter method-channel
results are posted on the main queue (see [Kotlin bridge K2](../../docs/bridges/kotlin.md#k2-nothing-blocking-on-the-main-looper)).

## PR2 scope (events, messaging, Internet)

- **Events:** subscribe with `OfflineProtocol.events` **before** `create` / `start`
  if you need early diagnostics. Payloads use the same `eventJson` (+ optional
  `seq`) shape as React Native `OfflineProtocol_Event`.
- **Messaging:** `OfflineProtocol.sendMessage`.
- **Internet:** `OfflineProtocol.enableInternetTransport`, plus auto-enable after
  `start()` when `transports.internet` is enabled with a `serverAddress` (same as
  the React Native JS layer).

PR2 compiles a **narrow** native source set: UniFFI, config parser, secure storage,
Internet transport, and shared relay helpers. BLE, Wi‑Fi Direct, Nostr, and Reticulum
ship in PR3.

`createFromConfig` accepts the same optional `dors` and `relay` JSON sections as
the React Native bridge (applied via shared `InitialRuntimeConfigApplier` sources).

Shared Internet wiring lives in `InternetTransportBootstrap` (Kotlin/Swift) for
Flutter; the React Native module keeps its existing paths until a later refactor.

## CI

A Flutter CI job is planned (PR4 in the Flutter binding rollout). Merging this
plugin does not yet gate on `flutter build` in GitHub Actions.

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

final sub = OfflineProtocol.events.listen((e) {
  final type = e.decodeEvent()['type'];
  // handle message_received, diagnostics, …
});

await OfflineProtocol.createFromConfig({
  'appId': 'com.example.app',
  'profile': 'default',
  'internetEnabled': true,
  'transports': {
    'internet': {
      'enabled': true,
      'serverAddress': 'wss://relay.example.com',
    },
  },
});
await OfflineProtocol.initializeMlsWithSecureStorage();
await OfflineProtocol.start();

final id = await OfflineProtocol.sendMessage(
  recipient: 'off1…',
  content: 'Hello',
);
```

## License

AGPL-3.0-only, or commercial license. See repository root `LICENSE`.
