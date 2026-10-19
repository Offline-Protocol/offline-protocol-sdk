# The Offline Protocol SDK from Java

A typed Java API for the JVM on desktop and server, generated from the same
interface definition as the Swift, Kotlin and Python bindings by
[`uniffi-bindgen-java`](https://github.com/IronCoreLabs/uniffi-bindgen-java).
It calls the native library directly through the Java foreign function API
(final in Java 22): there is no JNA, no JNI and no native glue of its own, and
it does not go through the C API.

The package is `com.offlineprotocol.sdk`. `OfflineProtocol` is the engine,
`MeshServices` and `DataStore` hang off it, every record and enum of the
definition is a class, and the things a host implements are interfaces:
`EventCallback`, the transport callbacks, and the two storage providers. An
engine refusal is a subclass of `ProtocolException` named for the variant
(`ProtocolException.EncryptFailed`).

The contract is [docs/bridges/java.md](../../docs/bridges/java.md), on top of
the [shared one](../../docs/bridges/README.md). Read it before writing a
callback or a storage provider.

## What you need

| What | Where it comes from |
|---|---|
| JDK 22 or later | Yours. The generated code uses `java.lang.foreign` |
| This library's sources | [`src/main/java`](src/main/java), built with Gradle (below) |
| `liboffline_protocol_uniffi.so` / `.dylib` / `offline_protocol_uniffi.dll` | The release's desktop archives, or `cargo build --release -p offline-protocol-uniffi` |

Run with `--enable-native-access=ALL-UNNAMED` (or the name of your module), or
the JVM warns on the first native call and a future release refuses it.

The library is found through `java.library.path`. If it is somewhere else, name
the file:

```sh
java --enable-native-access=ALL-UNNAMED \
     -Duniffi.component.offline_protocol.libraryOverride=/path/to/liboffline_protocol_uniffi.dylib ...
```

## Quick start

```java
import com.offlineprotocol.sdk.*;

// The generated record has no defaults: every field is spelled out, in the
// order of the definition (ProtocolConfig in offline_protocol.udl). Optional
// fields (mesh relay, custody) take null.
ProtocolConfig config = new ProtocolConfig(
    "com.example.app", "default",
    false, false, true, false, false, false, // ble, wifi direct, internet, reticulum, nostr, prefer online
    (byte) 8, true, true, true, true,        // ttl, encryption, auto key exchange, store pending, require encryption
    64L, 4096L, 60_000L, OverflowPolicy.DROP_OLDEST,
    false, 256, true, true, false, false,
    true, true, true, false, true, true, true,
    null, null,
    true, true, true);

try (OfflineProtocol engine = new OfflineProtocol(config)) {
    // Storage before start(). The store key seals key material at rest;
    // keep it out of the source.
    engine.initializeMlsWithFileStores("/var/lib/app/mls", "/var/lib/app/state", storeKey32Bytes);

    engine.setEventCallback(json -> queue.add(json)); // copy and return; never call the engine from here
    engine.start();
    System.out.println("this device is " + engine.localAddress());

    try {
        engine.sendMessage("off1...", "hi", MessagePriority.MEDIUM, null);
    } catch (ProtocolException.EncryptFailed e) {
        // each engine refusal is its own exception type
    }
    engine.stop();
}
```

`OfflineProtocolLib` holds the functions that need no engine
(`deriveAddress`, `parseInvite`, `verifyIdentityAssertion`,
`runStorageConformance`).

## Events

`setEventCallback` installs an `EventCallback`, or `pollEvent` takes the oldest
queued event; every event is queued either way. An event is JSON text, unchanged
from the engine, so parse it with the library you already use. The callback runs
on whichever thread is inside the engine and must not call back into it.

## Transports and storage

Your process is the platform, so you own the radios and sockets: implement the
`BleTransportCallback`, `WifiDirectTransportCallback`,
`ReticulumTransportCallback` and `NostrTransportCallback` notifications to learn
when a queue has frames, and drain them with the `ble*`, `wifiDirect*`,
`internet*`, `reticulum*` and `nostr*` methods.

For storage, implement `MlsStorageProvider` (key material, back it with a
keystore) and `ProtocolStateStorageProvider`, and pass both to
`initializeMls`. Prove an adapter with
`OfflineProtocolLib.runStorageConformance`. Or use the built-in file stores, as
above.

## Installing

```kotlin
// build.gradle.kts
dependencies {
    implementation("com.offlineprotocol:offline-protocol-java:<version>")
}
```

The jar holds the generated Java and no native code. Download the native
library for your platform from the release's assets, and give the JVM its path:
`-Djava.library.path=<directory>`, or
`-Duniffi.component.offline_protocol.libraryOverride=<file>` for an absolute
path. Run on JDK 22 or later with `--enable-native-access=ALL-UNNAMED`. On
Android use the AAR (`com.offlineprotocol:offline-protocol-sdk`) instead.

## Telemetry

Pass `TelemetryConfig.os` and `osMajor` as the platform you actually run on:
unlike Swift and Kotlin, there is no hand-written layer here to fill them. See
[docs/bridges/java.md](../../docs/bridges/java.md#j3-the-host-fills-the-telemetry-platform).

## Building and testing

The sources under `src/main/java` are generated and committed. To regenerate
them, with the other bindings:

```sh
cargo install uniffi --version 0.32.2 --features cli --locked
cargo install uniffi-bindgen-java --version 0.5.2 --locked
./scripts/generate-bindings.sh
```

To build and test (JDK 22+, Gradle 9):

```sh
cargo build -p offline-protocol-uniffi
cd bindings/java
gradle build                              # -PnativeLibrary=/path/to/the/library for another build
```

`build` compiles the generated sources, runs the consumer check against the
library, and writes the binary, sources and Javadoc jars to `build/libs`.
`publishAllPublicationsToStagingRepository` writes the Maven repository the
release signs and uploads, to `build/maven`.

## Licensing

Dual-licensed: [AGPL-3.0-only](../../LICENSE) for open-source use, or a
[commercial license](../../LICENSE-COMMERCIAL.md) for proprietary use. The
library contains cryptography; see [EXPORT.md](../../EXPORT.md).
