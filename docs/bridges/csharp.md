# C# binding contract

Covers `bindings/csharp`: a .NET host that drives the engine through
[the C API](c.md) (`offline_protocol_uniffi.dll` on Windows).

Read [the shared contract](README.md) first. This document covers what is
specific to C#.

## What it is

The C# binding is **hand-written** P/Invoke over `bindings/c/include/offline_protocol.h`.
It uses the [local API](../spec/local-api.md) JSON shape: methods named as the
method table names them, parameters as JSON objects, results and events as JSON
strings. It is **not** part of the UniFFI five-language artifact set ([C1](README.md#c1-regenerate-every-binding-together)).

A new engine method in the UDL is callable through `ProtocolEngine.CallAsync`
without regenerating C#, because dispatch stays in the Rust C API.

## Which shared rules apply

| Rule | How it reaches the C# binding |
|------|-------------------------------|
| [C2](README.md#c2-the-error-enum-is-append-only) | Engine refusals surface as `ProtocolEngineException` with `Variant` and `Code` from the C error |
| [C3](README.md#c3-events-cross-as-opaque-json) | Events are JSON text via callback and `PollEventAsync` |
| [C5](README.md#c5-hand-mirrored-constants-must-be-pinned-in-every-language) | `OfflineProtocolStatus` copies the C status values; `the_csharp_status_enum_is_the_c_apis` pins it to the Rust enum |
| [C6](README.md#c6-config-parsers-must-not-default-to-literals) | The SDK carries no `ProtocolConfig`: the host passes its own JSON and omitted fields take the definition's defaults. The consumer check's configuration is an example program's, as in the C quick start |
| [C9](README.md#c9-bridge-behaviour-is-not-covered-by-the-rust-test-suite) | The `C# Binding` CI job builds it, runs its tests and runs `consumer-check` on Linux, macOS and Windows; `cargo test` does not cover this binding |
| [C11](README.md#c11-a-storage-adapter-is-a-supported-extension-point-and-is-verified) | `IProtocolStorage` + `NativeStorageProvider`; `RunStorageConformanceAsync` |
| [C12](README.md#c12-telemetry-is-callback-free-and-the-binding-fills-the-platform) | `TelemetryConfig` + `TelemetryPlatform.OsMajor`; lifecycle via `NotifyAppStateAsync` |

## CS1. Engine thread and callbacks (K2 parity)

**All calls into `offline_protocol_*` for a given engine run on one dedicated
.NET thread** (`ProtocolEngine`). Public async methods queue work on a
**bounded channel** (default capacity 512) and complete on the caller's
synchronization context. When the queue is full, callers block in
`WriteAsync` until the engine drains the queue: this caps memory if the app
submits work faster than the engine runs it. **UI hosts should always `await`
each engine call** (and use `ConfigureAwait(true)` on the UI thread when
continuations must run there). Flooding the engine without awaiting can stall
the UI thread when the queue is full.

**One thread is a cost as well as a guarantee.** A slow call (`start` dialing
a relay, a large `send_file`, `flush_telemetry_blocking`) delays every call
queued behind it, `stop` included.

**Cancellation** ends the caller's wait, never the call. A call cancelled
before it enters the queue is not run; one already queued still runs to
completion on the engine thread, and its task reports cancellation. So a
cancelled `CreateAsync` whose create already ran still frees that engine.

**Dispose never blocks the calling thread**, so a UI thread may await it. It
refuses new calls (`ObjectDisposedException`), waits asynchronously until every
call it had accepted has finished, then runs `offline_protocol_free` on the
engine thread, after the last call that could use the handle. Every
`DisposeAsync` caller awaits that one shutdown. Hosts need not await pending
calls first for safety, but awaiting them is how their failures surface.

The engine thread is a **background** thread so a forgotten host does not block
process exit. **WinUI, WPF, and MAUI hosts must call `DisposeAsync` (or
`StopAsync` then dispose) before application shutdown** so telemetry flush and
transport teardown can finish; a follow-up PR may offer a foreground-thread option.

Native **event and transport callbacks** run on the engine thread inside the
library ([C K2](c.md#k2-callbacks-run-on-the-engines-thread-inside-the-engine)).
They must not call back into the engine: copy JSON and return; delivery to app
code is posted off the callback (see Events below).

### Events (PR2)

`SetEventCallbackAsync` installs `offline_protocol_set_event_callback`. Events
are copied on the engine thread, queued on a **bounded channel** (default 256,
`DropWrite` when full), and delivered on a **dedicated background delivery
thread**. Clearing the callback or disposing the engine **deactivates** the
bridge first so queued events are not delivered afterward. The handler must not
call back into the same `ProtocolEngine` synchronously. `PollEventAsync` reads
the same queue as the C binding's `offline_protocol_poll_event`.

## CS2. String ownership

Strings returned from the library are freed with `offline_protocol_string_free`
inside the SDK. Callers receive managed `string` copies only.

## CS3. Native library loading

The SDK loads `offline_protocol_uniffi` from:

1. `OFFLINE_PROTOCOL_LIBRARY_DIR` joined with the platform file name, when the
   variable is set. A set directory without the library is a
   `FileNotFoundException` naming the path, never a silent fall through.
2. Otherwise the runtime's own probing: the application directory, then the
   system paths, with the platform's `lib` prefix and suffix.

Step 2 must stay the runtime's. `NativeLibrary.Load` with the bare name is a
plain `dlopen` or `LoadLibrary`: it finds `offline_protocol_uniffi.dll` on
Windows and fails on macOS and Linux with the library beside the application.
CI runs the consumer check both ways on all three systems.

Release archives ship `offline_protocol_uniffi.dll` on Windows x64.

## CS4. Storage providers (PR2)

`NativeStorageProvider` wraps an `IProtocolStorage` as
`OfflineProtocolStorage` for `offline_protocol_initialize_mls` and
`offline_protocol_run_storage_conformance`. **`MemoryProtocolStorage`** is a
thread-safe in-memory backend for tests and consumer-check; production hosts
should implement `IProtocolStorage` over durable, atomic storage (Windows
Credential Manager, DPAPI-protected files, and so on) in a later PR.

Keep each `NativeStorageProvider` alive until the engine has dropped it (after
`DisposeAsync`). The engine holds references after a successful
`InitializeMlsAsync`.

`InitializeMlsWithFileStoresAsync` remains the filesystem path when no custom
provider is needed ([C K5](c.md#k5-what-offline_protocol_call-does-not-reach)).

## CS5. Telemetry (PR2, C12)

Same split as [C K3](c.md#k3-the-binding-fills-the-telemetry-platform):

- **`TelemetryConfig` never includes `os`.** The C API fills `os` from the
  library build target when `enable_telemetry` runs.
- **`os_major` is filled by the binding** via `TelemetryPlatform.OsMajor`
  unless the host overrides it on the config object.

The binding **owns the lifecycle**: WinUI/WPF/MAUI must call
`NotifyAppStateAsync(AppState.Background)` when the app leaves the foreground
and `AppState.Active` when it returns, matching Kotlin's process lifecycle and
Swift's `UIApplication` edges. Do not call `FlushTelemetryBlockingAsync` from a
UI thread; it blocks the engine thread until the uploader finishes.

## What CI runs

The `C# Binding` job, on Linux, macOS and Windows, whenever the binding, the C
header or the FFI crate changes:

| Step | What it proves |
|------|----------------|
| `dotnet build` | The SDK, the tests and the consumer check compile with warnings as errors |
| `dotnet test` | The engine facade against the library built from the same tree: each status, the queue under backpressure, dispose after unawaited calls, cancellation |
| `consumer-check`, library from `OFFLINE_PROTOCOL_LIBRARY_DIR` | Static calls, storage conformance, create, MLS over custom providers, events, telemetry lifecycle, start, stop, file-store MLS, close (CS3 step 1) |
| `consumer-check`, library beside it | The same run with the variable unset (CS3 step 2) |

`the_csharp_binding_has_a_ci_job_the_result_waits_on` holds the job, its
change filter and its place in the aggregate check.
