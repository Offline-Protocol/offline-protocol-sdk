# Android / Kotlin config presets (`ConfigScenarios`)

G4 PR3: host-side helpers on the shared Android bridge (ADR 0025). **No UDL or wire
changes.** Mirrors [React Native `forScenario`](react-native-config-presets.md).

## Direct UniFFI (native Kotlin)

```kotlin
import com.offlineprotocol.ConfigScenarios
import com.offlineprotocol.ConfigScenario
import uniffi.offline_protocol.OfflineProtocol

val config = ConfigScenarios.forScenario(
    ConfigScenario.localBle,
    appId = "my-android-app",
    profile = "default",
)

val mesh = OfflineProtocol(config)
```

Customize after the preset:

```kotlin
val config = ConfigScenarios.forScenario(
    ConfigScenario.internetRelay,
    appId = "app",
    profile = "user",
) { cfg ->
    // Internet transport address is configured when enabling the transport, not
    // on ProtocolConfig — see your InternetManager / enableTransport host code.
}
```

`ConfigScenarios` lives in `com.offlineprotocol` with the React Native bridge
sources. Native apps that ship only generated UniFFI Kotlin still need these
helpers on the classpath (for example via the assembled Android library or by
vendoring the same bridge files).

## JSON (Flutter plugin / `ProtocolEngineHost`)

```kotlin
import com.offlineprotocol.ConfigScenarios
import com.offlineprotocol.ConfigScenario

val json = ConfigScenarios.forScenarioJson(
    ConfigScenario.proximityMesh,
    appId = "app",
    profile = "default",
)
host.create(configJson = json, eventCallback = callback, emitEventJson = null)
```

## Scenarios

Same ids as React Native: `localBle`, `proximityMesh`, `internetRelay`,
`nostrMesh`, `developmentCleartext` (lab only). See the
[scenario table](react-native-config-presets.md#scenario-table).

## Related

- [android-integration.md](android-integration.md)
- [configuration.md](configuration.md)
