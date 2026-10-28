# C# / .NET bindings

.NET host for the [Offline Protocol SDK](https://github.com/Offline-Protocol/offline-protocol-sdk)
on Windows (primary) and other desktop targets. The binding calls the same
**C API** and native library as [bindings/c](../c/README.md) (`offline_protocol_uniffi.dll`
on Windows), not a separate UniFFI bindgen output.

Bridge contract: [docs/bridges/csharp.md](../../docs/bridges/csharp.md).

## Requirements

| What | Version |
|------|---------|
| .NET SDK | 8.0+ |
| Native library | `cargo build --release -p offline-protocol-uniffi` → `target/release/` |

Set **`OFFLINE_PROTOCOL_LIBRARY_DIR`** to the directory containing the native
library before running tests or the consumer check (same role as
`OFFLINE_PROTOCOL_LIBRARY_DIR` / `java.library.path` for other bindings).

## Build

```bash
cd bindings/csharp
dotnet build
dotnet test --no-build   # after build; JSON/unit tests only (no native library)
```

## Consumer check

Proves the P/Invoke surface, engine thread, and file-store MLS path against a
real library:

```bash
export OFFLINE_PROTOCOL_LIBRARY_DIR="$PWD/../../target/release"   # adjust path
dotnet run --project consumer-check/ConsumerCheck.csproj -c Release
```

On Windows (PowerShell):

```powershell
$env:OFFLINE_PROTOCOL_LIBRARY_DIR = "..\..\target\release"
dotnet run --project consumer-check/ConsumerCheck.csproj -c Release
```

## Layout

| Path | Role |
|------|------|
| `src/OfflineProtocol.Sdk/` | Hand-written .NET SDK (PR1: lifecycle + `offline_protocol_call`) |
| `consumer-check/` | CI-style smoke program |
| `../c/include/offline_protocol.h` | Stable C ABI (do not fork) |

Full RN parity (typed local API, events, transports, storage providers, CI) lands
in follow-up PRs.
