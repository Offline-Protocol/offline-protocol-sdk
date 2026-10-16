# The Offline Protocol SDK from Java

A Java wrapper over the [C API](../c/README.md), for the JVM on desktop and
server (Linux, macOS, Windows; 64-bit; Java 11 or later). It is the
[local API](../../docs/spec/local-api.md) called in process: you create an
engine from a JSON configuration, call any method of the interface definition
by name with JSON parameters, and receive events as JSON. It has no native
code of its own: it loads, through [JNA](https://github.com/java-native-access/jna),
the library every desktop release already ships.

It does not parse JSON. Every parameter and result is text, so use the JSON
library you already have. Because nothing in the signatures depends on a
method, a method added to the interface definition is callable from Java with
no change here.

The contract, including ownership, threading and what a listener must not do,
is [docs/bridges/java.md](../../docs/bridges/java.md) and, underneath it,
[docs/bridges/c.md](../../docs/bridges/c.md). Read both before writing a
listener or a storage provider.

## What you need

| What | Where it comes from |
|---|---|
| This library | Built from `bindings/java` (below) |
| `liboffline_protocol_uniffi.so` / `.dylib` / `offline_protocol_uniffi.dll` | The release's desktop archives, or `cargo build --release -p offline-protocol-uniffi` |
| JNA 5.14 or later | A dependency of this library |

Point the JVM at the native library one of two ways:

- `-Djna.library.path=<dir>` (or `java.library.path`), which is JNA's search
  for `offline_protocol_uniffi`, or
- `-Doffline_protocol.library=<full path to the file>`.

## Quick start

```java
import com.offlineprotocol.sdk.OfflineProtocol;
import com.offlineprotocol.sdk.OfflineProtocolException;

String config = "{\"app_id\":\"com.example.app\",\"profile\":\"default\","
    + "\"ble_enabled\":false,\"wifi_direct_enabled\":false,\"internet_enabled\":true,"
    + "\"reticulum_enabled\":false,\"nostr_enabled\":false,\"prefer_online\":false,"
    + "\"initial_ttl\":8,\"encryption_enabled\":true,\"auto_key_exchange\":true,"
    + "\"store_pending\":true,\"max_pending_per_peer\":64,\"max_pending_global\":4096,"
    + "\"pending_ttl_ms\":60000,\"overflow_policy\":\"DropOldest\"}";

try (OfflineProtocol engine = OfflineProtocol.create(config)) {
    // Storage before start(). The store key seals key material at rest;
    // keep it out of the source.
    engine.call("initialize_mls_with_file_stores",
        "{\"mls_root\":\"/var/lib/app/mls\",\"state_root\":\"/var/lib/app/state\","
        + "\"store_key\":\"<32 bytes, base64>\"}");

    engine.setEventListener(json -> queue.add(json)); // copy and return; never call the engine from here
    engine.call("start", null);
    System.out.println("this device is " + engine.call("local_address", null));

    try {
        engine.call("send_message",
            "{\"recipient\":\"off1...\",\"content\":\"hi\",\"priority\":\"Medium\"}");
    } catch (OfflineProtocolException e) {
        // An engine refusal names the definition's variant and its position.
        System.err.println(e.status() + " " + e.variant() + " (" + e.code() + ") " + e.getMessage());
    }
    engine.call("stop", null);
}
```

## Calling methods

Names, parameters and the JSON form of every type are the local API's, so its
[method table](../../docs/spec/local-api.md#method-table) is the reference:

- Engine methods are bare (`send_message`), `MeshServices` methods carry
  `services.`, `DataStore` methods carry `data.`.
- Parameters are a JSON object keyed by the definition's parameter names, or
  `null`. Bytes are padded standard base64. An omitted dictionary field takes
  the definition's default. An unknown key is refused.
- The result is JSON text. A method that returns nothing returns `"null"`.
- The instance-less functions (`derive_address`, `parse_invite`,
  `verify_identity_assertion`) are `OfflineProtocol.callStatic`.

Your process is the platform, so the transport methods (`ble_`, `internet_`,
`wifi_direct_`, `reticulum_`, `nostr_`) are reachable, and
`setTransportListener` says when one has frames to drain.

## Events

`setEventListener` installs a listener, or `pollEvent` takes the oldest queued
event; every event is queued either way. A listener runs on whichever thread is
inside the engine and must not call back into it.

## Storage

Pass your own with `initializeMls(secure, protocolState)` (implement
`StorageProvider`) and prove it with
`OfflineProtocol.runStorageConformance`, or use the built-in file stores as in
the quick start.

## Building and testing

```sh
cargo build -p offline-protocol-uniffi
cd bindings/java
gradle build                                  # JDK 11+, Gradle 8.9+
gradle build -PnativeLibraryDir=/path/to/lib  # a release build, say
```

`build` compiles with `-Xlint:all -Werror`, runs the consumer check against
the library, and writes the binary, sources and Javadoc jars to `build/libs`.

## Licensing

Dual-licensed: [AGPL-3.0-only](../../LICENSE) for open-source use, or a
[commercial license](../../LICENSE-COMMERCIAL.md) for proprietary use. The
library contains cryptography; see [EXPORT.md](../../EXPORT.md).
