# React Native config presets (`forScenario`)

`forScenario` builds a `ProtocolConfig` for a common transport mix. It is a
host-side helper over existing `ProtocolConfig` fields, with no wire or engine
change behind it, and it does not replace reading
[configuration.md](configuration.md) when tuning DORS, relay, or encryption.

## Quick use

```typescript
import { OfflineProtocol, forScenario } from '@offline-protocol/mesh-sdk';

const config = forScenario('proximityMesh', {
  appId: 'my-chat',
  profile: 'default',
});

const protocol = new OfflineProtocol(config);
```

Add transport credentials in `overrides`. Give only the fields you set; the
preset supplies the rest, including `enabled`:

```typescript
import { forScenario } from '@offline-protocol/mesh-sdk';

// Read the token from your own secure storage or login flow.
declare const relayToken: string;

const config = forScenario('internetRelay', {
  appId: 'my-chat',
  profile: 'default',
  overrides: {
    transports: {
      internet: {
        serverAddress: 'wss://relay.example.com/ws',
        authToken: relayToken,
      },
    },
  },
});
```

```typescript
import { forScenario } from '@offline-protocol/mesh-sdk';

const config = forScenario('nostrMesh', {
  appId: 'my-chat',
  profile: 'default',
  overrides: {
    transports: {
      nostr: { relayUrls: ['wss://relay.example.org'] },
    },
  },
});
```

## Scenario table

| Scenario | Transports | DORS / other | You must still set | When to use |
|----------|------------|--------------|--------------------|-------------|
| `localBle` | BLE on; Wi-Fi Direct, internet, Nostr, Reticulum off | (core defaults) | `appId`, `profile` | First on-device mesh. The same transports as a config that names none, spelled out |
| `proximityMesh` | BLE + Wi-Fi Direct on; internet, Nostr, Reticulum off | (core defaults) | Grants before `start()` on Android (see [mobile-permissions.md](mobile-permissions.md)) | Higher throughput when peers are nearby on LAN/Wi-Fi Direct |
| `internetRelay` | BLE + internet on | `dors.preferOnline: true` | `transports.internet.serverAddress` (and `authToken` if required), or `connectInternetLater: true` | Apps with a relay/WebSocket back end |
| `nostrMesh` | BLE + Nostr on | (core defaults) | `transports.nostr.relayUrls` | Nostr bootstrap path; needs MLS identity (`encryption` default on) |
| `developmentCleartext` | BLE on | `encryption.enabled: false` | Nothing beyond ids | **Dev / lab only.** Production must use default fail-closed encryption |

## What `forScenario` refuses

`start()` connects the internet transport only when it has a `serverAddress`,
and Nostr only when it has at least one relay URL. Without them the engine
starts and `start()` leaves the transport down without an error. `forScenario`
therefore throws instead of returning such a config, unless you say the app
connects the relay itself (next section):

- the internet transport enabled without `serverAddress`, and without
  `connectInternetLater`;
- `connectInternetLater` with the internet transport off, or together with a
  `serverAddress` (which `start()` would connect at once);
- Nostr enabled without `relayUrls`, or with `encryption.enabled: false`
  (Nostr needs the identity that MLS initialization creates, and disabling
  encryption skips that initialization);
- a transport entry with no `enabled`, which happens only when an override
  names a transport the preset does not;
- `appId` or `profile` inside `overrides`; they are parameters.

A config with encryption off, from `developmentCleartext` or from an
override, logs a warning each time `forScenario` builds it.

## A relay whose server or token arrives after login

An app that learns its relay address or token only once the user has logged
in keeps the internet transport on at construction (that is what registers
it) and connects it after `start()` with `enableTransport`. Pass
`connectInternetLater: true` so `forScenario` accepts the missing server:

```typescript
import { OfflineProtocol, forScenario } from '@offline-protocol/mesh-sdk';

// From your own login flow, after start().
declare function loggedInRelay(): Promise<{ serverAddress: string; authToken: string }>;

async function startMesh(): Promise<OfflineProtocol> {
  const protocol = new OfflineProtocol(
    forScenario('internetRelay', {
      appId: 'my-chat',
      profile: 'default',
      connectInternetLater: true,
    })
  );
  await protocol.start();

  const { serverAddress, authToken } = await loggedInRelay();
  await protocol.enableTransport('internet', { enabled: true, serverAddress, authToken });
  return protocol;
}
```

`start()` does not restore a relay connected this way: after a restart, call
`enableTransport('internet', ...)` again (see
[react-native-integration.md](react-native-integration.md)).

## Defaults philosophy

Presets intentionally **do not** restate every DORS, relay, or encryption field.
Those defaults live in the Rust core and cross the bridge only when your config
names them. `overrides` merges at every depth: a nested field such as
`transports.internet.serverAddress` or `encryption.pendingQueue.pendingTtlMs`
replaces only itself. Arrays such as `relayUrls` are replaced whole, never
merged element by element.

## Related

| Topic | Document |
|-------|----------|
| Full field reference | [bindings/react-native/README.md](../bindings/react-native/README.md#configuration) |
| DORS tuning | [configuration.md](configuration.md), [dors-configuration.md](dors-configuration.md) |
| Permissions timing | [mobile-permissions.md](mobile-permissions.md) |
| Lifecycle order | [integrator-map.md](integrator-map.md#react-native-lifecycle-ordering) |
