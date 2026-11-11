# Java config presets (`ConfigScenarios`)

G4 PR4: hand-written helpers for the JVM binding. **No UDL or wire changes.**
Mirrors [React Native `forScenario`](react-native-config-presets.md).

The classes live in `com.offlineprotocol.config`, not under the generated
`com.offlineprotocol.sdk` tree, so `scripts/generate-bindings.sh` does not
overwrite them.

## Direct UniFFI

```java
import com.offlineprotocol.config.ConfigScenario;
import com.offlineprotocol.config.ConfigScenarios;
import com.offlineprotocol.sdk.OfflineProtocol;

ProtocolConfig config = ConfigScenarios.forScenario(
    ConfigScenario.localBle,
    "com.example.app",
    "default");

try (OfflineProtocol engine = new OfflineProtocol(config)) {
    // initializeMls, setEventCallback, start() — see bindings/java/README.md
}
```

Customize after the preset:

```java
ProtocolConfig config = ConfigScenarios.forScenario(
    ConfigScenario.internetRelay,
    "com.example.app",
    "alice",
    cfg -> {
        // Internet transport address is configured when enabling the transport,
        // not on ProtocolConfig — wire your transport host accordingly.
    });
```

## JSON (tooling / gateways)

```java
String json = ConfigScenarios.forScenarioJson(
    ConfigScenario.proximityMesh,
    "com.example.app",
    "default");
```

The shape matches mobile bridge presets (`transports`, flat `bleEnabled`, …).
JVM apps normally construct `ProtocolConfig` through `forScenario` instead.

## Scenarios

Same ids as React Native: `localBle`, `proximityMesh`, `internetRelay`,
`nostrMesh`, `developmentCleartext` (lab only). See the
[scenario table](react-native-config-presets.md#scenario-table).

## Related

- [bindings/java/README.md](../bindings/java/README.md)
- [docs/bridges/java.md](bridges/java.md)
- [configuration.md](configuration.md)
