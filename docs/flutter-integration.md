# Flutter integration

How to add the Offline Protocol Flutter plugin to an app and ship it on pub.dev.

## Add the dependency

Path dependency while developing in the monorepo:

```yaml
dependencies:
  offline_protocol:
    path: ../bindings/flutter/offline_protocol
```

For pub.dev releases:

```yaml
dependencies:
  offline_protocol: ^0.30.0 # match the workspace release tag
```

## Platform setup

Follow [bindings/flutter/README.md](../bindings/flutter/README.md) for Android Gradle,
iOS CocoaPods, permissions, and MLS secure storage requirements. The plugin reads SDK
levels and dependency versions from the React Native module (ADR 0025 decision 5).

## Native artifacts

Release builds stage UniFFI libraries into the plugin tree via
`bindings/flutter/scripts/stage-plugin-native-artifacts.sh` so consumers are not required
to clone Rust or run `npm run build:uniffi`. The Gradle build prefers
`android/src/main/jniLibs` when present; the podspec prefers
`ios/Frameworks/offline_protocol_uniffi.xcframework` when present, otherwise the React
Native tree.

## API surface

Use `OfflineProtocol` for lifecycle, events, and common helpers. Use `MeshSdk` for the
full method-channel surface pinned in `method_channel_methods.txt` (React Native parity).

### Events and one-shots

Subscribe to `OfflineProtocol.events` before `createFromConfig` if you need the full
stream and sequence replay. **`internet_session_superseded`**: Android sticky-delivers
when Dart is not listening; iOS re-emits on app foreground while the latch is set.
**`mesh_stopped_by_user`**: Android only (mesh notification Stop). See
[F7b](bridges/flutter.md#f7b-one-shot-events-internet_session_superseded-mesh_stopped_by_user).

### Telemetry and engine polling

`MeshSdk.flushTelemetry`, `disableTelemetry`, and `endTelemetrySession` run on the
native engine thread and may block for a few seconds; other channel calls queue
behind them. Use `flushTelemetryAndWait` (and the other `*AndWait` helpers) at
shutdown boundaries; avoid tight UI polling loops (`getStateTyped`,
`internetIsReadyTyped`, …) while a non-awaited flush is in flight. Details: [F8](bridges/flutter.md#f8-telemetry).

## Publish to pub.dev (maintainers)

On a release tag, CI stages UniFFI libraries into the plugin tree
(`bindings/flutter/scripts/stage-plugin-native-artifacts.sh`). To publish:

1. Match `bindings/flutter/offline_protocol/pubspec.yaml` `version:` to the git tag.
2. Stage natives (or use CI artifacts from the release job).
3. From the plugin directory: `dart pub publish --dry-run`, then `dart pub publish`.

Helper: `bash bindings/flutter/scripts/publish-to-pub.sh` (dry-run only).
Requires a [pub.dev publisher](https://dart.dev/tools/pub/publishing) for the package name.
