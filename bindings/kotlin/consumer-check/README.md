# Android library consumer check

A minimal application that depends on `com.offlineprotocol:offline-protocol-sdk`
the way a native app does: one implementation dependency, a release build with
R8, and no other library. If the library's public API or its consumer rules
break, this stops compiling or R8 stops on a missing class.

`app/src/main/java/consumer/Use.kt` is the canonical set of imports: the engine
from `uniffi.offline_protocol`, the hand-written bridge types from
`com.offlineprotocol`. [docs/android-jni-and-imports.md](../../../docs/android-jni-and-imports.md)
explains the split, and the native library names for an application that copies
the `.so` files itself instead of using the AAR.

Build it after publishing the library to the staging repository (see
[bindings/kotlin/README.md](../README.md)):

```bash
gradle -p consumer-check -PVERSION_NAME=0.0.0 :app:assembleRelease
```
