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

Every engine method without a hand-written entry point has a **generated**
wrapper: an instance method on `ProtocolEngine`, a method on `engine.Services`
or `engine.Data` for the `services.` and `data.` objects, or a static method on
`ProtocolEngine` for a namespace function such as `DeriveAddressAsync`.
`ProtocolConfig`, its serializer and the enums the SDK exposes are generated
too. `tools/gen_csharp_local_api.py` writes all of them from the interface
definition into `src/OfflineProtocol.Sdk/Generated`; rerun it after any UDL
change. The hand-written entry points (create, MLS initialization, start, stop,
events, telemetry, storage conformance) stay in `ProtocolEngine.cs`.

## Which shared rules apply

| Rule | How it reaches the C# binding |
|------|-------------------------------|
| [C2](README.md#c2-the-error-enum-is-append-only) | Engine refusals surface as `ProtocolEngineException` with `Variant` and `Code` from the C error |
| [C3](README.md#c3-events-cross-as-opaque-json) | Events are JSON text via callback and `PollEventAsync` |
| [C5](README.md#c5-hand-mirrored-constants-must-be-pinned-in-every-language) | `OfflineProtocolStatus` copies the C status values; `the_csharp_status_enum_is_the_c_apis` pins it to the Rust enum |
| [C6](README.md#c6-config-parsers-must-not-default-to-literals) | A field with a default in the interface definition is nullable in `ProtocolConfig` and left out of the JSON while unset, so the engine applies its own default. The SDK carries no configuration values: the consumer check's are an example program's, as in the C quick start |
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

### Events

`SetEventCallbackAsync` installs `offline_protocol_set_event_callback`. Events
are copied on the engine thread, queued on a **bounded channel** (default 256,
`DropWrite` when full), and delivered on a **dedicated background delivery
thread**. A **generation stamp** drops any item queued before deactivate, so
teardown does not invoke the handler for stale work. **`DroppedEventDeliveryCount`**
counts queue rejections since the current callback was installed. Under burst
load, **`PollEventAsync`** reads the same native queue as the C binding and does
not share the delivery channel cap. Clearing the callback or disposing the engine
**deactivates** the bridge first (which **joins** the delivery thread after the
current handler returns). Keep handlers **short and non-blocking**: a slow handler
delays dispose and blocks the engine thread until delivery finishes. The handler
must not call back into the same `ProtocolEngine` synchronously.

**A throwing handler never ends the process.** An exception escaping a dedicated
thread, or a callback the native library invoked, terminates a .NET process, so
the bridge catches what the handler throws, counts it in
**`EventHandlerFailureCount`**, and goes on delivering. The native callbacks
themselves catch everything too. Without this, one bug in app code would take the
host down from a thread it never created.

The bridge's context handle is freed **once**, by whichever of the native `drop`
and the managed release runs first. The C API calls `drop` on every refused
install as well as on replacement, so freeing on both paths would free a handle
twice and could release another object's.

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

## CS4. Storage providers

`NativeStorageProvider` wraps an `IProtocolStorage` as
`OfflineProtocolStorage` for `offline_protocol_initialize_mls` and
`offline_protocol_run_storage_conformance`. **`MemoryProtocolStorage`** is a
thread-safe in-memory backend for tests and consumer-check; production hosts
should implement `IProtocolStorage` over durable, atomic storage (Windows
Credential Manager, DPAPI-protected files, and so on). Every method may be called
from any thread concurrently, as the C header says. `MemoryProtocolStorage` keeps
an emptied key-type bucket rather than pruning it, because a prune between the
emptiness check and the removal discards a concurrent write, and the conformance
suite is single-threaded so it would not notice.

Keep each `NativeStorageProvider` alive until the engine has dropped it (after
`DisposeAsync`). The engine holds references after a successful
`InitializeMlsAsync`.

**A provider is single use.** The library calls its `drop` once it lets go: when
the engine is freed, before a refused `InitializeMlsAsync` returns, and as soon as
`RunStorageConformanceAsync` returns. Its context handle dies with that drop, so
handing the provider over again throws `ObjectDisposedException` rather than give
native a freed handle. To probe a backend and then use it, wrap the same
`IProtocolStorage` in a new provider. `InitializeMlsAsync` checks both providers,
and refuses one provider passed twice, before it hands either out, so a refusal
strands neither.

`InitializeMlsWithFileStoresAsync` remains the filesystem path when no custom
provider is needed ([C K5](c.md#k5-what-offline_protocol_call-does-not-reach)).

## CS5. Telemetry (C12)

Same split as [C K3](c.md#k3-the-binding-fills-the-telemetry-platform):

- **`TelemetryConfig` never includes `os`.** The C API fills `os` from the
  library build target when `enable_telemetry` runs.
- **`os_major` is filled by the binding** via `TelemetryPlatform.OsMajor`
  unless the host overrides it on the config object.

The binding **owns the lifecycle**: WinUI/WPF/MAUI must call
`NotifyAppStateAsync(AppState.Background)` when the app leaves the foreground
and `AppState.Active` when it returns, matching Kotlin's process lifecycle and
Swift's `UIApplication` edges. `FlushTelemetryBlockingAsync(deadlineMs)` holds
the engine thread for up to the deadline and returns whether the uploader finished;
it is `false` while telemetry is off. Call it from a background-transition handler,
never from a UI thread.

## CS6. Typed config and the generated local API

**Invariant: the C# surface is derived from the interface definition, never
copied from it.** A copy drifts silently: a field appended to `ProtocolConfig`
in the UDL would be missing from a hand-written class with nothing to say so.

- **`ProtocolConfig`** is a record with a `required` property for each field
  that has no default in the definition and a nullable one for each that does.
  **`ProtocolConfigJson.Serialize`** writes the required fields and every
  nullable one that is set, even to the default's value, and refuses an enum
  value the definition does not name. `ProtocolEngine.CreateAsync(ProtocolConfig)`
  calls it; `CreateAsync(string)` takes raw JSON.
- The nested sections (`mesh_relay`, `custody`) cross as the host's own JSON
  objects, set as `JsonElement`; the engine parses them.
- **A generated method never shares a name with a hand-written one.** The two
  differ in their parameters, so both compile and a call with no arguments
  becomes ambiguous or binds to the wrapper. `poll_event` did this: its wrapper
  went through the JSON dispatcher and lost the `null` an empty queue returns.
  Such a declaration goes in the generator's skip list; the generator refuses
  to write a clash and `no_generated_csharp_method_shadows_a_hand_written_one`
  refuses a committed one.
- **`LocalApiParams`** builds the parameters of `initialize_mls_with_file_stores`
  and `send_message`. **`LocalApiTyped.GetStateAsync`** parses `get_state` into
  `ProtocolState` and refuses anything but a JSON string naming a member.
- `bindings/csharp/local_api_methods.txt` lists every wrapped wire name, so a
  UDL change shows up as a reviewable diff.

## What CI runs

The `C# Binding` job, on Linux, macOS and Windows, whenever the binding, the C
header or the FFI crate changes:

| Step | What it proves |
|------|----------------|
| `gen_csharp_local_api.py --check`, Linux only | The generated sources match the interface definition |
| `dotnet build` | The SDK, the tests and the consumer check compile with warnings as errors |
| `dotnet test` | The engine facade against the library built from the same tree: each status, the queue under backpressure, dispose after unawaited calls, cancellation; events on the delivery thread, a throwing handler contained, a cleared callback; conformance of the memory store, single-use providers, MLS over two providers; the telemetry refusal reached past the `os` fill, and the lifecycle calls with telemetry off |
| `consumer-check`, library from `OFFLINE_PROTOCOL_LIBRARY_DIR` | A generated static call, storage conformance, create from a typed `ProtocolConfig`, `get_state` parsed as `Stopped`, MLS over custom providers, events, `enable_telemetry` refused on an empty key after the C API fills `os`, disable, flush, app-state notifications, start, stop, file-store MLS, a document through `engine.Data`, close (CS3 step 1) |
| `consumer-check`, library beside it | The same run with the variable unset (CS3 step 2) |

`the_csharp_binding_has_a_ci_job_the_result_waits_on` holds the job, its
change filter and its place in the aggregate check.
`the_csharp_job_checks_the_generated_sources` holds the `--check` step and the
generator's inputs in that filter, and
`every_generated_csharp_method_is_dispatched_by_the_c_api` that each wrapped
wire name is one the C API answers.
