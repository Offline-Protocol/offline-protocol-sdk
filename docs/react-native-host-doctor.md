# React Native host doctor

`mesh-sdk-doctor` reads your app's files and reports the setups that stop this
SDK loading: Expo Go, and integration leftovers from before 0.20.0. Run it
before spending a session on a "native module not linked" error.

## Run it

From your **app root** (where `package.json` and `ios/` / `android/` live), after
`npm install @offline-protocol/mesh-sdk`:

```bash
npx --no-install mesh-sdk-doctor
npx --no-install mesh-sdk-doctor --root path/to/app   # from anywhere else
```

Always pass `--no-install`. Without it, running the command where the SDK is
not installed makes npx fetch whatever the public npm registry holds under that
name, which is not this package.

| Exit code | Meaning |
|-----------|---------|
| 0 | No findings, or warnings only |
| 1 | At least one **ERROR**: the app cannot load the SDK as written |
| 2 | Bad arguments |

The doctor reads files only. It runs no CocoaPods, Gradle or app code.

## What it checks

An **error** is something the files prove is broken. Anything the files cannot
prove, such as how you launch the app, is a **warning**.

| Code | Severity | Meaning |
|------|----------|---------|
| `legacy-meshsdk-pod` | error | A live `pod 'MeshSdk'` line in `ios/Podfile` (commented lines are ignored) |
| `legacy-uniffi-archives` | error | `offline_protocol_uniffi_sim` / `_device` in the Podfile; those archives no longer exist |
| `no-package-json`, `package-json-invalid` | error | No readable `package.json` at the root |
| `expo-without-dev-client` | warn | `expo` without `expo-dev-client`. Native builds (`npx expo run:ios`) work; Expo Go does not |
| `legacy-linker-hacks` | warn | Podfile code sets `OTHER_LDFLAGS`, `LIBRARY_SEARCH_PATHS`, `SWIFT_INCLUDE_PATHS` or `DEFINES_MODULE` within a few lines of a `MeshSdk` mention |
| `android-stale-jnilib` | warn | A hand-copied `liboffline_protocol_uniffi.so` under `android/app/src/main/jniLibs`. The package ships `libuniffi_offline_protocol.so`, and nothing loads the old name |
| `mesh-sdk-missing` | warn | `@offline-protocol/mesh-sdk` is not in `package.json` |

Each iOS fix is described in
[UPGRADING.md §12.1](UPGRADING.md#121-react-native-ios-delete-your-manual-pod-meshsdk-line-v0200).

## Expo Go vs a development build

**Expo Go cannot load this SDK**, because it runs only the native modules it
was built with. Use one of:

- a **development build**: `expo-dev-client`, then `eas build` or `npx expo run:ios`
- a **native build** without the dev client: `npx expo prebuild` then `npx expo run:ios` / `run:android`
- **bare React Native**: `npm install`, then `pod install`

If the native module is missing at runtime, the linking error the SDK throws
adds a line naming Expo Go when `expo-constants` reports it
(`executionEnvironment === 'storeClient'`, or `appOwnership === 'expo'` on older
Expo SDKs).

## Related

- [Integrator map](integrator-map.md#react-native-host-compatibility)
- [Mobile permissions](mobile-permissions.md)
- [React Native README](../bindings/react-native/README.md)
