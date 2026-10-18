# Flutter bindings

Flutter plugin for the [Offline Protocol SDK](https://github.com/Offline-Protocol/offline-protocol-sdk).

Native bridge sources are shared with the React Native module under
`bindings/react-native/{ios,android}` ([ADR 0025](../../docs/adr/0025-native-packages-are-assembled-in-place.md)).

## PR1 scope

Engine lifecycle only: `create`, MLS initialization with platform secure storage,
`start`, `localAddress`, and `destroy`. Transport managers and event streaming
ship in follow-up PRs.

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

```bash
cd bindings/flutter/offline_protocol/example
flutter pub get
flutter run
```

The example boots the engine with all transports disabled and prints this
device's `off1…` address.

## Add to your app

```yaml
dependencies:
  offline_protocol:
    path: ../bindings/flutter/offline_protocol
```

```dart
import 'package:offline_protocol/offline_protocol.dart';

final address = await OfflineProtocol.bootstrapSmokeTest(
  appId: 'com.example.app',
  profile: 'default',
);
```

## License

AGPL-3.0-only, or commercial license. See repository root `LICENSE`.
