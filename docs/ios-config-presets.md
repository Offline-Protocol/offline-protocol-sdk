# iOS / Swift config presets (`ConfigScenarios`)

G4 PR2: host-side helpers on the shared iOS bridge (ADR 0025). **No UDL or wire
changes.** Mirrors [React Native `forScenario`](react-native-config-presets.md).

## Direct UniFFI (native Swift)

```swift
import OfflineProtocol

let config = ConfigScenarios.forScenario(
    .localBle,
    appId: "my-ios-app",
    profile: "default"
)

let mesh = try OfflineProtocol(config: config)
```

Customize after the preset:

```swift
let config = ConfigScenarios.forScenario(.internetRelay, appId: "app", profile: "user") { cfg in
    // Internet transport address is configured when enabling the transport, not
    // on ProtocolConfig — see InternetManager / enableTransport in your host.
    _ = cfg
}
```

## JSON (Flutter plugin / `ProtocolEngineHost`)

```swift
let json = try ConfigScenarios.forScenarioJson(
    .proximityMesh,
    appId: "app",
    profile: "default"
)
try host.create(configJson: json, eventCallback: callback, emitEventJson: nil)
```

## Scenarios

Same ids as React Native: `localBle`, `proximityMesh`, `internetRelay`,
`nostrMesh`, `developmentCleartext` (lab only). See the
[scenario table](react-native-config-presets.md#scenario-table).

## Related

- [ios-integration.md](ios-integration.md)
- [configuration.md](configuration.md)
