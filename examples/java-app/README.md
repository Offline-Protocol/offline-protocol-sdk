# Java example

Two devices, Alice and Bob, in one JVM: they pair, establish an end-to-end
encrypted session and exchange two messages, using the
[Java binding](../../bindings/java/README.md).

```
alice off1q89jrev...
bob   off1q9jxmhy...
  ok: bob and alice are paired
  ok: bob received "hello bob" (encrypted end to end)
  ok: alice received "hello alice"
done
```

## What it shows

- Creating an engine from a `ProtocolConfig` (every field spelled out: the
  generated Java record has no defaults, see
  [J2](../../docs/bridges/java.md#j2-the-generated-records-have-no-defaults)).
- The engine's own encrypted file stores (`initializeMlsWithFileStores`), so no
  storage provider is needed.
- Pairing from events: the app reads `pollEvent()`, and answers
  `connection_request_received` and `connection_accepted` with the key package
  exchange.
- A transport's job, in a dozen lines: take what an engine wants to send
  (`internetGetNextMessage`) and hand it to the engine it is addressed to
  (`internetMessageReceived`). A real app does this over a radio or a socket.
  Everything else here, the pairing, the MLS session and the encryption, is the
  engine's own.

## Run it

You need JDK 22 or later and the native library.

```bash
cargo build -p offline-protocol-uniffi      # from the repository root
cd examples/java-app
gradle run                                   # Gradle 9; -PnativeLibrary=<file> for another library
```

`settings.gradle.kts` includes `bindings/java` as a composite build and
substitutes it for the published coordinate, so the example runs against the
source in this checkout. In your own project, drop that block and depend on a
release from Maven Central:

```kotlin
dependencies {
    implementation("com.offlineprotocol:offline-protocol-java:<version>")
}
```

Run on JDK 22 or later with `--enable-native-access=ALL-UNNAMED` and the
library's path in `-Duniffi.component.offline_protocol.libraryOverride=<file>`
(or on `java.library.path`); `build.gradle.kts` does both for `gradle run`.

## Where to go next

- Callbacks (`EventCallback`, transports, storage providers) run on the
  engine's thread and must not call back in:
  [J4](../../docs/bridges/java.md#j4-callbacks-run-on-the-engines-thread-and-must-not-call-back-in).
- The host fills the telemetry platform:
  [J3](../../docs/bridges/java.md#j3-the-host-fills-the-telemetry-platform).
