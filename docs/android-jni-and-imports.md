# Android: native library names and Kotlin imports

Two things go wrong for a native Android application that does not use React
Native: a native library copied under the wrong name, and an import from the
wrong package. Both compile, so both fail late.

An application on the Maven library (`com.offlineprotocol:offline-protocol-sdk`)
only needs the [imports](#kotlin-imports): the AAR already carries the native
library under the right name. The [names](#native-library-names) matter only to
an application that builds the Rust library itself and copies the `.so` files
into its own `jniLibs`.

## Native library names

The generated Kotlin opens exactly one name, and it is not the name cargo
writes:

| Stage | File name |
|-------|-----------|
| What `cargo build` writes | `liboffline_protocol_uniffi.so` |
| What the generated Kotlin loads | `libuniffi_offline_protocol.so` |

Copy each ABI's output to `app/src/main/jniLibs/<abi>/libuniffi_offline_protocol.so`.
A copy that keeps the cargo name builds and installs, and fails at the first
engine call with:

```
java.lang.UnsatisfiedLinkError: Unable to load library 'uniffi_offline_protocol'
```

Every ABI the application ships needs the file. An ABI folder that holds other
native libraries but not this one crashes on that ABI only, which is usually
the device nobody tested on.

The library exposes both names as `OfflineProtocolJniLibrary.LOADED_BASENAME`
and `OfflineProtocolJniLibrary.CARGO_OUTPUT_BASENAME`. React Native's
[`build-uniffi-android.sh`](../bindings/react-native/scripts/build-uniffi-android.sh)
renames on copy, and `mesh-sdk-doctor` warns about the cargo name under a React
Native app's `android/app/src/main/jniLibs` (see
[react-native-host-doctor.md](react-native-host-doctor.md)).

Check a hand-copied tree with:

```bash
python3 scripts/check_manual_android_jni_libs.py app/src/main/jniLibs
```

It fails on the cargo name anywhere in the tree, on an ABI folder without the
loaded name, and on a path that is not a directory. The application's other
native libraries (`libc++_shared.so`, a dependency's own) are left alone.

## Kotlin imports

| You need | Package | Examples |
|----------|---------|----------|
| The engine (generated from the UDL) | `uniffi.offline_protocol` | `OfflineProtocol`, `ProtocolConfig`, `EventCallback` |
| The hand-written bridge types the library ships | `com.offlineprotocol` | `OfflineProtocolReferenceStorage`, `InternetManager`, `StorageConformance` |
| React Native only, not in the library | `com.offlineprotocol` | `OfflineProtocolModule` |

Drive the engine through `uniffi.offline_protocol`. Import `com.offlineprotocol`
types by name, never with a wildcard: the package also holds the React Native
module, which the library leaves out because it needs React, so an import of
`OfflineProtocolModule` does not compile against the library.

[`consumer/Use.kt`](../bindings/kotlin/consumer-check/app/src/main/java/consumer/Use.kt)
in the consumer check is the canonical set of imports. CI compiles it against
the published library in a minified release build, so it cannot drift from what
the library ships.

See also [android-integration.md](android-integration.md),
[bindings/kotlin/README.md](../bindings/kotlin/README.md) and
[integrator-map.md](integrator-map.md).
