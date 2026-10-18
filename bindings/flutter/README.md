# Flutter bindings

Flutter plugin for the [Offline Protocol SDK](https://github.com/Offline-Protocol/offline-protocol-sdk).

Native bridge sources are shared with the React Native module under
`bindings/react-native/{ios,android}` ([ADR 0025](../../docs/adr/0025-native-packages-are-assembled-in-place.md)).

## PR1 scope

Engine lifecycle only: `create`, MLS initialization with platform secure storage,
`start`, `localAddress`, and `destroy`. Transport managers and event streaming
ship in follow-up PRs.

PR1 compiles a **minimal** native source set (UniFFI, config parser, secure storage,
initial DORS/relay applier). Transport Swift/Kotlin is added when those managers
are wired up.

`createFromConfig` accepts the same optional `dors` and `relay` JSON sections as
the React Native bridge (applied via shared `InitialRuntimeConfigApplier` sources).

UniFFI calls run on a dedicated engine thread; Flutter method-channel results are
posted on the main queue (see [Kotlin bridge K2](../../docs/bridges/kotlin.md#k2-nothing-blocking-on-the-main-looper)).

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
