# Android library consumer check

Minimal application that depends on `com.offlineprotocol:offline-protocol-sdk`
the way a native app would: one implementation dependency, release build with
R8, no extra libraries.

| Path | Role |
|------|------|
| `app/src/main/java/consumer/Use.kt` | Canonical engine vs bridge imports, storage, and C12 telemetry lifecycle stubs |
| `app/build.gradle.kts` | `checkJniLibraryNames` if you add manual `jniLibs/` (see below) |

JNI naming for hand-copied `.so` files:
[docs/android-jni-and-imports.md](../../../docs/android-jni-and-imports.md).

Build after publishing the library to the local Maven repo (see
[bindings/kotlin/README.md](../README.md)):

```bash
gradle -p consumer-check -PVERSION_NAME=0.0.0 :app:assembleRelease
```
