# Flutter binding size baselines

These files record **release artifact sizes** so PR4 CI (and local runs) can
detect unexpected growth in the Flutter plugin native surface.

| File | What it measures |
|------|------------------|
| `offline-protocol-release-aar.json` | `offline_protocol-release.aar` from `:offline_protocol:assembleRelease` |

The AAR includes shared bridge Kotlin and the UniFFI `jniLibs` copied from
`bindings/react-native/android`. It is a direct read on PR3 compile scope, not
the full example APK (which also pulls Flutter engine and app code).

## Update a baseline

After an intentional change to the plugin compile surface or UniFFI libraries:

```bash
bindings/flutter/scripts/measure-plugin-aar-size.sh --write-baseline
```

## Verify locally (same check as CI)

```bash
bindings/flutter/scripts/measure-plugin-aar-size.sh --check
```
