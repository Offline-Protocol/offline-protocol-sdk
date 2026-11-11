# React Native config presets (`forScenario`)

G4 PR1: host-side helpers only. **No UDL or wire changes.** Presets wrap
existing `ProtocolConfig` fields; they do not replace reading
[configuration.md](configuration.md) when tuning DORS, relay, or encryption.

## Quick use

```typescript
import { OfflineProtocol, forScenario } from '@offline-protocol/mesh-sdk';

const config = forScenario('localBle', {
  appId: 'my-chat',
  profile: 'default',
});

const protocol = new OfflineProtocol(config);
```

Add transport credentials in `overrides`:

```typescript
const config = forScenario('internetRelay', {
  appId: 'my-chat',
  profile: 'default',
  overrides: {
    transports: {
      internet: {
        serverAddress: 'wss://relay.example.com/ws',
        authToken: process.env.RELAY_TOKEN,
      },
    },
  },
});
```

## Scenario table

| Scenario | Transports | DORS / other | You must still set | When to use |
|----------|------------|--------------|--------------------|-------------|
| `localBle` | BLE on; Wi‑Fi Direct, internet, Nostr, Reticulum off | (core defaults) | `appId`, `profile` | First on-device mesh; smallest radio surface |
| `proximityMesh` | BLE + Wi‑Fi Direct on; internet/Nostr off | (core defaults) | Grants before `start()` on Android (see [mobile-permissions.md](mobile-permissions.md)) | Higher throughput when peers are nearby on LAN/Wi‑Fi Direct |
| `internetRelay` | BLE + internet on | `dors.preferOnline: true` | `transports.internet.serverAddress` (and `authToken` if required) | Apps with a relay/WebSocket back end |
| `nostrMesh` | BLE + Nostr on | (core defaults) | `transports.nostr.relayUrls` | Nostr bootstrap path; needs MLS identity (`encryption` default on) |
| `developmentCleartext` | BLE on | `encryption.enabled: false` | Nothing beyond ids | **Dev / lab only.** Production must use default fail-closed encryption |

## Defaults philosophy

Presets intentionally **do not** restate every DORS, relay, or encryption field.
Those defaults live in the Rust core and cross the bridge only when your config
names them. Overriding a scenario with `overrides` merges one level deep
(`transports.ble`, `dors.preferOnline`, and similar).

## Native mobile / Flutter

Swift hosts use `ConfigScenarios.forScenario` / `forScenarioJson` on the shared
iOS bridge (same scenario ids). See [ios-config-presets.md](ios-config-presets.md).

Kotlin hosts use the same API on the shared Android bridge. See
[android-config-presets.md](android-config-presets.md).

JVM (desktop/server) hosts use `com.offlineprotocol.config.ConfigScenarios`. See
[java-config-presets.md](java-config-presets.md).

C# hosts use `OfflineProtocol.Sdk.Config.ConfigScenarios`. See
[csharp-config-presets.md](csharp-config-presets.md).

C hosts use `op_config_scenario_json` from `config_scenarios.h`. See
[c-config-presets.md](c-config-presets.md).

## Related

| Topic | Document |
|-------|----------|
| Full field reference | [bindings/react-native/README.md](../bindings/react-native/README.md#configuration) |
| DORS tuning | [configuration.md](configuration.md), [dors-configuration.md](dors-configuration.md) |
| Permissions timing | [mobile-permissions.md](mobile-permissions.md) |
| Lifecycle order | [integrator-map.md](integrator-map.md#react-native-lifecycle-ordering) |
