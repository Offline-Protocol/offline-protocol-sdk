# Android native JNI naming and imports

Two footguns on the **manual** Android path (Maven coordinates or hand-copied
`jniLibs`): wrong `.so` names and mixing React Native bridge types with the
engine API.

## JNI library names (#15)

| Stage | File name |
|-------|-----------|
| `cargo build` output | `liboffline_protocol_uniffi.so` |
| What UniFFI loads at runtime | `libuniffi_offline_protocol.so` |

Copy or rename every ABI into `app/src/main/jniLibs/<abi>/` as
**`libuniffi_offline_protocol.so`**. The published AAR already uses the loaded
name; hand-copying cargo output without renaming fails at runtime with an
obscure unload error.

Constants (also in the shipped library): **`OfflineProtocolJniLibrary.LOADED_BASENAME`**
and **`CARGO_OUTPUT_BASENAME`**.

The React Native build script
[`build-uniffi-android.sh`](../bindings/react-native/scripts/build-uniffi-android.sh)
renames automatically. Manual integrators can validate a tree with:

```bash
python3 scripts/check_manual_android_jni_libs.py path/to/jniLibs
```

`mesh-sdk-doctor` flags the stale cargo name under an RN app's
`android/app/src/main/jniLibs` (see [react-native-host-doctor.md](react-native-host-doctor.md)).

## Kotlin imports (#16)

| You need | Package | Examples |
|----------|---------|----------|
| Engine (UDL / UniFFI) | `uniffi.offline_protocol` | `OfflineProtocol`, `ProtocolConfig`, `EventCallback` |
| Bridge helpers & transports | `com.offlineprotocol` | `OfflineProtocolReferenceStorage`, `InternetManager`, `StorageConformance` |
| React Native only (not in the Maven library) | `com.offlineprotocol` module classes | `OfflineProtocolModule` |

**Rule:** Implement the engine with `uniffi.offline_protocol.*`. Import
`com.offlineprotocol.*` only for hand-written bridge types the Android library
ships. Do not import or call `OfflineProtocolModule` on the native path.

The canonical compile-checked sample:
[`bindings/kotlin/consumer-check/app/src/main/java/consumer/Use.kt`](../bindings/kotlin/consumer-check/app/src/main/java/consumer/Use.kt).

Maven users depend on `com.offlineprotocol:offline-protocol-sdk` only; see
[bindings/kotlin/README.md](../bindings/kotlin/README.md). JNI layout matters
when copying `.so` files yourself instead of the AAR.

See also [android-integration.md](android-integration.md) and
[integrator-map.md](integrator-map.md).
