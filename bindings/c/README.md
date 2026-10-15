# The Offline Protocol SDK from C

A C API over the same native library the other bindings load. It is the
[local API](../../docs/spec/local-api.md) called in process: you create an
engine from a JSON configuration, call any method of the interface
definition by name with JSON parameters, and receive events as JSON. Any
language that calls C uses it the same way: C, C++, Go through cgo, C#
through P/Invoke, Zig.

The contract, including ownership, threading and what a callback must not
do, is [docs/bridges/c.md](../../docs/bridges/c.md). Read it before writing
a callback or a storage provider.

## What you need

| File | Where it comes from |
|---|---|
| `offline_protocol.h` | [`include/`](include/offline_protocol.h), and every desktop archive on a [GitHub release](https://github.com/Offline-Protocol/offline-protocol-sdk/releases) |
| The shared library | The release's desktop archives: `liboffline_protocol_uniffi.so` (Linux x86_64, aarch64), `liboffline_protocol_uniffi.dylib` (macOS arm64), `offline_protocol_uniffi.dll` with its import library `offline_protocol_uniffi.dll.lib` (Windows x86_64) |
| A JSON library | Yours. Every result and event is JSON text |

To build the library yourself, including the static archive the releases do
not ship (it is over ten times the size of the shared library):

```sh
cargo build --release -p offline-protocol-uniffi
# target/release/liboffline_protocol_uniffi.{so,dylib,a}
# (the static archive may land in target/release/deps/)
```

Linking the static archive needs the system libraries rustc names:

```sh
cargo rustc --release -p offline-protocol-uniffi --lib -- --print native-static-libs
```

The iOS XCFramework and the Android libraries in the same release carry the
same C functions; the same header applies.

## Quick start

```c
#include <stdio.h>
#include "offline_protocol.h"

int main(void) {
  OfflineProtocolHandle *engine = NULL;
  OfflineProtocolError err = {0};
  char *result = NULL;

  /* Every ProtocolConfig field without a default in the interface
   * definition; the rest take the definition's defaults. */
  const char *config =
      "{\"app_id\":\"com.example.app\",\"profile\":\"default\","
      "\"ble_enabled\":false,\"wifi_direct_enabled\":false,\"internet_enabled\":true,"
      "\"reticulum_enabled\":false,\"nostr_enabled\":false,\"prefer_online\":false,"
      "\"initial_ttl\":8,\"encryption_enabled\":true,\"auto_key_exchange\":true,"
      "\"store_pending\":true,\"max_pending_per_peer\":64,\"max_pending_global\":4096,"
      "\"pending_ttl_ms\":60000,\"overflow_policy\":\"DropOldest\"}";

  if (offline_protocol_create(config, &engine, &err) != OfflineProtocolStatus_Ok) {
    fprintf(stderr, "create: %s\n", err.message);
    offline_protocol_error_clear(&err);
    return 1;
  }

  /* Storage before start(): the built-in file stores need no callbacks.
   * The store key seals key material at rest; keep it out of the source. */
  if (offline_protocol_call(engine, "initialize_mls_with_file_stores",
          "{\"mls_root\":\"/var/lib/app/mls\",\"state_root\":\"/var/lib/app/state\","
          "\"store_key\":\"<32 bytes, base64>\"}", &result, &err) != OfflineProtocolStatus_Ok) {
    fprintf(stderr, "storage: %s\n", err.message);
    offline_protocol_error_clear(&err);
    offline_protocol_free(engine);
    return 1;
  }
  offline_protocol_string_free(result);

  offline_protocol_call(engine, "start", NULL, &result, &err);
  offline_protocol_string_free(result);

  if (offline_protocol_call(engine, "local_address", NULL, &result, &err) ==
      OfflineProtocolStatus_Ok) {
    printf("this device is %s\n", result); /* "off1..." as a JSON string */
    offline_protocol_string_free(result);
  }

  /* An engine refusal names the definition's variant and its position;
   * the other statuses are the binding's own (a bad parameter, say). */
  if (offline_protocol_call(engine, "send_message",
          "{\"recipient\":\"off1...\",\"content\":\"hi\",\"priority\":\"Medium\"}",
          &result, &err) == OfflineProtocolStatus_Ok) {
    printf("queued %s\n", result); /* the message id */
    offline_protocol_string_free(result);
  } else {
    fprintf(stderr, "send: %s (%d) %s\n", err.variant ? err.variant : "-", err.code, err.message);
    offline_protocol_error_clear(&err);
  }

  offline_protocol_call(engine, "stop", NULL, &result, &err);
  offline_protocol_string_free(result);
  offline_protocol_free(engine);
  return 0;
}
```

```sh
cc -I include app.c -L <libdir> -loffline_protocol_uniffi -Wl,-rpath,<libdir>
```

## Calling methods

The method names, the parameter names and the JSON form of every type are
the local API's, so its [method table](../../docs/spec/local-api.md#method-table)
is the reference for what you can call:

- Engine methods are bare (`send_message`), `MeshServices` methods carry
  `services.` (`services.register_service`), `DataStore` methods carry
  `data.` (`data.create_doc`).
- Parameters are a JSON object keyed by the definition's parameter names.
  `NULL` means no parameters.
- Bytes are padded standard base64. Enums are spelled as the definition
  spells them (`"WiFiDirect"`, `"Medium"`). An omitted dictionary field
  takes the definition's default. An unknown key is refused.
- A method that returns nothing returns `null`.

Unlike the local API, the transport methods are reachable: your process is
the platform here, and it owns the radios and sockets (`ble_`,
`internet_`, `wifi_direct_`, `reticulum_`, `nostr_`).

The declarations that take a callback have their own functions: events,
the four transport notifications, `initialize_mls` with your own storage,
and the storage conformance suite. The header documents each.

## Events

Either install a callback with `offline_protocol_set_event_callback`, or
poll with `offline_protocol_poll_event`; every event is queued either way.
A callback runs on whichever thread is inside the engine and must not call
back into it: copy the text and return.

## The consumer check

[`consumer-check/`](consumer-check/) is a complete program, with a storage
provider written in C, that CI compiles with `-std=c99 -Wpedantic -Werror`
and runs against the shared and the static library:

```sh
cargo build -p offline-protocol-uniffi
make -C bindings/c/consumer-check
```

## Other languages

A wrapper in another language binds these few functions and marshals JSON,
and keeps working as the interface definition grows, because nothing in the
C signatures changes when a method is added. For Go, there is also the
community generator `uniffi-bindgen-go`, whose v0.6.0 targets the UniFFI
version this SDK pins (0.30); it binds UniFFI's own ABI rather than this one.

## Licensing

Dual-licensed: [AGPL-3.0-only](../../LICENSE) for open-source use, or a
[commercial license](../../LICENSE-COMMERCIAL.md) for proprietary use. The
library contains cryptography; see [EXPORT.md](../../EXPORT.md).
