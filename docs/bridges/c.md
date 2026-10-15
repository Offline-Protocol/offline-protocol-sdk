# C binding contract

Covers the C API in `bindings/c/include/offline_protocol.h`, implemented in
`crates/offline-protocol-uniffi/src/capi`. Any language that calls C (C++,
Go through cgo, C# through P/Invoke, Zig, Rust through a foreign build) uses
it the same way.

Read [the shared contract](README.md) first. This document covers what is
specific to a host that drives the engine through C.

## What it is

**The C binding is the [local API](../spec/local-api.md) called in process.**
A method is named as that chapter's method table names it: bare for the
engine and for the definition's instance-less functions, `services.` for
`MeshServices`, `data.` for `DataStore`. Its parameters are a JSON object
keyed by the definition's parameter names, encoded by the chapter's
[encoding table](../spec/local-api.md#encoding), and its result is JSON. An
engine refusal carries the definition's `ProtocolError` variant name and its
zero-based position, the same `n` as the local API's `-32000 - n`.

So the C surface is about a dozen functions, and it does not change when the
interface definition grows. A method added to the definition is callable
from C with no C-side change, because the dispatch is generated from the
definition (`capi/generated.rs`).

The shape was chosen over one C function per declaration for three reasons.
Events already cross every boundary as JSON (C3), so a C host parses JSON
whatever the call surface is. A typed C API would be about 220 functions and
46 structures with nested ownership, whose field lists drift with nothing to
catch it. And a stable ABI is what a wrapper in another language needs:
a Go or C# package written against these functions keeps working across
releases of the definition.

## Which shared rules apply

| Rule | How it reaches the C binding |
|---|---|
| [C1](README.md#c1-regenerate-every-binding-together) | Does not apply to the header: the C functions take JSON, so nothing in them is generated from the definition. It applies to the dispatcher, which is generated from the definition and checked by `the_generated_c_dispatch_is_current` |
| [C2](README.md#c2-the-error-enum-is-append-only) | `OfflineProtocolError.code` is the variant's position. Append-only is what keeps a C host's `switch` on it correct |
| [C3](README.md#c3-events-cross-as-opaque-json) | An event reaches C as the engine's JSON, unchanged, through the callback and through `offline_protocol_poll_event` alike |
| [C6](README.md#c6-config-parsers-must-not-default-to-literals) | An omitted dictionary field takes the definition's own default, which the generator reads from the definition. The binding writes no default of its own |
| [C9](README.md#c9-bridge-behaviour-is-not-covered-by-the-rust-test-suite) | Covered twice: the boundary is tested from Rust through the `extern "C"` functions, and `bindings/c/consumer-check` is a C program compiled and run in CI |
| [C11](README.md#c11-a-storage-adapter-is-a-supported-extension-point-and-is-verified) | A C storage provider is an adapter like any other. `offline_protocol_run_storage_conformance` is the same suite, and green is the same bar |
| [C12](README.md#c12-telemetry-is-callback-free-and-the-binding-fills-the-platform) | See K3 |

## K1. Ownership has one rule

**Every `char *` the library hands out is freed with
`offline_protocol_string_free`, and nothing else is.** Results, polled events
and the two strings of an `OfflineProtocolError` are all such strings. The
error's strings are freed together by `offline_protocol_error_clear`, which
also resets the structure for reuse. The library writes an error only when a
call fails and never reads one, so a host that reuses an error clears it
first, or leaks the previous strings.

Strings the host passes in are borrowed for the call. `offline_protocol_version`
returns a static string.

The failure this prevents is a host freeing a library string with its own
`free`, which works with one allocator and corrupts the heap with another.

## K2. Callbacks run on the engine's thread, inside the engine

**An event or transport callback runs synchronously, on the thread that is
inside the engine, before the engine call that produced it returns.** That
may be the host's own thread (an event emitted by `send_message`) or one of
the engine's (the telemetry uploader). So:

- A callback must be callable from any thread, and so must every storage
  function. The library promises Rust this (`Send + Sync`) on the host's
  behalf and cannot check it.
- **A callback must not call back into the engine.** The engine may be
  holding a lock the call needs. Copy the event, queue it, return. The
  supported alternative to a callback is polling: every event is also
  queued (the newest 1000), and `offline_protocol_poll_event` takes the
  oldest.
- A callback must not unwind: no C++ exception and no `longjmp` out of it.
  Unwinding into Rust through a C frame is undefined behaviour.

Every function that takes a callback also takes a `drop` function and a
`ctx`. The library calls `drop(ctx)` exactly once, when it no longer holds
the callback: when it is replaced, when the engine is freed, or when the
registration is refused. A replaced event callback can still be running on
another thread at the moment it is replaced; `drop` comes after that call
returns, which is the only point at which freeing `ctx` is safe.

## K3. The binding fills the telemetry platform

`TelemetryConfig.os` is filled by the binding from the target the library
was built for, and a configuration that carries `os` is refused with
`InvalidParams`. `os_major` is the host's to supply: the host is the
platform here, and Rust has no portable way to read an operating system's
major version. The lifecycle is the host's too: report backgrounding and
foregrounding with `notify_app_state`, and never call `disable_telemetry`,
`end_telemetry_session` or `flush_telemetry_blocking` on a UI thread.

## K4. A panic is a status, not a crash

Every entry point catches a panic and answers
`OfflineProtocolStatus_Internal`. A panic reaching an `extern "C"` boundary
aborts the process, which for a library embedded in someone else's program
is a crash in their program. A build under `panic = "abort"` (the `minisize`
profile) aborts before anything can be caught; the desktop libraries the
release ships use the unwinding `release` profile.

## K5. What `offline_protocol_call` does not reach

The declarations below take a callback or an object, or construct one, and
so cannot be called with JSON parameters. Each has its own entry point or is
not available from C. A guard
(`the_hand_written_declarations_are_the_ones_the_bridge_doc_lists`) holds
this table to the generator's list, so a declaration the generator skips is
listed here or the suite is red.

<!-- hand-written: begin -->
| Declaration | From C |
|---|---|
| `constructor` | `offline_protocol_create` |
| `set_event_callback` | `offline_protocol_set_event_callback` |
| `set_ble_transport_callback` | `offline_protocol_set_transport_callback` with `OfflineProtocolTransport_Ble` |
| `set_wifi_direct_transport_callback` | `offline_protocol_set_transport_callback` with `OfflineProtocolTransport_WifiDirect` |
| `set_reticulum_transport_callback` | `offline_protocol_set_transport_callback` with `OfflineProtocolTransport_Reticulum` |
| `set_nostr_transport_callback` | `offline_protocol_set_transport_callback` with `OfflineProtocolTransport_Nostr` |
| `initialize_mls` | `offline_protocol_initialize_mls` with two `OfflineProtocolStorage` providers |
| `run_storage_conformance` | `offline_protocol_run_storage_conformance` |
| `services.constructor` | Implicit: built on the first `services.` call |
| `data.constructor` | Implicit: built on the first `data.` call, and retried on the next one if it failed (it refuses before `initialize_mls`) |
| `data.with_storage` | Not available. Documents use the engine's protocol-state provider |
<!-- hand-written: end -->

`initialize_mls_with_file_stores` takes no callback and goes through
`offline_protocol_call`. It is the simplest route for a host with a
filesystem and no platform keystore.

## K6. Storage providers

`OfflineProtocolStorage` is the four operations of
`MlsStorageProvider` and `ProtocolStateStorageProvider` as C functions, plus
`release` for buffers the host hands back and `drop`. The interface
definition's comments on those two callback interfaces are the contract, and
two of them are worth restating because they are where a provider loses
data:

- **Answer `OfflineProtocolStorageStatus_CorruptedData` only for a record no
  later launch can read.** A locked keystore or an I/O failure is
  `LoadFailed`. The engine answers corruption on the protocol-state record
  key by minting a new key, which strands every record sealed under the old
  one. A value outside the six the header names is read as the operation's
  own failure, never as corruption, so a host that returns `-1` for any
  error loses nothing.
- **The protocol-state `load` bounds its read** at the record ceiling
  (8 MiB) by checking the stored size first. The library can only check a
  length it has already been handed.

`load` answers absent as success with `*out_data = NULL`, and `list_keys`
answers a JSON array of strings, as bytes. The library copies every buffer
it is handed and returns it through `release` before the call that received
it returns.

## What the host owes

The same things every binding owes, with nothing provided:

- **The transports.** The crates open no socket and touch no radio for the
  protocol. A host drains each transport's outbound queue (the `ble_`,
  `wifi_direct_`, `internet_`, `reticulum_` and `nostr_` methods), performs
  the I/O, reports the outcome, and injects what it receives. The transport
  callbacks say when a queue has something; the Internet transport has none
  and is polled.
- **Storage**: two providers over storage that is durable and atomic per
  entry, or the built-in file stores.
- **The lifecycle**: `start`, `stop`, `process` on a timer, and the telemetry
  notifications of K3.
- **One engine per handle, freed once**, with no call in flight when it is.

## Testing

```bash
cargo test -p offline-protocol-uniffi --lib capi
scripts/generate-c-header.sh --check
cargo build -p offline-protocol-uniffi
make -C bindings/c/consumer-check
```

The first runs the dispatcher's and the boundary's tests and the guards.
The consumer check compiles a C program against the header with
`-std=c99 -Wall -Wextra -Wpedantic -Werror`, links it against the shared and
the static library in turn, runs both, and compiles the header as C++. CI
runs all four on Linux and macOS, building the library once through the
probe that reads the static library's system dependencies. Windows is
built by the release and not yet linked by CI.

## Which builds carry it

**The C binding is the crate's `capi` feature, on by default and off in the
Android builds.** A shared library exports every `#[no_mangle]` symbol, so
nothing can strip the dispatcher behind the C functions, and an Android
application has no C host to call them. The desktop libraries the release
ships carry it. The iOS builds keep the default: an application links the
static library with dead stripping, which drops exports nothing references.
CI builds the crate without the feature under `-D warnings`, so a change
that only compiles with the C binding fails before a release does.

## What this does not cover

- **A typed C++ API.** The header compiles as C++; a class wrapper is a
  package of its own.
- **A leaf node from C.** The leaf crate is `no_std` with a different
  surface, and a C API for firmware is separate work.
