# Offline Protocol SDK Integration Guide (React Native)

This guide covers integrating the Offline Protocol SDK into React Native applications: installation, configuration, transports (BLE, Wi‑Fi Direct, Internet), Dynamic Offline Relay Switch (DORS), **MLS-encrypted group messaging via SDK group methods**, and a **complete API reference** for every function.

---

## 1. Getting Started

### 1.1 Prerequisites

- **React Native** ≥ 0.71 or native Android/iOS projects.
- Xcode 15+ (iOS) / Android Studio Giraffe+ (Android).
- Rust toolchain (nightly not required), Node.js 18+, Yarn or npm.
- BLE support enabled in project entitlements and manifest.
- Optional: Wi‑Fi Direct requires Android 10+ with `android.permission.NEARBY_WIFI_DEVICES`.

### 1.2 Installation

```bash
yarn add @offline-protocol/mesh-sdk
# or
npm install @offline-protocol/mesh-sdk
```

- **iOS**: `cd ios && pod install`. Autolinking handles the native module — no
  manual `pod` entry is needed, and device and simulator builds both work (the
  native binary ships as an XCFramework, so CocoaPods picks the matching slice).
  Upgrading from below 0.20.0: delete any manual `pod 'MeshSdk', ...` line first.
- **Android**: Ensure `minSdkVersion ≥ 24` and Kotlin 1.8+.

See `bindings/react-native/README.md` for existing native projects.

---

## 2. Initialising the SDK

Create one `OfflineProtocol` instance at app start, configure transports, and register event listeners.

```ts
import { OfflineProtocol, MessagePriority } from '@offline-protocol/mesh-sdk';

const protocol = new OfflineProtocol({
  appId: 'com.example.app',
  profile: 'default',
  network: { initialTtl: 8 },
  transports: {
    ble: { enabled: true },
    wifiDirect: { enabled: Platform.OS === 'android' },
    internet: { enabled: true, serverAddress: 'wss://mesh.example.com/socket', authToken: 'string' },
  },
  dors: {
    preferOnline: false,
    switchHysteresis: 18,
    switchCooldownSecs: 30,
    bleToWifiRetryThreshold: 2,
    rssiSwitchThreshold: -85,
    congestionQueueThreshold: 40,
    stabilityWindowSecs: 10,
    poorSignalDurationSecs: 12,
    ttlEscalationThreshold: 3,
    congestionDurationSecs: 12,
    ttlEscalationHoldSecs: 25,
    historyWindowSize: 12,
    queueRecoveryRatio: 0.4,
  },
  relay: {
    allowRelay: true,
    minBatteryForRelay: 35,
    relayPriority: 'auto',
  },
});

protocol.on('message_received', evt => {
  console.log(`New message ${evt.message_id} from ${evt.sender}`);
});

await protocol.start();
```

Call `await protocol.stop()` (and optionally `await protocol.destroy()`) on teardown.

---

## 3. Offline vs Hybrid Modes

| Scenario                 | Recommended `dors.preferOnline` | Enabled Transports                    |
|-------------------------|----------------------------------|---------------------------------------|
| Fully offline mesh      | `false`                          | BLE + Wi‑Fi Direct                     |
| Hybrid (online-first)   | `true`                           | Internet (primary), BLE, Wi‑Fi Direct |
| Emergency response mesh | `false` + aggressive hysteresis | BLE + Wi‑Fi Direct                     |

**Offline**: set `internet: { enabled: false }`, bump `initialTtl` for sparse networks.

**Hybrid**: keep Internet enabled; DORS uses it first and falls back to BLE/Wi‑Fi Direct.

```ts
transports: {
  internet: { enabled: true, autoReconnect: true, serverAddress: 'wss://...' },
  ble: { enabled: true },
  wifiDirect: { enabled: true, autoAccept: true },
}
```

---

## 4. Enabling Transports

### 4.1 BLE

- **Permissions**: Bluetooth, Bluetooth Advertise/Connect/Scan (Android 12+), Location (Android), `NSBluetoothAlwaysUsageDescription` (iOS).
- BLE scanning/advertising and fragmentation are handled automatically; 512-fragment cap per message.

### 4.2 Wi‑Fi Direct (Android)

- Permissions: `NEARBY_WIFI_DEVICES` (Android 13+), `ACCESS_FINE_LOCATION`, `CHANGE_WIFI_STATE`.
- Config: `wifiDirect: { enabled: true, autoAccept: true, groupOwnerIntent: 10 }`.

### 4.3 Internet

- Set `serverAddress` (WebSocket URL). With `autoReconnect: true`, the SDK reconnects automatically.
- Use `preferOnline` to prefer Internet; DORS falls back when the socket is unavailable.

---

## 5. DORS Tuning

| Parameter                    | Purpose                                           | Default |
|-----------------------------|---------------------------------------------------|---------|
| `switchHysteresis`          | Min score delta to switch transports             | 15      |
| `switchCooldownSecs`        | Min time between switches                         | 20s     |
| `bleToWifiRetryThreshold`   | Retries before escalating to Wi‑Fi Direct        | 2       |
| `rssiSwitchThreshold`        | RSSI (dBm) for escalation                         | -85     |
| `congestionQueueThreshold`  | Queue depth for congestion                        | 50      |
| `historyWindowSize`         | Samples for smoothing (1–100)                     | 10      |
| `queueRecoveryRatio`        | Queue ratio indicating recovery (0–1)             | 0.5     |

See `docs/dors-configuration.md` and `docs/configuration.md` for full parameters.

---

## 6. Lifecycle & Store-and-Forward

- `protocol.pause()` / `protocol.resume()` for background transitions.
- Outbox, retry queue, and ACK tracking use exponential backoff; the native layer calls `process()` periodically.
- Use `getTransportMetrics('ble')`, `getRetryQueueSize()`, `getPendingAckCount()` to monitor health.

### 6.1 Teardowns your app did not initiate (Android)

On Android the mesh keep-alive notification carries a **Stop** action. When the user taps it, the SDK tears down the transports, the process scheduler, the keep-alive service and the protocol core, then emits `mesh_stopped_by_user`. An app that tracks "mesh active" itself **must** reconcile on this event, or it will keep reporting an active mesh against a protocol that is fully stopped. There is no iOS equivalent — the notification affordance is Android-only.

```ts
sdk.on('mesh_stopped_by_user', () => {
  setMeshActive(false); // a teardown you did not ask for; the SDK is already down
});
```

**Delivery, and what it does not promise.** `mesh_stopped_by_user` and `internet_session_superseded` are *one-shot*: nothing else ever restates them, so all three layers work to keep them from being lost. **Delivery is at-least-once, and the events are state, not edges.**

- **Android** holds a copy of an emit JS could not take and redelivers it on your next event subscription or app foreground.
- **iOS** re-derives `internet_session_superseded` from the live transport latch on every app foreground, for as long as the session stays superseded. (`mesh_stopped_by_user` has no iOS counterpart at all — the notification affordance is Android-only.)
- **The JS layer, on both platforms,** holds a one-shot event that arrived before you had a listener for it, and delivers it to the first `on(...)` that registers. This is a separate gap from the two above and only the SDK can see it: the SDK subscribes to the native emitter inside its own constructor, so between `new OfflineProtocol(...)` and your first `on(...)` there is a window where events arrive with nothing registered — and Android's redelivery lands in exactly that window, because it fires on that constructor-time subscribe. Replay is asynchronous (a microtask), so a handler never runs before the `on(...)` that registered it returns.

Four consequences worth designing against:

- **Handlers must be idempotent.** Every one of these mechanisms can deliver the same fact more than once — Android by redelivering a held copy, iOS by restating on each foreground until the transport is re-enabled, the JS layer by replaying to a late listener. Set a flag; do not push a screen or fire a notification per event.
- **They can arrive late.** Treat them as "this is true", not "this just happened" — reconcile against actual state rather than assuming the event is fresh.
- **Register listeners before `start()`.** The JS hold makes an `await` between construction and your first `on(...)` survivable, but only for these two tags — every other event in that window is dropped, correctly, because it is periodic or re-derivable. Registering synchronously right after construction keeps the window at zero and is still the right habit. (The SDK warns once per event type if events arrive while you have registered no listeners at all.)
- **They are not durable.** No mechanism survives a process kill, and neither the Android hold nor the JS hold survives a JS reload. Persisting would not help: if the process was killed, the event was never generated in the first place.

The JS hold is also cleared where continuing to hold would be *worse* than dropping — redelivering a stale one-shot is the same failure inverted, not a milder one. A held `mesh_stopped_by_user` replayed after you called `start()` would report a mesh that is coming up as down, with nothing to correct it, so `start()` discards anything no listener has claimed by then; `enableTransport('internet', ...)` discards a held `internet_session_superseded`, because that call is what clears the latch the event reports; and `destroy()` discards whatever is left, so an instance you destroy and start again cannot hand the previous session's event to the next session's first listener.

**So reconcile on foreground regardless.** This is the belt-and-braces every integrator should have on both platforms. It takes three reads, because the events report different things and no single call covers them:

```ts
import { ProtocolState } from '@offline-protocol/mesh-sdk';

// on app foreground (AppState 'active')
const state = await sdk.getState();
if (state !== ProtocolState.Running) {
  setMeshActive(false); // reconcile whatever local "mesh active" flag you keep
}

if (await sdk.isInternetSuperseded()) {
  // Another device took the relay slot. This will NEVER reconnect on its own.
  setRelayConnected(false);
  promptReconnectElsewhere();          // your "connected elsewhere" affordance
} else if (!(await sdk.isInternetReady())) {
  setRelayConnected(false);            // an ordinary drop; it reconnects itself
}
```

`getState()` reads the live protocol state, so it stays correct after a notification Stop, after a sticky service restart, and after any teardown the app did not drive — including one that took the whole process with it, where a fresh module reports `Stopped` because there is no protocol yet. It says nothing about the relay: after a supersede the protocol is still `Running`, because only the relay session was displaced.

`isInternetSuperseded()` is the relay half, and it is the read that resolves the ambiguity `isInternetReady()` structurally cannot: a `false` from an ordinary disconnect — which reconnects itself within seconds — and a `false` from a supersede — which will **not** reconnect on its own, ever — are identical there. Check the supersede first and fall back to readiness, as above. Recovery from a supersede is always a deliberate re-enable: `enableTransport('internet', ...)` clears the latch, which stops the iOS restatement and drops any copy Android is still holding, so a notice about the session you just replaced cannot arrive after you are reconnected.

Because `isInternetSuperseded()` reads the latch itself rather than a delivery, it is also the only thing that covers the windows no in-memory delivery reaches — a JS reload, a process restart, or an instance whose hold `start()` has since swept. **An app that reconciles against it on foreground needs nothing from the event but the prompt**, and that is the shape we recommend.

### 6.2 Process death, and why the mesh does not resume itself (Android)

Android can kill your process while mesh is running — memory pressure is the usual reason, and a foreground service makes it less likely, not impossible. The keep-alive service is `START_STICKY`, so the system hands the service back afterwards, **but the SDK never re-creates the protocol from there.** By default, if nothing in the new process has brought a mesh back up by the time the re-delivered intent lands, the service stops itself, so no "Mesh Active" notification outlives the protocol it advertises. (An app that boots React Native from `Application.onCreate` can win that race and have a mesh running already — then the service keeps the notification it is holding for the mesh *your app* started.)

This is a decision, not a missing feature. A protocol re-created with no JavaScript context behind it is worse than one that is simply down: the receive path sends a delivery ACK *before* it emits `message_received`, that ACK makes the sender retire the message from its outbox, and the event is then dropped because nothing is subscribed. The message is gone, and its sender was told it arrived. Staying down keeps the failure recoverable — the sender's outbox holds for up to seven days, retries, parks, and pushes, and delivers once this device is genuinely running again.

**You can opt in to having the mesh come back on its own** — see §6.3. It does not weaken any of the above: nothing native re-creates the protocol there either. It starts *JavaScript* first, so a receiver exists before a protocol does, and your own code decides what happens next.

**What your app should do.** Nothing on Android's behalf, but treat mesh state as something to reconcile at launch rather than assume:

```ts
// on app start, after your own "mesh should be on" preference says yes
const state = await sdk.getState();
if (state !== ProtocolState.Running) {
  await sdk.start();
}
```

**`start()` only restores the transports it can see.** It re-enables the ones declared in the constructor config the instance still holds — BLE, and `transports.internet` / `nostr` / `reticulum` — and nothing else. Anything you enable out of band has to be re-issued by you: **Wi‑Fi Direct always**, because `start()` never starts it, and **the relay** whenever the `serverAddress` or `authToken` reaches the SDK through `enableTransport('internet', ...)` rather than through `transports.internet`. `Running` is not a claim that any particular transport is attached — `getActiveTransports()` is the read that answers that.

Two more things to know if you restart the SDK yourself:

- **Never reuse a `destroy()`ed instance.** `destroy()` removes the event subscriptions, and only the constructor creates them — a destroyed instance that is `start()`ed again will run but deliver zero events. Construct a new `OfflineProtocol`.
- **Nothing is queued for you while the process is dead.** The one-shot event delivery described in §6.1 is in-memory on both platforms; a process kill loses it. That is not a gap — if the process was killed, the event was never generated. For the relay case there is a durable read regardless: `isInternetSuperseded()` reports the transport's own latch, so a restarted process that re-enables the relay and is displaced again learns it the same way.

### 6.3 Restoring the mesh automatically after a process kill (Android, opt-in)

Reconciling on foreground (§6.2) closes the window as soon as the user opens your app. For an always-on app that is not always soon enough — the device is off the mesh for the whole interval. If you want it back sooner, the SDK can wake JavaScript when the keep-alive service is restarted, and let *your code* bring the mesh up.

**This is opt-in, and the reason is the obligation in step 3 below.** Waking JavaScript is only safe for an app that durably stores what it receives, and only you know whether yours does.

**1. Declare the flag** in your app's `AndroidManifest.xml`, inside `<application>`:

```xml
<meta-data android:name="com.offlineprotocol.MESH_WAKE_ENABLED" android:value="true" />
<!-- Optional; default 60, clamped to 10–300. -->
<meta-data android:name="com.offlineprotocol.MESH_WAKE_TIMEOUT_SECONDS" android:value="60" />
```

**2. Register the task at module scope** in `index.js` — next to `AppRegistry.registerComponent`, not inside a component, which will not have mounted:

```js
import { AppRegistry } from 'react-native';
import { registerMeshWakeTask } from '@offline-protocol/mesh-sdk';
import App from './App';

AppRegistry.registerComponent('MyApp', () => App);

registerMeshWakeTask(async () => {
  if (getLiveProtocol()) return;             // already running — nothing to do
  const config = await loadSavedConfig();    // your storage, your credentials
  if (!config) return;                       // logged out — stay down
  const protocol = new OfflineProtocol(config);
  protocol.on('message_received', persistMessage);   // BEFORE start()
  await protocol.start();
  await protocol.enableTransport('internet', { serverAddress, authToken });
});
```

**3. Store what you receive, durably, before `start()`.** The core never persists inbound content, and the receive path ACKs a message before it emits it. A handler registered after `start()`, or one that only sets React state, loses the message *and* has already told its sender it arrived. This is the same trap §6.2 describes, and opting in is you taking responsibility for not walking into it.

Three more things the task has to get right:

- **Be idempotent.** The task is allowed to run while your app is in the foreground — the alternative is React Native crashing the process when the user opens the app mid-wake — so it can find a protocol already live. Return early instead of building a second one.
- **Re-issue what `start()` does not restore.** Wi‑Fi Direct always, and the relay whenever its `serverAddress`/`authToken` arrive through `enableTransport('internet', ...)`. Same list as §6.2.
- **Resolve promptly.** The keep-alive holds the process; the task does not need to. Past its budget React Native terminates it.

**If the wake does not land, the keep-alive stops itself.** A task that was never registered, failed to boot, threw, or simply declined all end the same way: a watchdog brings the service down rather than leave a "Mesh Active" notification over a mesh that is not running. Declining is a legitimate outcome — return early and the device goes back to the §6.2 behaviour.

**What it does not cover.** The wake rides the service restart, so it reaches process kills the system chooses to recover from — memory pressure, the usual case. It does not fire after a force-stop from Settings, after the user swipes the app away on OEMs that treat that as a force-stop, or after a reboot; those need the user to open the app, exactly as before. It also requires a foreground promotion to have succeeded on restart, and stops rather than waking if one was refused.

**Version requirement:** React Native **0.76.5+** when the New Architecture is enabled. Headless tasks did not work under bridgeless before 0.76 and were patchy until 0.76.5. On RN 0.84 and 0.85 a core bug (fixed in 0.86) can leave the wake service running after the task finishes; the SDK's timeout bounds it, but 0.86+ is the cleaner target.

---

## 7. Group Messaging (MLS-Encrypted Mesh Groups)

Group features (create groups, send messages, manage members) are provided directly by the SDK's **mesh group methods**. Membership and message encryption are MLS, end to end, and handled entirely by the SDK — **there is no group server that can read your messages**, and no separate service for your app to run. Each method runs against your `OfflineProtocol` instance and delivers over whatever transports DORS has selected.

The relay does play two roles over the Internet transport, neither of which can see plaintext. Groups are **registered** with it so invite links resolve (`group.relayEnabled`, on by default). And a group send over the internet takes one of two paths: against a relay that advertised the `group_delivery_v3` capability, **one O(1) broadcast answered by a settled per-recipient delivery report** — the SDK automatically re-sends a per-member copy to anyone the relay did not reach, and surfaces the result as the `group_message_delivery_report` event; against any other relay (or with `group.relayBroadcastEnabled: false`), **one ordinary message frame per member** — the same MLS ciphertext addressed individually — so every member's copy gets the full direct-message delivery ladder: outbox, ACK, retry, offline push, and park-on-unreachable with presence-driven flush. See [Group sends](message-delivery.md#group-sends) and [Group Configuration](configuration.md#group-configuration), including the JS shape of the `group` config section.

### 7.1 Prerequisites

- MLS must be initialised first (see §11.12), e.g. `await protocol.initializeMlsWithSecureStorage(...)`.
- The creator becomes the group admin; admins manage membership.

### 7.2 Creating and Using Groups

```ts
// Create a group — the creator is the admin. Returns MlsGroupInfo.
const group = await protocol.meshCreateGroup('My Group');

// Invite a member by user ID (admin only). Sends MLS Welcome + Commit.
await protocol.meshInviteToGroup(group.groupId, 'user456');

// Send an encrypted message to all members.
// Signature: meshSendGroupMessage(groupId, content, priority?, replyToMsg?)
await protocol.meshSendGroupMessage(group.groupId, 'Hello group!');

// List groups and fetch info.
const groupIds = await protocol.meshListGroups();
const info = await protocol.meshGetGroupInfo(group.groupId);
```

In the **example React Native app**, **ProtocolProvider** wraps these in context actions (`createGroup` → `meshCreateGroup`, `sendGroupMessage` → `meshSendGroupMessage`, `addGroupMember` → `meshInviteToGroup`, etc.). Screens use `useProtocol()` and call those context methods only. See `examples/react-native-app` and `ProtocolProvider.tsx` for the full pattern.

### 7.3 Group Methods Summary

| Method | Description |
|--------|-------------|
| `meshCreateGroup(name)` | Create group; creator is admin. Returns `MlsGroupInfo`. |
| `meshSendGroupMessage(groupId, content, priority?, replyToMsg?)` | Send encrypted message to all members. |
| `meshInviteToGroup(groupId, inviteeUserId)` | Invite member (admin); sends Welcome + Commit. |
| `meshRemoveFromGroup(groupId, memberId)` | Remove member (admin). |
| `meshLeaveGroup(groupId)` | Current user leaves the group. |
| `meshListGroups()` | List all group IDs. |
| `meshGetGroupInfo(groupId)` | Get group metadata (members, epoch). |
| `meshRenameGroup(groupId, newName)` | Rename group (admin); broadcasts to members. |

Member roles (admin/member) are managed with `meshSetMemberRole` / `meshGetMemberRole` / `meshGetGroupRoles`. See §11.13–§11.14 for the complete reference.

### 7.4 Reply-to (Threaded Messages)

Store the message ID when the user taps "reply", then pass it as the `replyToMsg` argument:

```ts
await protocol.meshSendGroupMessage(groupId, content, null, replyToMessageId);
```

Render replies in the UI by resolving `replyToMsg` to the original message.

---

## 8. Surfacing Metrics

```ts
const metrics = await protocol.getTransportMetrics('ble');
// { packetsSent, packetsReceived, bytesSent, bytesReceived, errorRate, avgLatencyMs }
```

Subscribe to `transport_switched`, `relay_promoted`, `network_metrics` for DORS and relay status in the UI.

---

## 9. Testing Checklist

1. BLE/Wi‑Fi Direct permissions granted before start.
2. `cargo test -p offline-protocol-router` and `cargo test -p offline-protocol` for core logic.
3. Example app (`examples/react-native-app`) for device scenarios and Control Center.
4. For groups: MLS initialised, then `meshCreateGroup` / `meshSendGroupMessage` / reply-to work; invite, remove member, leave, rename (admin).

---

## 10. Production Tips

- Run `cargo fmt`, `cargo clippy --workspace --all-targets -- -D warnings` and test suites before release.
- Use `docs/dors-configuration.md` and `docs/configuration.md` for tuning.
- Encrypt sensitive payloads at the application layer; the SDK carries bytes without inspecting them.
- Monitor battery when using Wi‑Fi Direct; DORS deprioritises high-power transports when battery is low.

---

## 11. Complete API Reference

All methods are on the `OfflineProtocol` class. Types and events are exported from `@offline-protocol/mesh-sdk`.

### 11.1 Constructor & Events

| Method | Signature | Description |
|--------|-----------|-------------|
| **constructor** | `new OfflineProtocol(config: ProtocolConfig)` | Creates an instance. Does not start the protocol. |
| **on** | `on(eventType: EventType \| 'all', listener: EventListener): this` | Registers an event listener. |
| **off** | `off(eventType, listener): this` | Removes an event listener. |
| **once** | `once(eventType, listener): this` | Registers a one-time listener. |
| **removeAllListeners** | `removeAllListeners(eventType?: EventType \| 'all'): this` | Removes listeners for a type or all. |

**Event types**: `message_sent`, `message_received`, `message_delivered`, `message_failed`, `transport_switched`, `relay_promoted`, `relay_demoted`, `neighbor_discovered`, `neighbor_lost`, `network_metrics`, `file_progress`, `file_received`, `diagnostic`, `secure_session_established`, `secure_session_failed`, `connection_request_received`, `connection_request_undeliverable`, `connection_accepted`, `connection_rejected`, `connection_request_cancelled`, `group_created`, `group_message_received`, `group_member_added`, `group_member_removed`, `group_unauthorized_membership_change`, `group_message_sent`, `group_message_partial_failure`, `group_message_delivery_report`, `group_epoch_fork_detected`, `group_epoch_fork_resolved`, `group_role_changed`, `service_discovered`, `service_request_received`, `service_response_received`, `presence_updated`, `typing_indicator_received`, `read_receipt_received`, `message_relayed`, `message_deferred`.

**Identity**: `neighbor_discovered.peer_id` is the peer's canonical address — the `off1…` value that peer derived from its own identity key, on every transport — and is what you pass as `recipient` to `sendMessage` / `sendConnectionRequest`. Your own is `localAddress()`; no app chooses its address, so a peer claiming one can be checked by re-deriving it from the key it presents.

---

### 11.2 Lifecycle

| Method | Signature | Description |
|--------|-----------|-------------|
| **start** | `start(): Promise<void>` | Creates native instance if needed, starts protocol, auto-initialises MLS if encryption enabled, auto-enables Internet if configured. |
| **stop** | `stop(): Promise<void>` | Stops the protocol and BLE operations. |
| **emitTestEvent** | `emitTestEvent(): Promise<void>` | Emits a test `network_metrics` event (debug). |
| **destroy** | `destroy(): Promise<void>` | Removes listeners, unsubscribes from native events, destroys native instance. |

---

### 11.3 Messaging

| Method | Signature | Description |
|--------|-----------|-------------|
| **sendMessage** | `sendMessage(params: SendMessageParams): Promise<string>` | Sends a message; returns message ID. `params`: `recipient`, `content`, `priority?`, `replyToMsg?`, plus rich params `contentType?`, `replyContext?`, `mediaMetadata?`, `forwardInfo?`. |
| **receiveMessage** | `receiveMessage(): Promise<MessageReceivedEvent \| null>` | Polls for the next received message. |
| **sendConnectionRequest** | `sendConnectionRequest(params: SendConnectionRequestParams): Promise<string>` | Sends a connection request; returns the message ID that correlates all outcome events. `params`: `recipient` (the target's `off1…` address), `senderName`, `keyPackage?`, `initialMessage?`. |
| **acceptConnectionRequest** | `acceptConnectionRequest(params): Promise<string>` | Accepts a received request. `params`: `recipient`, `accepterName`, `keyPackage?`. |
| **rejectConnectionRequest** | `rejectConnectionRequest(params): Promise<string>` | Rejects a received request. `params`: `recipient`. |
| **cancelConnectionRequest** | `cancelConnectionRequest(params): Promise<string>` | Cancels a request you sent. `params`: `recipient`. |

Rich params (`replyContext`, `mediaMetadata`, `forwardInfo`, a non-default `contentType`) only ever travel sealed inside the MLS ciphertext, toward recipients whose SDK advertised rich-payload support; toward anyone else they are silently dropped — never sent cleartext — while `replyToMsg` threading survives. When any rich param is present the call routes to the native `sendMessageRich` method.

> **Over-the-air JS updates (CodePush-style):** `sendMessageRich` and `sendMediaRich` are native methods. A JS-only update that starts passing rich params against an older native binary fails those calls (method not found); plain `sendMessage`/`sendMedia` calls are unaffected. Gate rich params on your native binary version. Note the routing triggers below: for `sendMedia`, cloud/sticker `mediaMetadata` fields alone are enough to route to the native rich method.

Connection-request failure contract: recipient offline emits `connection_request_undeliverable` (`reason` starts with `recipient_unreachable`); retry exhaustion emits it with `reason: 'max_retries_exceeded'` alongside the generic `message_failed`. Both carry the message ID returned by `sendConnectionRequest`. Answers arrive as `connection_accepted` / `connection_rejected`, correlated by peer id (`accepted_by` / `rejected_by`), not message ID.

---

### 11.4 Transports & BLE

| Method | Signature | Description |
|--------|-----------|-------------|
| **getActiveTransports** | `getActiveTransports(): Promise<TransportType[]>` | Returns active transport types. |
| **enableTransport** | `enableTransport(type, config?): Promise<void>` | Enables a transport; optional config for Internet/Wi‑Fi Direct. |
| **disableTransport** | `disableTransport(type: TransportType): Promise<void>` | Disables a transport. |
| **isBluetoothEnabled** | `isBluetoothEnabled(): Promise<boolean>` | Whether Bluetooth is on. |
| **requestEnableBluetooth** | `requestEnableBluetooth(): Promise<boolean>` | Requests user to enable Bluetooth (Android shows dialog; iOS returns false). |
| **getBLePeerCount** | `getBLePeerCount(): Promise<number>` | Number of discovered BLE peers. |
| **getTransportMetrics** | `getTransportMetrics(transportType): Promise<{...} \| null>` | Metrics: packetsSent/Received, bytesSent/Received, errorRate, avgLatencyMs. |
| **forceTransport** | `forceTransport(transportType): Promise<void>` | Forces use of a transport (overrides DORS). |
| **releaseTransportLock** | `releaseTransportLock(): Promise<void>` | Releases transport lock so DORS decides again. |

---

### 11.5 State, Battery & Relay

| Method | Signature | Description |
|--------|-----------|-------------|
| **getState** | `getState(): Promise<ProtocolState>` | Current state: Stopped, Running, Paused. |
| **pause** | `pause(): Promise<void>` | Pauses the protocol. |
| **resume** | `resume(): Promise<void>` | Resumes from pause. |
| **setBatteryLevel** | `setBatteryLevel(level: number): Promise<void>` | Reports battery level (0–100). Prefer `setBatteryState`. |
| **setBatteryState** | `setBatteryState(level: number, isCharging: boolean): Promise<void>` | Reports battery level and charging state. |
| **getBatteryLevel** | `getBatteryLevel(): Promise<number \| null>` | Last reported battery level, or null if never reported. |
| **getIsCharging** | `getIsCharging(): Promise<boolean>` | Last reported charging state. |
| **setRelayPriority** | `setRelayPriority(priority: 'never' \| 'auto' \| 'always'): Promise<void>` | Sets relay priority. |
| **getRelayPriority** | `getRelayPriority(): Promise<'never' \| 'auto' \| 'always'>` | Current relay priority. |
| **updateRelayConfig** | `updateRelayConfig(config: RelayConfig): Promise<void>` | Updates relay config at runtime; omitted fields keep their values. |
| **getRelayConfig** | `getRelayConfig(): Promise<Required<RelayConfig>>` | Current relay configuration. |
| **isRelay** | `isRelay(): Promise<boolean>` | Whether this device is currently carrying traffic for other devices. Reports observed forwarding, not capability, so it stays `false` on a device whose peers can all reach each other — and on any device with a working internet relay, since the mesh is only offered frames nothing else can deliver. |

> **The battery feed is required for relay behaviour to work at all.** No
> transport can observe the host's battery, so until `setBatteryState` (or
> `setBatteryLevel`) is called, DORS energy scoring, the message-forwarding
> battery floor, and the capability bias that makes a healthier device carry
> more of the mesh's traffic all run in their unknown-level branch — the
> device stays willing to relay at any charge, and at full effort. Call it on
> start and on each platform battery notification.
>
> The relay events are not affected: `relay_promoted` / `relay_demoted` report
> traffic this device has actually carried, so they fire with or without a
> reading.
>
> Report charging state where the platform provides it: a charging device is
> deliberately excused the soft `minBatteryForRelay` floor, so reporting the
> level alone strips relay duty from plugged-in devices that should keep it.

---

### 11.6 Topology & Message Stats

| Method | Signature | Description |
|--------|-----------|-------------|
| **getTopology** | `getTopology(): Promise<NetworkTopology>` | Current network topology snapshot. |
| **getMessageStats** | `getMessageStats(): Promise<MessageDeliveryStats[]>` | Message delivery statistics. |
| **getDeliverySuccessRate** | `getDeliverySuccessRate(): Promise<number>` | Success rate (0–1). |
| **getMedianLatency** | `getMedianLatency(): Promise<number \| null>` | Median delivery latency (ms). |
| **getMedianHops** | `getMedianHops(): Promise<number \| null>` | Median hop count. |

---

### 11.7 DORS & Reliability

| Method | Signature | Description |
|--------|-----------|-------------|
| **updateDorsConfig** | `updateDorsConfig(config): Promise<void>` | Updates DORS parameters at runtime (values clamped). |
| **getDorsConfig** | `getDorsConfig(): Promise<{...}>` | Current DORS configuration. |
| **updateAckConfig** | `updateAckConfig(config: AckConfig): Promise<void>` | Updates ACK config. |
| **updateRetryConfig** | `updateRetryConfig(config: RetryConfig): Promise<void>` | Updates retry config. |
| **updateDedupConfig** | `updateDedupConfig(config: DedupConfig): Promise<void>` | Updates dedup config. |
| **getDedupStats** | `getDedupStats(): Promise<DedupStats>` | Deduplicator statistics. |
| **getPendingAckCount** | `getPendingAckCount(): Promise<number>` | Number of pending ACKs. |
| **getRetryQueueSize** | `getRetryQueueSize(): Promise<number>` | Retry queue size. |
| **shouldEscalateToWifi** | `shouldEscalateToWifi(): Promise<boolean>` | Whether DORS recommends escalating to Wi‑Fi Direct. |
| **getMeshRelayStats** | `getMeshRelayStats(): Promise<MeshRelayStats>` | What this device has carried for other people (cumulative). |
| **getMeshRelayTunables** | `getMeshRelayTunables(): Promise<MeshRelayTunables>` | Mesh forwarding tunables in force, read from the governor. Every field populated. |

---

### 11.8 Gradient Routing

| Method | Signature | Description |
|--------|-----------|-------------|
| **learnRoute** | `learnRoute(destination, nextHop, hopCount, quality, sequenceNumber?): Promise<void>` | Records a route from an incoming message. `sequenceNumber` is DSDV-style (default 0); pass 0 when the message does not carry one. Negative values are clamped to 0. |
| **getBestRoute** | `getBestRoute(destination): Promise<{nextHop, hopCount, quality, lastSeenMs} \| null>` | Best route to destination. |
| **getAllRoutes** | `getAllRoutes(destination): Promise<Array<{...}>>` | All valid routes to destination. |
| **hasRoute** | `hasRoute(destination): Promise<boolean>` | Whether any route exists. |
| **removeNeighborRoutes** | `removeNeighborRoutes(neighborId): Promise<void>` | Removes routes through a neighbor. |
| **cleanupExpiredRoutes** | `cleanupExpiredRoutes(): Promise<void>` | Removes expired routes. |
| **getRoutingStats** | `getRoutingStats(): Promise<{destinationCount, routeCount}>` | Routing table stats. |
| **updateRoutingConfig** | `updateRoutingConfig(config): Promise<void>` | Updates routing config (maxRoutesPerDestination, routeTtlSecs, maxRoutingTableSize). |

---

### 11.9 File Transfer

| Method | Signature | Description |
|--------|-----------|-------------|
| **sendFile** | `sendFile(params: SendFileParams): Promise<string>` | Sends a file; returns file ID. `params`: filePath, recipient, fileName?. |
| **sendMedia** | `sendMedia(params: SendMediaParams): Promise<string>` | Sends media bytes; returns file ID. `params`: `recipient`, `fileData` (base64), `fileName`, `contentType`, `mediaMetadata?`, plus sealed-only rich params `caption?`, `replyToMsg?`, `replyContext?`, `forwardInfo?`, and `fileId?` (for answering `media_resend_required`). Rich params — including the cloud/sticker `mediaMetadata` fields (`mediaId`, `downloadUrl`, `encryptionKey`, …), even with no other rich param — route to the native `sendMediaRich` method (same over-the-air-update caveat as `sendMessageRich`) and travel sealed with chunk 0 — dropped, never cleartext, toward non-rich recipients. |
| **getFileProgress** | `getFileProgress(fileId): Promise<FileProgress \| null>` | Progress for a file transfer. |
| **cancelFileTransfer** | `cancelFileTransfer(fileId): Promise<boolean>` | Cancels a transfer. |
| **processFileChunk** | `processFileChunk(fileId, chunkIndex, data: number[]): Promise<void>` | Processes a file chunk (custom handling). |
| **finalizeFile** | `finalizeFile(fileId): Promise<void>` | Finalises file after all chunks processed. |

---

### 11.10 Wi‑Fi Direct (Low-Level)

| Method | Signature | Description |
|--------|-----------|-------------|
| **wifiDirectStatusChanged** | `wifiDirectStatusChanged(isConnected: boolean): Promise<void>` | Notifies protocol of Wi‑Fi Direct connection state. |
| **wifiDirectMessageReceived** | `wifiDirectMessageReceived(senderId, data: number[]): Promise<void>` | Incoming Wi‑Fi Direct message. |
| **wifiDirectGetNextMessage** | `wifiDirectGetNextMessage(): Promise<{recipientId, data} \| null>` | Next outgoing Wi‑Fi Direct message. |
| **wifiDirectPeerConnected** | `wifiDirectPeerConnected(peerId): Promise<void>` | Peer connected. |
| **wifiDirectPeerDisconnected** | `wifiDirectPeerDisconnected(peerId): Promise<void>` | Peer disconnected. |

---

### 11.11 Internet Transport (Low-Level)

| Method | Signature | Description |
|--------|-----------|-------------|
| **internetStatusChanged** | `internetStatusChanged(isConnected: boolean): Promise<void>` | Notifies protocol of internet connection state. |
| **internetMessageReceived** | `internetMessageReceived(senderId, data: number[]): Promise<void>` | Incoming internet message. `senderId` asserts the peer is *reachable* (it drives outbox flush, Welcome re-arm, auto key exchange, and `neighbor_discovered`), so pass `""` for locally synthesized frames that no peer transmitted, and build those with `requires_ack: false`. A placeholder id here becomes a phantom peer the SDK repeatedly tries to message. |
| **internetGetNextMessage** | `internetGetNextMessage(): Promise<{messageId, recipientId, data} \| null>` | Next outgoing internet message. Use `messageId` to confirm or report failure. |
| **internetConfirmSent** | `internetConfirmSent(messageId: string): Promise<void>` | Confirms a message was sent over the wire. Call after WebSocket send succeeds. |
| **internetSendFailed** | `internetSendFailed(messageId: string): Promise<void>` | Reports a message failed to send. Call when WebSocket send fails. |
| **sendRawServerCommand** | `sendRawServerCommand(json: string): Promise<boolean>` | Sends a complete application-owned relay command verbatim. Responses arrive as `internet_server_message`. |

#### Lossless group snapshot extensions

Relay `GroupInfo` and `UserGroups` frames are dual-emitted:

- `group_info` and `user_groups` provide the stable SDK-owned projections for standard group state.
- `internet_server_message` carries the original relay frame in its `json` string, including application-owned fields such as `description`, `avatar_url`, `pending_join_requests`, profile/membership data, and unknown future extensions.

Parse the raw event and check the server frame's `type` before consuming extension fields. Do not apply standard group state from both streams, and do not rely on typed/raw arrival order. Raw snapshots can include invite tokens, profile data, and key packages, so avoid logging them indiscriminately.

---

### 11.12 MLS (End-to-End Encryption)

| Method | Signature | Description |
|--------|-----------|-------------|
| **initializeMlsWithSecureStorage** | `initializeMlsWithSecureStorage(): Promise<void>` | Initialises MLS key material in iOS Keychain / Android EncryptedSharedPreferences and message-plane state in the app container. Called automatically by `start()` when encryption enabled. |
| **wipePersistedState** | `wipePersistedState(appId, profile): Promise<void>` | Erases all persisted state for one account — namespaced secure store, protocol-state directory, and the pre-namespace store when this account owns it or nobody does. Call on logout/username switch, **after** `destroy()`; rejects if the account named is the one currently running. Irreversible, and rotates the MLS and Nostr identities. See [UPGRADING §10](./UPGRADING.md#logging-out-and-switching-accounts). |
| **isMlsInitialized** | `isMlsInitialized(): Promise<boolean>` | Whether MLS is ready. |
| **mlsGenerateKeyPackage** | `mlsGenerateKeyPackage(): Promise<MlsKeyPackage>` | Generates a new key package. |
| **mlsGetOrCreateKeyPackage** | `mlsGetOrCreateKeyPackage(): Promise<MlsKeyPackage>` | Gets or creates key package. |
| **mlsGetPendingKeyPackages** | `mlsGetPendingKeyPackages(): Promise<MlsKeyPackage[]>` | Pending key packages not yet synced. |
| **mlsMarkKeyPackageSynced** | `mlsMarkKeyPackageSynced(packageId): Promise<void>` | Marks key package as synced. |
| **mlsImportKeyPackage** | `mlsImportKeyPackage(userId, keyPackageData: number[]): Promise<void>` | Imports another user's key package. |
| **mlsHasSession** | `mlsHasSession(otherUserId): Promise<boolean>` | Whether an MLS session exists with that user. |
| **hasPendingKeyPackage** | `hasPendingKeyPackage(peerId): Promise<boolean>` | Whether a pending key package is available for the peer. |
| **establishSecureSession** | `establishSecureSession(peerId): Promise<MlsWelcome \| null>` | High-level: establishes session (uses pending key package if available); returns Welcome or null if session exists. |
| **mlsCreateSession** | `mlsCreateSession(otherUserId): Promise<MlsWelcome>` | Creates session (requires key package already imported). |
| **mlsJoinSession** | `mlsJoinSession(welcome: MlsWelcome): Promise<MlsSessionInfo>` | Joins session from Welcome message. |
| **mlsEncryptForUser** | `mlsEncryptForUser(otherUserId, plaintext: number[]): Promise<MlsEncryptedMessage>` | Encrypts for user (creates session if needed). |
| **mlsDecryptFromUser** | `mlsDecryptFromUser(encrypted): Promise<number[] \| null>` | Decrypts message from user. |
| **mlsDecrypt** | `mlsDecrypt(encrypted): Promise<number[] \| null>` | Decrypts any MLS message (1:1 or group). |
| **mlsListSessions** | `mlsListSessions(): Promise<string[]>` | User IDs with active sessions. |
| **mlsDeleteSession** | `mlsDeleteSession(otherUserId): Promise<void>` | Deletes session with user. |
| **mlsProcessWelcome** | `mlsProcessWelcome(welcome): Promise<MlsSessionInfo \| MlsGroupInfo>` | Processes Welcome (session or group). |

---

### 11.13 Mesh Group Messaging (MLS-Encrypted)

High-level group methods that handle MLS encryption and per-member fan-out automatically. `meshSendGroupMessage` resolves to **one message id per recipient** — each is a separately tracked frame with its own outbox entry, ACK, and retry ladder ([Group sends](message-delivery.md#group-sends)).

| Method | Signature | Description |
|--------|-----------|-------------|
| **meshCreateGroup** | `meshCreateGroup(groupName): Promise<MlsGroupInfo>` | Creates group; creator is admin. |
| **meshInviteToGroup** | `meshInviteToGroup(groupId, inviteeUserId): Promise<void>` | Invites user (admin only); sends Welcome + Commit. |
| **meshSendGroupMessage** | `meshSendGroupMessage(groupId, content, priority?, replyToMsg?): Promise<string[]>` | Sends encrypted message to all members. |
| **meshForwardMessageToGroup** | `meshForwardMessageToGroup(params): Promise<string[]>` | Forwards message to group with attribution. |
| **meshGroupRichReadiness** | `meshGroupRichReadiness(groupId): Promise<GroupRichReadiness>` | Advisory pre-check: `{ ready, unknownMembers }` — whether a rich group send right now would seal its extras, and which members hold the gate closed. When a send's extras do drop, the `group_rich_extras_dropped` event fires (with `unknown_members`) and the SDK probes those members' capability automatically. |
| **meshRemoveFromGroup** | `meshRemoveFromGroup(groupId, memberId): Promise<void>` | Removes member (admin only). |
| **meshLeaveGroup** | `meshLeaveGroup(groupId): Promise<void>` | Leaves group with notification. |
| **meshListGroups** | `meshListGroups(): Promise<string[]>` | Lists all group IDs (excluding 1:1 sessions). |
| **meshGetGroupInfo** | `meshGetGroupInfo(groupId): Promise<MlsGroupInfo \| null>` | Gets group info (members, epoch, etc.). |
| **meshRenameGroup** | `meshRenameGroup(groupId, newName): Promise<void>` | Renames group (admin only); broadcasts to all members. |

---

### 11.14 Group Role Management

| Method | Signature | Description |
|--------|-----------|-------------|
| **meshSetMemberRole** | `meshSetMemberRole(groupId, userId, role): Promise<void>` | Sets role (`"admin"` or `"member"`); admin only. |
| **meshGetMemberRole** | `meshGetMemberRole(groupId, userId): Promise<string>` | Gets member's role. |
| **meshGetGroupRoles** | `meshGetGroupRoles(groupId): Promise<Record<string, string>>` | Gets all roles as `{ userId: role }`. |

**Security invariants:**
- The group creator is automatically `Admin`.
- Only admins can *call* invite, remove, change-role, or rename — these are checked before sending.
- Role changes and renames are additionally enforced on receive: a non-admin's frame is rejected by every peer.
- Membership changes (invite/remove) are **not** enforced on receive. MLS authenticates the committer as a group member, but a member running a modified client can add or remove anyone; the change applies and is reported via the `group_unauthorized_membership_change` event. See [Group authorization model](./mls-integration.md#group-authorization-model).
- The last admin cannot be demoted, removed, or leave (prevents orphaned groups).
- Deterministic election promotes the lexicographically smallest member if the last admin disconnects.

---

### 11.15 Security & Trust

| Method | Signature | Description |
|--------|-----------|-------------|
| **localAddress** | `localAddress(): Promise<string \| null>` | This device's `off1…` address — its `sender` on every frame and what peers pass as `recipient` to reach it. Derived from the identity key in this profile's storage, so the app does not choose it, and stable across restarts of the same `profile`. `null` until startup completes; the `identity_ready` event carries the same value the moment it is known. |
| **blockUser** | `blockUser(userId: string): Promise<void>` | Blocks a user (silently drops their messages). |
| **unblockUser** | `unblockUser(userId: string): Promise<void>` | Unblocks a user. |
| **getBlockedUsers** | `getBlockedUsers(): Promise<string[]>` | Returns all blocked user IDs. |
| **isUserBlocked** | `isUserBlocked(userId: string): Promise<boolean>` | Whether a user is blocked. |

---

### 11.16 Presence, Typing & Read Receipts

| Method | Signature | Description |
|--------|-----------|-------------|
| **sendPresenceUpdate** | `sendPresenceUpdate(recipient, status): Promise<void>` | Sends a presence update (`"online"`, `"away"`, `"offline"`). |
| **sendTypingIndicator** | `sendTypingIndicator(recipient, conversationId, isTyping): Promise<void>` | Sends a typing indicator. |
| **sendReadReceipt** | `sendReadReceipt(recipient, messageIds: string[]): Promise<void>` | Sends read receipts for one or more messages. |

---

### 11.17 Message Forwarding

| Method | Signature | Description |
|--------|-----------|-------------|
| **forwardMessage** | `forwardMessage(params: ForwardMessageParams): Promise<string>` | Forwards a message to a 1:1 recipient with attribution. |

For group forwarding, see `meshForwardMessageToGroup` in [§11.13](#1113-mesh-group-messaging-mls-encrypted).

---

### Need more?

- **Configuration**: `docs/configuration.md` for every tunable parameter.
- **DORS**: `docs/dors-configuration.md` for algorithm details.
- **Mesh**: `docs/mesh.md` for mesh networking and BLE.
- **Architecture**: `docs/architecture.md` for high-level design.
- **Example app**: `examples/react-native-app` and `ProtocolProvider.tsx` for a full setup with UI.
