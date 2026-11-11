# React Native host doctor

G5: catch **Expo Go** and **legacy iOS/Android integration** leftovers before
you spend a session on native link failures.

## Run it

From your **app root** (where `package.json` and `ios/` / `android/` live):

```bash
npx mesh-sdk-doctor
```

In the monorepo package:

```bash
cd bindings/react-native
node scripts/mesh-doctor.js --root /path/to/your/app
```

Exit code **1** when any **ERROR** is reported; **0** when clean or only
warnings.

## What it checks

| Code | Severity | Meaning |
|------|----------|---------|
| `expo-go-unsupported` | error | `expo` without `expo-dev-client` |
| `legacy-meshsdk-pod` | error | Manual `pod 'MeshSdk'` in Podfile |
| `legacy-uniffi-archives` | error | `offline_protocol_uniffi_sim` / `_device` linker flags |
| `legacy-linker-hacks` | warn | `post_install` tweaks `OTHER_LDFLAGS` / `LIBRARY_SEARCH_PATHS` for MeshSdk |
| `mesh-sdk-missing` | warn | Package not in `package.json` dependencies |
| `android-jni-name` | warn | Wrong `.so` name under `jniLibs` |

Mechanical fixes for iOS upgrades: [UPGRADING.md §12.1](UPGRADING.md#121-react-native-ios-delete-your-manual-pod-meshsdk-line-v0200).

## Expo Go vs development build

**Expo Go cannot load this SDK.** You need either:

- A **development build** (`expo-dev-client` + `eas build` / `npx expo run:ios`), or
- **Bare React Native** (`npm install` + `pod install`).

At runtime, if the native module is missing, the JS layer throws a linking error
that mentions Expo Go when `expo-constants` reports `appOwnership === 'expo'`.

## Related

- [integrator map](integrator-map.md) (RN path)
- [mobile-permissions.md](mobile-permissions.md)
- [bindings/react-native/README.md](../bindings/react-native/README.md#troubleshooting)
