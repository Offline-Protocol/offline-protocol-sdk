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
| [C3](README.md#c3-events-cross-as-opaque-json) | Events are JSON text (callback/poll in later PRs) |
| [C6](README.md#c6-config-parsers-must-not-default-to-literals) | PR1 uses explicit JSON for required config fields; typed config follows in later PRs |
| [C9](README.md#c9-bridge-behaviour-is-not-covered-by-the-rust-test-suite) | `consumer-check` plus future Windows CI; `cargo test` does not cover this binding |
| [C11](README.md#c11-a-storage-adapter-is-a-supported-extension-point-and-is-verified) | Custom storage via C providers (later PR); PR1 uses `initialize_mls_with_file_stores` |
| [C12](README.md#c12-telemetry-is-callback-free-and-the-binding-fills-the-platform) | Host fills telemetry platform fields (later PR) |

## CS1. Engine thread and callbacks (K2 parity)

**All calls into `offline_protocol_*` for a given engine run on one dedicated
.NET thread** (`ProtocolEngine`). Public async methods queue work there and
complete on the caller's synchronization context.

Native **event and transport callbacks** run on the engine thread inside the
library ([C K2](c.md#k2-callbacks-run-on-the-engines-thread-inside-the-engine)).
They must not call back into the engine: copy JSON and return; delivery to app
code is posted off the callback (wired in PR2+).

## CS2. String ownership

Strings returned from the library are freed with `offline_protocol_string_free`
inside the SDK. Callers receive managed `string` copies only.

## CS3. Native library loading

The SDK loads `offline_protocol_uniffi` from:

1. `OFFLINE_PROTOCOL_LIBRARY_DIR` + platform file name, if set and the file exists.
2. Otherwise the default `DllImport` search path.

Release archives ship `offline_protocol_uniffi.dll` on Windows x64.

## What CI runs (PR1)

PR1 does not add a GitHub Actions job yet. Run `consumer-check` locally with a
release build of `offline-protocol-uniffi`. Windows CI is planned in a later PR
in the C# stack.
