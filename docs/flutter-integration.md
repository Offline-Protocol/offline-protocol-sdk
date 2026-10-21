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
