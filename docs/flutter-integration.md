# Flutter integration

How to add the Offline Protocol Flutter plugin to an app.

## Dependency

**pub.dev** (recommended for apps):

```yaml
dependencies:
  offline_protocol: ^0.30.0   # use the version that matches your SDK release
```

**Monorepo path** (SDK development):

```yaml
dependencies:
  offline_protocol:
    path: ../bindings/flutter/offline_protocol
```

Build and stage native libraries as described in [bindings/flutter/README.md](../bindings/flutter/README.md).

## Platform setup

Permissions, Gradle/CocoaPods expectations, and MLS secure storage match React Native. Follow [bindings/flutter/README.md](../bindings/flutter/README.md#platform-setup) and the linked React Native permission sections for iOS and Android.

SDK levels and native dependency versions are taken from the React Native module build (one source of truth in the monorepo).

## Native libraries

Release packages on pub.dev ship with UniFFI libraries already inside the plugin (`jniLibs` on Android, XCFramework on iOS when staged). You do not need Rust or Node installed to consume a published version.

When building the plugin from source, run `bindings/flutter/scripts/stage-plugin-native-artifacts.sh` after `npm run build:uniffi:all` so local Gradle and CocoaPods pick up the artifacts.

## Using the API

- **`OfflineProtocol`:** lifecycle, events, `sendMessage`, and common helpers.
- **`MeshSdk`:** full method-channel surface (same operations as React Native’s native module).

Subscribe to `OfflineProtocol.events` before `createFromConfig` if you need contiguous sequence numbers or gap replay. For telemetry shutdown, prefer `MeshSdk.disableTelemetryAndWait`, `endTelemetrySessionAndWait`, or `flushTelemetryAndWait` so `await` matches native flush on the engine thread.

Details on events, transports, and config: [bindings/flutter/README.md](../bindings/flutter/README.md) and [Configuration](configuration.md).

## Publishing (SDK maintainers)

Release workflow stages natives and checks version alignment with the git tag. See the repository release documentation and `bindings/flutter/scripts/publish-to-pub.sh` (dry-run helper).
