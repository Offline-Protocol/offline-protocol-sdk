# Flutter: pub.dev install path

Use this path when you want the Flutter plugin from pub.dev without building
the Rust bridge from a full SDK checkout.

React Native teams should use `@offline-protocol/mesh-sdk` instead ([QUICKSTART.md](../QUICKSTART.md)).

## Recommended: `pub.dev`

When the release train has `FLUTTER_PUBLISH` enabled, each `vX.Y.Z` release
publishes the `offline_protocol` plugin with **prebuilt** Android `jniLibs` and
the iOS XCFramework staged into the package. Your app only adds the dependency
and declares permissions ([mobile-permissions.md](mobile-permissions.md)).

```bash
flutter pub add offline_protocol
```

Pin the same **version** as the SDK release you adopt (`0.33.0` with tag `v0.33.0`).

### Minimal wiring

Same contract as [bindings/flutter/README.md](../bindings/flutter/README.md):

1. Subscribe to `OfflineProtocol.events` before `createFromConfig` when you need replay.
2. Build config JSON (or use documented presets from the RN guides).
3. `initializeMlsWithSecureStorage()` then `start()`.

## Path dependency (SDK contributors)

While iterating on the monorepo, depend on the plugin by path and build UniFFI
natives once:

```bash
./scripts/generate-bindings.sh
cd bindings/react-native && npm run build:uniffi:all
bash bindings/flutter/scripts/stage-plugin-native-artifacts.sh
```

See [docs/bridges/flutter.md](bridges/flutter.md) for CI and bridge rules.

## Upgrading

Bump the `offline_protocol` version in `pubspec.yaml` to match the SDK release.
You do **not** rebuild Rust locally unless you develop the plugin from source.

## Release mechanics (maintainers)

`scripts/assemble-flutter-pub-package.sh` copies shared bridge sources, staged
natives, and legal files into a publish tree. Release CI runs it after the mobile
UniFFI artifacts are available, then `dart pub publish` when `FLUTTER_PUBLISH` is
on. See [CONTRIBUTING.md](../CONTRIBUTING.md).
