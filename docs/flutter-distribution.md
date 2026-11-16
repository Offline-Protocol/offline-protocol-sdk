# Flutter: pub.dev install path

App integrators should not need the Rust toolchain or `generate-bindings.sh` to
try the Flutter plugin: use the pub.dev package with prebuilt mobile libraries.

React Native teams should use `@offline-protocol/mesh-sdk` instead ([QUICKSTART.md](../QUICKSTART.md)).

## Recommended: `pub.dev`

When the release train has `FLUTTER_PUBLISH` enabled, each `vX.Y.Z` release
publishes the `offline_protocol` plugin with **prebuilt** Android `jniLibs` and
the iOS XCFramework staged into the package. Your app only adds the dependency
and declares permissions ([mobile-permissions.md](mobile-permissions.md)).

```bash
flutter pub add offline_protocol
```

Pin the same **version** as the SDK release you adopt: `X.Y.Z` for the release
tagged `vX.Y.Z`. The first release on pub.dev is the first one this applies to.

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
UniFFI artifacts are available, dry-runs `dart pub publish` on every release,
and publishes through pub.dev automated publishing (OIDC, no stored token) from
a pushed tag when `FLUTTER_PUBLISH` is on. The first version is uploaded by
hand. See [CONTRIBUTING.md](../CONTRIBUTING.md#cutting-a-release).

The tree holds the tracked files of the plugin and the bridge plus the staged
native libraries, never a copy of a working directory: anything a build or an
IDE left there would otherwise publish, and a pub.dev version is permanent.

The tree must be assembled outside any git checkout, and ships no `.gitignore`:
pub honours both, and either one silently drops the native libraries or the
iOS bridge from the archive. The script refuses the first and removes the
second, and `scripts/tests/test-assemble-flutter-pub-package.sh` dry-runs the
publish to prove the archive.
