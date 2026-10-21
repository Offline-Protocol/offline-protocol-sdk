# Flutter plugin size baselines

Release artifact sizes for CI and local checks against unexpected growth in
the Flutter plugin native surface.

| File | What it measures |
|------|------------------|
| `offline-protocol-release-aar.json` | `offline_protocol-release.aar` from `:offline_protocol:assembleRelease` |

The AAR includes shared bridge Kotlin and UniFFI `jniLibs` from
`bindings/react-native/android`. It measures the plugin native surface, not a
full example APK (which also includes the Flutter engine and app code).

## Update a baseline

After an intentional change to the plugin compile surface or UniFFI libraries:

```bash
bindings/flutter/scripts/measure-plugin-aar-size.sh --write-baseline
```

## Verify locally (same check as CI)

```bash
bindings/flutter/scripts/measure-plugin-aar-size.sh --check
```
