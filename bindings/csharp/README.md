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

The SDK loads the native library from **`OFFLINE_PROTOCOL_LIBRARY_DIR`** when
it is set, and otherwise from the application directory and the system paths
([CS3](../../docs/bridges/csharp.md#cs3-native-library-loading)). The tests and
the consumer check load it, so build it first.

## Build

```bash
cargo build -p offline-protocol-uniffi
cd bindings/csharp
dotnet build -c Release
OFFLINE_PROTOCOL_LIBRARY_DIR="$PWD/../../target/debug" dotnet test -c Release --no-build
```

## Consumer check

Proves the P/Invoke surface, engine thread, and file-store MLS path against a
real library:

```bash
export OFFLINE_PROTOCOL_LIBRARY_DIR="$PWD/../../target/debug"   # or target/release
dotnet run --project consumer-check/ConsumerCheck.csproj -c Release
```

On Windows (PowerShell):

```powershell
$env:OFFLINE_PROTOCOL_LIBRARY_DIR = "..\..\target\debug"
dotnet run --project consumer-check/ConsumerCheck.csproj -c Release
```

## Layout

| Path | Role |
|------|------|
| `src/OfflineProtocol.Sdk/` | Hand-written SDK: engine thread, storage, telemetry, events |
| `src/OfflineProtocol.Sdk/Generated/` | Generated from the interface definition: the local API wrappers, `ProtocolConfig` and its enums. Do not edit |
| `local_api_methods.txt` | Every wire name with a generated wrapper |
| `tests/OfflineProtocol.Sdk.Tests/` | The engine facade against the native library |
| `consumer-check/` | CI-style smoke program |
| `../c/include/offline_protocol.h` | Stable C ABI (do not fork) |

Regenerate after a UDL change:

```bash
python3 tools/gen_csharp_local_api.py
```

Not wired yet: transports.
