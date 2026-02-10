# Offline Protocol SDK Integration Guide (React Native)

This guide covers integrating the Offline Protocol SDK into React Native applications: installation, configuration, transports (BLE, Wi‑Fi Direct, Internet), Dynamic Offline Relay Switch (DORS), **group management via WebSocket relay**, and a **complete API reference** for every function.

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

- **iOS**: `cd ios && pod install`
- **Android**: Ensure `minSdkVersion ≥ 24` and Kotlin 1.8+.

See `bindings/react-native/README.md` for existing native projects.

---

## 2. Initialising the SDK

Create one `OfflineProtocol` instance at app start, configure transports, and register event listeners.

```ts
import { OfflineProtocol, MessagePriority } from '@offline-protocol/mesh-sdk';

const protocol = new OfflineProtocol({
  appId: 'com.example.app',
  userId: 'user-42',
  network: { initialTtl: 8 },
  transports: {
    ble: { enabled: true },
    wifiDirect: { enabled: Platform.OS === 'android' },
    internet: { enabled: true, serverAddress: 'wss://mesh.example.com/socket' },
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
    relayThreshold: 3,
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
| Hybrid (Fernweh model)  | `true`                           | Internet (primary), BLE, Wi‑Fi Direct |
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

---

## 7. Group SDK (WebSocket Relay)

Group features (create groups, send messages, manage members) use a **WebSocket relay server**. The SDK **returns JSON strings** that your app **sends over the WebSocket**; the SDK does not open or manage the socket.

### 7.1 Prerequisites

- Relay server implementing the group WebSocket API.
- User identity (e.g. JWT) for `Authenticate`.

### 7.2 Connection and Authentication

```ts
const ws = new WebSocket('wss://your-relay.example.com/ws');

ws.onopen = () => {
  ws.send(JSON.stringify({ type: 'Authenticate', token: AUTH_TOKEN }));
};

ws.onmessage = (event) => {
  const msg = JSON.parse(event.data);
  switch (msg.type) {
    case 'Authenticated':
      setCurrentUser({ userId: msg.user_id, username: msg.username });
      break;
    case 'UserGroups':
      setGroups(msg.groups ?? []);
      break;
    case 'GroupCreated':
    case 'GroupMessage':
    case 'GroupInfo':
    case 'Error':
      // Handle per your relay contract
      break;
  }
};
```

Only after `Authenticated` should you call group methods and send their return values over the WebSocket.

### 7.3 Using Group Methods

Each group method returns a **JSON string**. Send it with `ws.send(payload)`.

```ts
function sendToRelay(jsonString: string) {
  if (ws.readyState === WebSocket.OPEN) ws.send(jsonString);
}

const createPayload = await protocol.groupCreate('My Group');
sendToRelay(createPayload);

const listPayload = await protocol.groupGetUserGroups();
sendToRelay(listPayload);

const msgPayload = await protocol.groupSendMessage(groupId, content, replyToMsgId ?? null);
sendToRelay(msgPayload);
```

### 7.4 Group Methods Summary

| Method | Description |
|--------|-------------|
| `groupCreate(name)` | Create group; creator is admin. |
| `groupSendMessage(groupId, content, replyToMsg?)` | Send message; optional reply-to message ID. |
| `groupAddMember(groupId, username)` | Add member (admin). |
| `groupRemoveMember(groupId, username)` | Remove member (admin or self). |
| `groupSetAdmin(groupId, username)` | Grant admin (admin). |
| `groupRemoveAdmin(groupId, username)` | Revoke admin (admin). |
| `groupLeave(groupId)` | Current user leaves. |
| `groupDelete(groupId)` | Delete group (admin). |
| `groupGetInfo(groupId)` | Get group metadata. |
| `groupGetUserGroups()` | Get all groups for current user. |

Relay responses (e.g. `UserGroups`, `GroupCreated`, `GroupMessage`, `GroupInfo`, `Error`) are received asynchronously in `ws.onmessage`; see your relay API docs for exact shapes.

### 7.5 Reply-to (Threaded Messages)

Store the message ID when the user taps “reply”, then pass it as the third argument:

```ts
const payload = await protocol.groupSendMessage(groupId, content, replyToMessageId);
sendToRelay(payload);
```

Render replies in the UI by resolving `reply_to` / `replyToMsg` to the original message.

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
4. For groups: WebSocket connects, `Authenticated` received, `groupGetUserGroups` / create / send / reply-to work; add/remove member, leave, delete (admin).

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

**Event types**: `message_sent`, `message_received`, `message_delivered`, `message_failed`, `transport_switched`, `relay_promoted`, `relay_demoted`, `neighbor_discovered`, `neighbor_lost`, `network_metrics`, `file_progress`, `file_received`, `diagnostic`, `secure_session_established`, `secure_session_failed`.

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
| **sendMessage** | `sendMessage(params: SendMessageParams): Promise<string>` | Sends a message; returns message ID. `params`: `recipient`, `content`, `priority?`, `replyToMsg?`. |
| **receiveMessage** | `receiveMessage(): Promise<MessageReceivedEvent \| null>` | Polls for the next received message. |

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
| **setBatteryLevel** | `setBatteryLevel(level: number): Promise<void>` | Sets battery level (0–100) for relay decisions. |
| **getBatteryLevel** | `getBatteryLevel(): Promise<number \| null>` | Current battery level. |
| **setRelayPriority** | `setRelayPriority(priority: 'low' \| 'medium' \| 'high'): Promise<void>` | Sets relay priority. |
| **getRelayPriority** | `getRelayPriority(): Promise<'low' \| 'medium' \| 'high'>` | Current relay priority. |
| **isRelay** | `isRelay(): Promise<boolean>` | Whether this device is acting as a relay. |

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

---

### 11.8 Gradient Routing

| Method | Signature | Description |
|--------|-----------|-------------|
| **learnRoute** | `learnRoute(destination, nextHop, hopCount, quality): Promise<void>` | Records a route from an incoming message. |
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
| **internetMessageReceived** | `internetMessageReceived(senderId, data: number[]): Promise<void>` | Incoming internet message. |
| **internetGetNextMessage** | `internetGetNextMessage(): Promise<{messageId, recipientId, data} \| null>` | Next outgoing internet message. Use `messageId` to confirm or report failure. |
| **internetConfirmSent** | `internetConfirmSent(messageId: string): Promise<void>` | Confirms a message was sent over the wire. Call after WebSocket send succeeds. |
| **internetSendFailed** | `internetSendFailed(messageId: string): Promise<void>` | Reports a message failed to send. Call when WebSocket send fails. |

---

### 11.12 MLS (End-to-End Encryption)

| Method | Signature | Description |
|--------|-----------|-------------|
| **initializeMlsWithSecureStorage** | `initializeMlsWithSecureStorage(): Promise<void>` | Initialises MLS with iOS Keychain / Android EncryptedSharedPreferences. Called automatically by `start()` when encryption enabled. |
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

### 11.13 MLS Groups

| Method | Signature | Description |
|--------|-----------|-------------|
| **mlsCreateGroup** | `mlsCreateGroup(groupName): Promise<MlsGroupInfo>` | Creates MLS group. |
| **mlsAddGroupMember** | `mlsAddGroupMember(groupId, memberKeyPackage: number[]): Promise<MlsWelcome>` | Adds member; returns Welcome for new member. |
| **mlsRemoveGroupMember** | `mlsRemoveGroupMember(groupId, memberId): Promise<void>` | Removes member from MLS group. |
| **mlsLeaveGroup** | `mlsLeaveGroup(groupId): Promise<void>` | Leaves MLS group. |
| **mlsEncryptForGroup** | `mlsEncryptForGroup(groupId, plaintext: number[]): Promise<MlsEncryptedMessage>` | Encrypts for group. |
| **mlsDecryptFromGroup** | `mlsDecryptFromGroup(encrypted): Promise<number[] \| null>` | Decrypts message from group. |
| **mlsJoinGroup** | `mlsJoinGroup(welcome: MlsWelcome): Promise<MlsGroupInfo>` | Joins group from Welcome. |
| **mlsListGroups** | `mlsListGroups(): Promise<string[]>` | All MLS group IDs. |
| **mlsGetGroupInfo** | `mlsGetGroupInfo(groupId): Promise<MlsGroupInfo \| null>` | MLS group info. |

---

### 11.14 Group Management (Relay Server API)

Each method returns a **JSON string** to send over your WebSocket to the relay.

| Method | Signature | Description |
|--------|-----------|-------------|
| **groupCreate** | `groupCreate(name: string): Promise<string>` | Create group; creator is admin. |
| **groupSendMessage** | `groupSendMessage(groupId, content, replyToMsg?: string \| null): Promise<string>` | Send message to group; optional reply-to message ID. |
| **groupAddMember** | `groupAddMember(groupId, username): Promise<string>` | Add member (admin). |
| **groupRemoveMember** | `groupRemoveMember(groupId, username): Promise<string>` | Remove member (admin or self). |
| **groupSetAdmin** | `groupSetAdmin(groupId, username): Promise<string>` | Set member as admin (admin). |
| **groupRemoveAdmin** | `groupRemoveAdmin(groupId, username): Promise<string>` | Remove admin role (admin). |
| **groupLeave** | `groupLeave(groupId): Promise<string>` | Leave group. |
| **groupDelete** | `groupDelete(groupId): Promise<string>` | Delete group (admin). |
| **groupGetInfo** | `groupGetInfo(groupId): Promise<string>` | Get group info. |
| **groupGetUserGroups** | `groupGetUserGroups(): Promise<string>` | Get all groups for current user. |

---

### Need more?

- **Configuration**: `docs/configuration.md` for every tunable parameter.
- **DORS**: `docs/dors-configuration.md` for algorithm details.
- **Mesh**: `bindings/react-native/MESH.md` for mesh networking and BLE.
- **Architecture**: `docs/architecture.md` for high-level design.
- **Example app**: `examples/react-native-app` and `ProtocolProvider.tsx` for a full setup with UI.
