# C# config presets (`ConfigScenarios`)

G4 PR5 (C# half): hand-written helpers on `OfflineProtocol.Sdk`. **No UDL or wire
changes.** Mirrors [React Native `forScenario`](react-native-config-presets.md).

## Direct use

```csharp
using OfflineProtocol.Sdk;
using OfflineProtocol.Sdk.Config;

var config = ConfigScenarios.ForScenario(
    ConfigScenario.localBle,
    "com.example.app",
    "default");

await using var engine = await ProtocolEngine.CreateAsync(config);
```

Tune after the preset with record `with`:

```csharp
var config = ConfigScenarios.ForScenario(
    ConfigScenario.internetRelay,
    "com.example.app",
    "alice",
  map: c => c with { /* fields */ });
```

## JSON

`ForScenarioJson` serializes through `ProtocolConfigJson` (snake_case, engine
shape for the C API).

## Scenarios

Same ids as React Native: `localBle`, `proximityMesh`, `internetRelay`,
`nostrMesh`, `developmentCleartext` (lab only). See the
[scenario table](react-native-config-presets.md#scenario-table).

## Related

- [bindings/csharp/README.md](../bindings/csharp/README.md)
- [docs/bridges/csharp.md](bridges/csharp.md)
