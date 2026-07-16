# @offline-protocol/mesh-sdk

Offline-first mesh networking SDK for React Native. Enables peer-to-peer messaging over BLE, WiFi Direct, Internet, Reticulum, and Nostr relays with intelligent transport switching.

## Table of Contents

- [Requirements](#requirements)
- [Installation](#installation)
- [Platform Setup](#platform-setup)
- [Quick Start](#quick-start)
- [End-to-End Encryption](#end-to-end-encryption)
- [Protocol Lifecycle](#protocol-lifecycle)
- [Configuration](#configuration)
- [API Reference](#api-reference)
- [Events](#events)
- [Types](#types)
- [DORS (Transport Switching)](#dors-dynamic-offline-relay-switch)
- [Mesh Networking](#mesh-networking)
- [Reliability Layer](#reliability-layer)
- [File Transfer](#file-transfer)
- [Troubleshooting](#troubleshooting)

---

## Requirements

| Platform | Version |
|----------|---------|
| React Native | >= 0.70.0 |
| iOS | >= 13.0 |
| Android | >= API 24 (Android 7.0) |
| Node.js | >= 16 |

---

## Installation

```bash
npm install @offline-protocol/mesh-sdk
```

### iOS

```bash
cd ios && pod install
```

### Android

Pre-built native libraries are included. No additional setup required.

---

## Platform Setup

### iOS Permissions

Add to `Info.plist`:

```xml
<key>NSBluetoothAlwaysUsageDescription</key>
<string>Required for offline mesh communication</string>

<key>NSBluetoothPeripheralUsageDescription</key>
<string>Required for offline mesh communication</string>

<key>UIBackgroundModes</key>
<array>
    <string>bluetooth-central</string>
    <string>bluetooth-peripheral</string>
</array>
```

### Android Permissions

Add to `AndroidManifest.xml`:

```xml
<!-- Bluetooth (Android 12+) -->
<uses-permission android:name="android.permission.BLUETOOTH_SCAN" />
<uses-permission android:name="android.permission.BLUETOOTH_ADVERTISE" />
<uses-permission android:name="android.permission.BLUETOOTH_CONNECT" />

<!-- Bluetooth (Android 11 and below) -->
<uses-permission android:name="android.permission.BLUETOOTH" />
<uses-permission android:name="android.permission.BLUETOOTH_ADMIN" />
<uses-permission android:name="android.permission.ACCESS_FINE_LOCATION" />

<!-- WiFi Direct -->
<uses-permission android:name="android.permission.ACCESS_WIFI_STATE" />
<uses-permission android:name="android.permission.CHANGE_WIFI_STATE" />
<uses-permission android:name="android.permission.NEARBY_WIFI_DEVICES" />

<!-- Internet -->
<uses-permission android:name="android.permission.INTERNET" />
```

---

## Quick Start

```typescript
import { OfflineProtocol, MessagePriority } from '@offline-protocol/mesh-sdk';

const protocol = new OfflineProtocol({
  appId: 'my-app',
  userId: 'user123',
  // Encryption is enabled by default - MLS is auto-initialized on start()
});

protocol.on('message_received', (event) => {
  console.log(`From ${event.sender}: ${event.content}`);
  // event.encrypted indicates if the message was encrypted
});

await protocol.start(); // MLS auto-initialized with secure storage

// Messages are automatically encrypted when possible!
const messageId = await protocol.sendMessage({
  recipient: 'user456',
  content: 'Hello!',  // Automatically encrypted
  priority: MessagePriority.High,
});

await protocol.stop();
await protocol.destroy();
```

### End-to-End Encryption

The SDK provides automatic end-to-end encryption using MLS (RFC 9420):

```typescript
const protocol = new OfflineProtocol({
  appId: 'my-app',
  userId: 'alice',
  encryption: {
    enabled: true,           // Auto-encrypt messages (default)
    autoKeyExchange: true,   // Exchange keys on peer discovery (default)
    storePending: true,      // Queue messages until session ready (default)
  },
});

await protocol.start(); // MLS auto-initialized when encryption.enabled is true

// That's it! Messages are now automatically encrypted/decrypted.
// Key packages are exchanged automatically when peers are discovered.
```

Both wire-format kill switches — top-level `binaryWireEnabled` and
`encryption.compactEnvelopeEnabled` (default `true`) — disable the compact
binary codec / compact MLS envelope at runtime; see
[Wire Format Kill Switches](../../docs/configuration.md#wire-format-kill-switches).

See the [MLS Integration Guide](../../docs/mls-integration.md) for advanced usage.

---

## Protocol Lifecycle

### Complete Flow Example

```typescript
import { 
  OfflineProtocol, 
  MessagePriority,
  ProtocolEvent,
  MessageReceivedEvent,
  MessageDeliveredEvent,
  NeighborDiscoveredEvent,
} from '@offline-protocol/mesh-sdk';

// 1. CREATE PROTOCOL INSTANCE
const protocol = new OfflineProtocol({
  appId: 'my-chat-app',
  userId: 'alice-device-001',
});

// 2. REGISTER EVENT LISTENERS (before starting)

// Track discovered peers
const discoveredPeers = new Map<string, number>(); // peerId -> rssi

protocol.on('neighbor_discovered', (event: NeighborDiscoveredEvent) => {
  console.log(`[PEER FOUND] ${event.peer_id} via ${event.transport}, RSSI: ${event.rssi}`);
  discoveredPeers.set(event.peer_id, event.rssi ?? -100);
});

protocol.on('neighbor_lost', (event) => {
  console.log(`[PEER LOST] ${event.peer_id}`);
  discoveredPeers.delete(event.peer_id);
});

// Track outgoing messages
const pendingMessages = new Map<string, { recipient: string; content: string }>();

protocol.on('message_sent', (event) => {
  console.log(`[SENT] Message ${event.message_id} to ${event.recipient}`);
  pendingMessages.set(event.message_id, {
    recipient: event.recipient,
    content: event.content,
  });
});

protocol.on('message_delivered', (event: MessageDeliveredEvent) => {
  console.log(`[DELIVERED] Message ${event.message_id} in ${event.latency_ms}ms, ${event.hop_count} hops`);
  pendingMessages.delete(event.message_id);
});

protocol.on('message_failed', (event) => {
  console.log(`[FAILED] Message ${event.message_id}: ${event.reason} (${event.retry_count} retries)`);
  pendingMessages.delete(event.message_id);
});

// Handle incoming messages
protocol.on('message_received', (event: MessageReceivedEvent) => {
  console.log(`[RECEIVED] From ${event.sender}: ${event.content}`);
  console.log(`  - Message ID: ${event.message_id}`);
  console.log(`  - Hop count: ${event.hop_count}`);
  console.log(`  - Transport: ${event.transport}`);
  
  // Process the message in your app
  handleIncomingMessage(event);
});

// Monitor transport changes
protocol.on('transport_switched', (event) => {
  console.log(`[TRANSPORT] Switched from ${event.from} to ${event.to}: ${event.reason}`);
});

// 3. START THE PROTOCOL
await protocol.start();
// At this point:
// - BLE scanning begins (discovers nearby devices)
// - BLE advertising begins (makes this device discoverable)
// - neighbor_discovered events will start firing as peers are found

// 4. WAIT FOR PEERS (optional helper)
async function waitForPeer(peerId: string, timeoutMs = 30000): Promise<boolean> {
  if (discoveredPeers.has(peerId)) return true;
  
  return new Promise((resolve) => {
    const timeout = setTimeout(() => resolve(false), timeoutMs);
    
    const handler = (event: NeighborDiscoveredEvent) => {
      if (event.peer_id === peerId) {
        clearTimeout(timeout);
        protocol.off('neighbor_discovered', handler);
        resolve(true);
      }
    };
    
    protocol.on('neighbor_discovered', handler);
  });
}

// 5. SEND A MESSAGE
async function sendChatMessage(recipientId: string, text: string) {
  try {
    const messageId = await protocol.sendMessage({
      recipient: recipientId,
      content: text,
      priority: MessagePriority.High,
    });
    console.log(`Message queued with ID: ${messageId}`);
    return messageId;
  } catch (error) {
    console.error('Failed to send message:', error);
    throw error;
  }
}

// 6. CLEANUP ON APP EXIT
async function cleanup() {
  await protocol.stop();
  await protocol.destroy();
}
```

### Event Sequence Timeline

```
┌─────────────────────────────────────────────────────────────────────┐
│                        PROTOCOL LIFECYCLE                           │
├─────────────────────────────────────────────────────────────────────┤
│                                                                     │
│  new OfflineProtocol(config)                                        │
│       │                                                             │
│       ▼                                                             │
│  protocol.on('...', handler)  ← Register all event listeners        │
│       │                                                             │
│       ▼                                                             │
│  await protocol.start()                                             │
│       │                                                             │
│       ├──► BLE advertising starts (device becomes discoverable)    │
│       ├──► BLE scanning starts (looking for other devices)         │
│       │                                                             │
│       ▼                                                             │
│  ┌─────────────────────────────────────────────────────────────┐   │
│  │  PEER DISCOVERY PHASE                                        │   │
│  │                                                              │   │
│  │  neighbor_discovered { peer_id, transport, rssi }           │   │
│  │  neighbor_discovered { peer_id, transport, rssi }           │   │
│  │  ...                                                         │   │
│  │                                                              │   │
│  │  MeshController evaluates peers, establishes connections     │   │
│  │  (MEMBER for same cluster, BRIDGE for different clusters)   │   │
│  └─────────────────────────────────────────────────────────────┘   │
│       │                                                             │
│       ▼                                                             │
│  ┌─────────────────────────────────────────────────────────────┐   │
│  │  MESSAGING PHASE                                             │   │
│  │                                                              │   │
│  │  protocol.sendMessage({ recipient, content, priority })     │   │
│  │       │                                                      │   │
│  │       ▼                                                      │   │
│  │  message_sent { message_id, recipient, content, ... }       │   │
│  │       │                                                      │   │
│  │       ├──► [SUCCESS] message_delivered { message_id, ... }  │   │
│  │       │                                                      │   │
│  │       └──► [FAILURE] message_failed { message_id, reason }  │   │
│  │                                                              │   │
│  │  ─ ─ ─ ─ ─ ─ ─ ─ ─ ─ ─ ─ ─ ─ ─ ─ ─ ─ ─ ─ ─ ─ ─ ─ ─ ─ ─   │   │
│  │                                                              │   │
│  │  INCOMING: message_received { sender, content, ... }        │   │
│  └─────────────────────────────────────────────────────────────┘   │
│       │                                                             │
│       │  (peers may come and go)                                    │
│       │                                                             │
│       ▼                                                             │
│  neighbor_lost { peer_id }                                          │
│  neighbor_discovered { peer_id, ... }  ← new peer appears           │
│       │                                                             │
│       ▼                                                             │
│  await protocol.stop()                                              │
│       │                                                             │
│       ├──► BLE scanning stops                                       │
│       ├──► BLE advertising stops                                    │
│       ├──► All connections closed                                   │
│       │                                                             │
│       ▼                                                             │
│  await protocol.destroy()  ← Clean up resources                     │
│                                                                     │
└─────────────────────────────────────────────────────────────────────┘
```

### What Happens Under the Hood

#### On `protocol.start()`

1. **Protocol core starts** in Rust
2. **BLE Manager initializes**:
   - Starts scanning for devices advertising the Offline Protocol service UUID
   - Starts advertising this device with mesh metadata (degree, free slots, battery, uptime)
3. **Process timer starts** - polls for outgoing fragments every 100ms

#### On Peer Discovery

1. **BLE scan detects advertisement** from another device
2. **MeshController.shouldInitiateOutbound()** evaluates the candidate:
   - Checks connection budget (default max: 4)
   - Calculates peer score (RSSI, availability, battery, uptime, stability, load)
   - Determines if this is a cluster bridge opportunity
3. **If accepted**: BLE connection established, `neighbor_discovered` fires
4. **If at capacity**: May evict a lower-scoring peer to make room

#### On `protocol.sendMessage()`

1. **Message created** with unique ID, TTL, timestamp, priority
2. **message_sent event** fires immediately
3. **Message queued** for transmission
4. **DORS selects transport** (BLE, WiFi Direct, or Internet)
5. **Message sent** to connected peers
6. **ACK tracking begins** (default 10s timeout)
7. **On ACK received**: `message_delivered` event fires
8. **On timeout/max retries**: `message_failed` event fires

#### On Incoming Message

1. **BLE fragment received** from peer
2. **Deduplication check** - skip if message ID already seen
3. **If addressed to this device**: `message_received` event fires
4. **ACK sent back** to sender
5. **Hop count incremented** for metrics

#### On `protocol.stop()`

1. **BLE Manager stops** scanning and advertising
2. **All peer connections closed**
3. **neighbor_lost events** fire for each disconnected peer
4. **Protocol core stops**

### Diagnostic Events

The SDK emits diagnostic events for debugging:

```typescript
protocol.on('diagnostic', (event) => {
  console.log(`[${event.level.toUpperCase()}] ${event.message}`, event.context);
});
```

---

## Configuration

### ProtocolConfig

```typescript
interface ProtocolConfig {
  appId: string;
  userId: string;
  transports?: TransportsConfig;
  binaryWireEnabled?: boolean;  // binary wire-codec kill switch (default: true)
  encryption?: EncryptionConfig;
  dors?: DorsConfig;
  network?: NetworkConfig;
  reliability?: ReliabilityConfig;
  fileTransfer?: FileTransferConfig;
  path?: PathConfig;
}
```

### TransportsConfig

```typescript
interface TransportsConfig {
  ble?: {
    enabled: boolean;  // default: true
  };
  internet?: {
    enabled: boolean;           // default: false
    serverAddress?: string;     // WebSocket URL
    autoReconnect?: boolean;    // default: true
    reconnectDelay?: number;    // ms
  };
  wifiDirect?: {
    enabled: boolean;           // default: false (Android only)
    deviceName?: string;
    autoAccept?: boolean;
    groupOwnerIntent?: number;  // 0-15
  };
  reticulum?: {
    enabled: boolean;           // default: false (requires external daemon)
  };
  nostr?: {
    enabled: boolean;           // default: false
    relayUrls?: string[];       // wss:// relay URLs
    connectionTimeout?: number; // seconds, default: 30
    autoReconnect?: boolean;    // default: true
    reconnectDelay?: number;    // ms, default: 1000
    maxReconnectAttempts?: number; // default: 0 (infinite)
  };
}
```

### DorsConfig

Controls transport switching behavior.

```typescript
interface DorsConfig {
  preferOnline?: boolean;              // default: false
  switchHysteresis?: number;           // default: 15.0
  switchCooldownSecs?: number;         // default: 20
  bleToWifiRetryThreshold?: number;    // default: 2
  rssiSwitchThreshold?: number;        // default: -85 dBm
  congestionQueueThreshold?: number;   // default: 50
  stabilityWindowSecs?: number;        // default: 8
  poorSignalDurationSecs?: number;     // default: 10
  ttlEscalationThreshold?: number;     // default: 2
  congestionDurationSecs?: number;     // default: 10
  ttlEscalationHoldSecs?: number;      // default: 20
  historyWindowSize?: number;          // default: 10
  queueRecoveryRatio?: number;         // default: 0.5
}
```


### NetworkConfig

```typescript
interface NetworkConfig {
  initialTtl?: number;  // default: 8
}
```

### ReliabilityConfig

```typescript
interface ReliabilityConfig {
  ack?: {
    defaultTimeoutMs?: number;   // default: 10000
    maxPendingAcks?: number;     // default: 1000
  };
  retry?: {
    maxRetries?: number;         // default: 10
    initialDelayMs?: number;     // default: 1000
    maxDelayMs?: number;         // default: 30000
    backoffMultiplier?: number;  // default: 2.0
    outboxMaxLifetimeMs?: number; // default: 3600000
  };
  dedup?: {
    maxTrackedMessages?: number; // default: 1000
    retentionTimeSecs?: number;  // default: 3600
  };
}
```

### PathConfig

```typescript
interface PathConfig {
  forwardToTopK?: number;        // default: 3
  maxCongestionLevel?: number;   // default: 0.7
}
```

### FileTransferConfig

```typescript
interface FileTransferConfig {
  chunkSize?: number;    // default: 32768 (32KB)
  maxFileSize?: number;  // default: 104857600 (100MB)
}
```

---

## API Reference

### Constructor

```typescript
new OfflineProtocol(config: ProtocolConfig)
```

### Lifecycle Methods

| Method | Returns | Description |
|--------|---------|-------------|
| `start()` | `Promise<void>` | Start the protocol and all enabled transports |
| `stop()` | `Promise<void>` | Stop the protocol and disconnect all peers |
| `pause()` | `Promise<void>` | Pause for background mode |
| `resume()` | `Promise<void>` | Resume from paused state |
| `destroy()` | `Promise<void>` | Clean up all resources |
| `getState()` | `Promise<ProtocolState>` | Get current state (`Stopped`, `Running`, `Paused`) |

### Messaging

| Method | Returns | Description |
|--------|---------|-------------|
| `sendMessage(params: SendMessageParams)` | `Promise<string>` | Send message, returns message ID |
| `receiveMessage()` | `Promise<MessageReceivedEvent \| null>` | Poll for next received message |

```typescript
interface SendMessageParams {
  recipient: string;
  content: string;
  priority?: MessagePriority;  // default: Medium
}

enum MessagePriority {
  Low = 0,
  Medium = 1,
  High = 2,
  Critical = 3,
}
```

### Connection Requests

| Method | Returns | Description |
|--------|---------|-------------|
| `sendConnectionRequest(params)` | `Promise<string>` | Send a connection request, returns message ID |
| `acceptConnectionRequest(params)` | `Promise<string>` | Accept a received request |
| `rejectConnectionRequest(params)` | `Promise<string>` | Reject a received request |
| `cancelConnectionRequest(params)` | `Promise<string>` | Cancel a request you sent |

`recipient` is always the target's canonical user id — the value they
supplied as `ProtocolConfig.userId`, which is exactly what
`neighbor_discovered` reports as `peer_id` (see [Identity](#identity)).

The message ID returned by `sendConnectionRequest` is the correlation key
for the request's outcome events:

| Outcome | Event | Correlate by |
|---------|-------|--------------|
| Recipient offline (relay delivery error) | `connection_request_undeliverable` (`reason` starts with `recipient_unreachable`) | `message_id` |
| Retry budget exhausted | `connection_request_undeliverable` (`reason: 'max_retries_exceeded'`) **and** `message_failed` | `message_id` |
| Reached the recipient's device | `message_delivered` | `message_id` |
| Recipient accepted | `connection_accepted` | `accepted_by` (peer id) |
| Recipient rejected | `connection_rejected` | `rejected_by` (peer id) |
| Sender cancelled (recipient side) | `connection_request_cancelled` | `cancelled_by` (peer id) |

`connection_request_undeliverable` is a status signal, not proof of
permanent failure: the retry machinery may still deliver the original if
the peer comes back online, so a user-initiated resend can duplicate the
original request on the recipient's side.

### Transport Management

| Method | Returns | Description |
|--------|---------|-------------|
| `getActiveTransports()` | `Promise<TransportType[]>` | Get list of active transports |
| `enableTransport(type, config?)` | `Promise<void>` | Enable a transport |
| `disableTransport(type)` | `Promise<void>` | Disable a transport |
| `forceTransport(type)` | `Promise<void>` | Force specific transport (override DORS) |
| `releaseTransportLock()` | `Promise<void>` | Release forced transport, let DORS decide |
| `getTransportMetrics(type)` | `Promise<TransportMetrics \| null>` | Get transport statistics |

```typescript
type TransportType = 'ble' | 'internet' | 'wifiDirect' | 'reticulum' | 'nostr';

interface TransportMetrics {
  packetsSent: number;
  packetsReceived: number;
  bytesSent: number;
  bytesReceived: number;
  errorRate: number;
  avgLatencyMs: number;
}
```

### Bluetooth

| Method | Returns | Description |
|--------|---------|-------------|
| `isBluetoothEnabled()` | `Promise<boolean>` | Check if Bluetooth is enabled |
| `requestEnableBluetooth()` | `Promise<boolean>` | Request to enable Bluetooth (Android only) |
| `getBLePeerCount()` | `Promise<number>` | Get number of discovered BLE peers |

### Network Topology

| Method | Returns | Description |
|--------|---------|-------------|
| `getTopology()` | `Promise<NetworkTopology>` | Get network topology snapshot |
| `getMessageStats()` | `Promise<MessageDeliveryStats[]>` | Get message delivery statistics |
| `getDeliverySuccessRate()` | `Promise<number>` | Get delivery success rate (0-1) |
| `getMedianLatency()` | `Promise<number \| null>` | Get median latency in ms |
| `getMedianHops()` | `Promise<number \| null>` | Get median hop count |

### Battery

| Method | Returns | Description |
|--------|---------|-------------|
| `setBatteryLevel(level)` | `Promise<void>` | Set battery level (0-100) for mesh decisions |
| `getBatteryLevel()` | `Promise<number \| null>` | Get current battery level |

### DORS Configuration

| Method | Returns | Description |
|--------|---------|-------------|
| `updateDorsConfig(config)` | `Promise<void>` | Update DORS settings at runtime |
| `getDorsConfig()` | `Promise<DorsConfig>` | Get current DORS configuration |
| `shouldEscalateToWifi()` | `Promise<boolean>` | Check if DORS recommends WiFi escalation |

### Reliability Configuration

| Method | Returns | Description |
|--------|---------|-------------|
| `updateAckConfig(config)` | `Promise<void>` | Update ACK settings |
| `updateRetryConfig(config)` | `Promise<void>` | Update retry settings |
| `updateDedupConfig(config)` | `Promise<void>` | Update deduplication settings |
| `getDedupStats()` | `Promise<DedupStats>` | Get deduplication statistics |
| `getPendingAckCount()` | `Promise<number>` | Get pending ACK count |
| `getRetryQueueSize()` | `Promise<number>` | Get retry queue size |

### Gradient Routing

| Method | Returns | Description |
|--------|---------|-------------|
| `learnRoute(destination, nextHop, hopCount, quality, sequenceNumber?)` | `Promise<void>` | Learn a route from incoming message. `sequenceNumber` is DSDV-style; pass `0` when the message does not carry one. Negative values are clamped to 0 before passing to the native layer. |
| `getBestRoute(destination)` | `Promise<RouteEntry \| null>` | Get best route to destination |
| `getAllRoutes(destination)` | `Promise<RouteEntry[]>` | Get all routes to destination |
| `hasRoute(destination)` | `Promise<boolean>` | Check if route exists |
| `removeNeighborRoutes(neighborId)` | `Promise<void>` | Remove routes through neighbor |
| `cleanupExpiredRoutes()` | `Promise<void>` | Clean up expired routes |
| `getRoutingStats()` | `Promise<RoutingStats>` | Get routing table statistics |
| `updateRoutingConfig(config)` | `Promise<void>` | Update routing configuration |

### Event Listeners

| Method | Returns | Description |
|--------|---------|-------------|
| `on(eventType, listener)` | `this` | Register event listener |
| `off(eventType, listener)` | `this` | Remove event listener |
| `once(eventType, listener)` | `this` | Register one-time listener |
| `removeAllListeners(eventType?)` | `this` | Remove all listeners |

---

## Events


### Message Events

#### message_sent

```typescript
interface MessageSentEvent {
  type: 'message_sent';
  message_id: string;
  sender: string;
  recipient: string;
  content: string;
  priority: 'low' | 'medium' | 'high' | 'critical';
  requires_ack: boolean;
  timestamp: number;
}
```

#### message_received

```typescript
interface MessageReceivedEvent {
  type: 'message_received';
  message_id: string;
  sender: string;
  recipient: string;
  content: string;
  hop_count: number;
  transport: string;
  timestamp: number;
}
```

#### message_delivered

```typescript
interface MessageDeliveredEvent {
  type: 'message_delivered';
  message_id: string;
  latency_ms: number;
  hop_count: number;
  transport: string;
}
```

#### message_failed

```typescript
interface MessageFailedEvent {
  type: 'message_failed';
  message_id: string;
  reason: string;
  retry_count: number;
}
```

### Network Events

#### transport_switched

```typescript
interface TransportSwitchedEvent {
  type: 'transport_switched';
  from: string | null;
  to: string;
  reason: string;
}
```

#### neighbor_discovered

```typescript
interface NeighborDiscoveredEvent {
  type: 'neighbor_discovered';
  peer_id: string;
  transport: string;
  rssi?: number;
}
```

<a name="identity"></a>
**Identity contract**: `peer_id` is the peer's canonical user id — the
same value the peer supplied as `ProtocolConfig.userId`, regardless of
which transport discovered them. Use it directly as `recipient` in
`sendMessage` and `sendConnectionRequest`; there is no separate
transport-level address to resolve.

#### neighbor_lost

```typescript
interface NeighborLostEvent {
  type: 'neighbor_lost';
  peer_id: string;
}
```

#### network_metrics

```typescript
interface NetworkMetricsEvent {
  type: 'network_metrics';
  neighbor_count: number;
  relay_count: number;
  delivery_ratio: number;
  avg_latency_ms: number;
}
```

### Connection Events

See [Connection Requests](#connection-requests) for the full outcome table.

#### connection_request_received

Fires on the recipient's side when a connection request arrives.

```typescript
interface ConnectionRequestReceivedEvent {
  type: 'connection_request_received';
  sender: string;
  sender_name: string;
  timestamp: number;
  key_package?: number[];
  initial_message?: string;
}
```

#### connection_request_undeliverable

Fires on the sender's side when a request could not be delivered — the
recipient is offline (`reason` starts with `recipient_unreachable`) or the
retry budget ran out (`reason: 'max_retries_exceeded'`, alongside a
generic `message_failed`). Correlate via the message ID returned by
`sendConnectionRequest`. This is a status signal, not proof of permanent
failure.

```typescript
interface ConnectionRequestUndeliverableEvent {
  type: 'connection_request_undeliverable';
  recipient: string;
  message_id: string;
  reason: string;
}
```

#### connection_accepted

```typescript
interface ConnectionAcceptedEvent {
  type: 'connection_accepted';
  accepted_by: string;
  accepted_by_name: string;
  timestamp: number;
  key_package?: number[];
}
```

#### connection_rejected

```typescript
interface ConnectionRejectedEvent {
  type: 'connection_rejected';
  rejected_by: string;
}
```

#### connection_request_cancelled

```typescript
interface ConnectionRequestCancelledEvent {
  type: 'connection_request_cancelled';
  cancelled_by: string;
}
```

### File Events

#### file_progress

```typescript
interface FileProgressEvent {
  type: 'file_progress';
  file_id: string;
  chunks_sent: number;
  total_chunks: number;
  percentage: number;
}
```

#### file_received

```typescript
interface FileReceivedEvent {
  type: 'file_received';
  file_id: string;
  file_name: string;
  file_size: number;
  sender: string;
}
```

### Diagnostic Events

```typescript
interface DiagnosticEvent {
  type: 'diagnostic';
  level: 'info' | 'warning' | 'error';
  message: string;
  context?: Record<string, unknown>;
}
```

---

## Types

### NetworkTopology

```typescript
interface NetworkTopology {
  timestamp: number;
  local_user_id: string;
  nodes: NetworkNode[];
  links: NetworkLink[];
  stats: NetworkStats;
}

interface NetworkNode {
  user_id: string;
  role: string;  // 'Normal' or 'Relay' from topology API
  connection_count: number;
  battery_level?: number;
  last_seen: number;
  transports: TransportType[];
}

interface NetworkLink {
  from: string;
  to: string;
  quality: number;  // 0.0 - 1.0
  transport: TransportType;
  rssi?: number;
}

interface NetworkStats {
  total_nodes: number;
  relay_nodes: number;  // Count of nodes with 'Relay' role in topology
  total_connections: number;
  avg_link_quality: number;
  network_diameter?: number;
}
```

### MessageDeliveryStats

```typescript
interface MessageDeliveryStats {
  message_id: string;
  sender: string;
  recipient: string;
  sent_at: number;
  delivered_at?: number;
  hop_count: number;
  transport?: TransportType;
  retry_count: number;
  latency_ms?: number;
}
```

### RouteEntry

```typescript
interface RouteEntry {
  nextHop: string;
  hopCount: number;
  quality: number;     // 0.0 - 1.0
  lastSeenMs: number;
}
```

### RoutingStats

```typescript
interface RoutingStats {
  destinationCount: number;
  routeCount: number;
}
```

### DedupStats

```typescript
interface DedupStats {
  totalTracked: number;
  recentTracked: number;
  capacityUsedPercent: number;
  mode: 'HashMap' | 'BloomFilter';
}
```

### FileProgress

```typescript
interface FileProgress {
  file_id: string;
  file_name: string;
  file_size: number;
  chunks_completed: number;
  total_chunks: number;
  percentage: number;
}
```

### ProtocolState

```typescript
enum ProtocolState {
  Stopped = 0,
  Running = 1,
  Paused = 2,
}
```

---

## DORS (Dynamic Offline Relay Switch)

DORS automatically selects the optimal transport based on real-time conditions.

### Scoring Factors

| Factor | Description |
|--------|-------------|
| Signal Strength | RSSI for BLE/WiFi (-50 to -100 dBm) |
| Proximity | Hop count to destination |
| Bandwidth | Transport throughput capability |
| Congestion | Queue depth and backlog |
| Energy | Battery impact of transport |
| Reliability | Historical delivery success rate |
| Load | Current processing capacity |

### Transport Weights

**BLE**: Optimized for energy efficiency and mesh scenarios
- Signal: 30%, Energy: 30%, Congestion: 15%, Proximity: 15%

**WiFi Direct**: Optimized for high throughput
- Bandwidth: 35%, Proximity: 20%, Congestion: 20%, Reliability: 15%

**Internet**: Optimized for server connectivity
- Bandwidth: 35%, Reliability: 30%, Congestion: 15%, Energy: 10%

**Reticulum**: Optimized for resilience and long-range fallback
- Reliability: 30%, Energy: 25%, Proximity: 20%, Congestion: 15%, Signal: 5%, Bandwidth: 5%

**Nostr**: Optimized for censorship-resistant fallback over WebSocket relays
- Reliability: 35%, Bandwidth: 20%, Congestion: 20%, Energy: 15%, Load: 10%

### Switching Safeguards

| Safeguard | Default | Description |
|-----------|---------|-------------|
| Hysteresis | 15 points | Minimum score improvement to switch |
| Cooldown | 20 seconds | Wait time between switches |
| Stability Window | 8 seconds | Transport must be stable before switching |

---

## Mesh Networking

### Cluster Architecture

Devices organize into **clusters** (groups of nearby connected peers). Connections between clusters are handled by **bridge** connections.

**Connection Roles:**
- `MEMBER` - Intra-cluster connection (devices in same neighborhood)
- `BRIDGE` - Inter-cluster connection (bridges different neighborhoods)

### How It Works

1. **Discovery**: Devices broadcast BLE advertisements with mesh metadata (degree, free slots, battery, uptime)
2. **Cluster Detection**: Each device computes a cluster signature from connected peer hashes
3. **Connection Decisions**: MeshController evaluates candidates - prioritizes bridging different clusters
4. **Rebalancing**: Periodically swaps lower-quality peers for better candidates or bridge opportunities
5. **Delivery**: Messages sent to connected peers

### Connection Budget

- Default: 4 connections per device
- Minimum: 1 connection maintained
- Connections are scored and rebalanced every ~15 seconds
- Bridge candidates get priority when clusters need unifying

### Peer Scoring

| Factor | Weight | Description |
|--------|--------|-------------|
| RSSI | 35% | Signal strength to peer |
| Availability | 20% | Free connection slots |
| Uptime | 15% | How long peer has been active |
| Battery | 15% | Peer's battery level |
| Stability | 10% | Connection reliability history |
| Load | 5% | Current processing load |

**Bridge Favor**: Candidates from different clusters get a score bonus (`bridgeFavor: 0.1`) to encourage network unification.

### Message TTL

- Default: 8 hops
- Messages are dropped when TTL reaches 0
- Prevents infinite message circulation

### Group Messaging (MLS-Encrypted)

Create and manage encrypted groups over the mesh:

```typescript
// Create a group (you become admin automatically)
const group = await protocol.meshCreateGroup('Project Team');

// Invite a member (admin only)
await protocol.meshInviteToGroup(group.groupId, 'bob');

// Send an encrypted group message
await protocol.meshSendGroupMessage(group.groupId, 'Hello team!');

// Remove a member (admin only)
await protocol.meshRemoveFromGroup(group.groupId, 'bob');

// Leave a group
await protocol.meshLeaveGroup(group.groupId);
```

### Group Roles

Groups use role-based access control with two roles: **Admin** and **Member**.

| Method | Description |
|--------|-------------|
| `meshSetMemberRole(groupId, userId, role)` | Change a member's role (`"admin"` or `"member"`) — admin only |
| `meshGetMemberRole(groupId, userId)` | Get a member's current role |
| `meshGetGroupRoles(groupId)` | Get all roles as `{ userId: role }` |

```typescript
// Promote a member to admin
await protocol.meshSetMemberRole(groupId, 'bob', 'admin');

// Check a member's role
const role = await protocol.meshGetMemberRole(groupId, 'bob'); // "admin"

// Get all roles
const roles = await protocol.meshGetGroupRoles(groupId);
// { alice: "admin", bob: "admin", charlie: "member" }

// Listen for role changes
protocol.on('group_role_changed', (event) => {
  console.log(`${event.user_id} is now ${event.new_role}`);
});
```

**Security invariants:**
- The group creator is automatically `Admin`
- Only admins can invite, remove, or change roles
- The last admin cannot be demoted, removed, or leave (prevents orphaned groups)
- If the last admin disconnects unexpectedly, a deterministic election promotes the next admin

---

## Reliability Layer

### Acknowledgments

- Messages require ACK for delivery confirmation
- Default timeout: 5 seconds
- `message_delivered` event fires on ACK receipt

### Retry Queue

- Failed messages are retried with exponential backoff
- Initial delay: 1 second
- Maximum delay: 30 seconds
- Maximum retries: 5

### Deduplication

Prevents duplicate message processing:

- **Bloom Filter Mode**: Space-efficient, ~1% false positive rate
- **HashMap Mode**: Exact tracking, configurable capacity

---

## File Transfer

### Sending Files

```typescript
const fileId = await protocol.sendFile({
  filePath: '/path/to/file.pdf',
  recipient: 'user456',
  fileName: 'document.pdf',  // optional
});

protocol.on('file_progress', (event) => {
  console.log(`${event.percentage}% complete`);
});
```

### Managing Transfers

```typescript
const progress = await protocol.getFileProgress(fileId);
await protocol.cancelFileTransfer(fileId);
```

---

## Troubleshooting

### Messages Not Delivering

1. Verify both devices have protocol started
2. Check they're within BLE range (~10-30m)
3. Ensure TTL is sufficient for network size
4. Monitor `message_failed` events for retry information

### No Peers Discovered

1. Verify Bluetooth is enabled: `await protocol.isBluetoothEnabled()`
2. Check permissions are granted
3. Ensure background modes enabled (iOS)
4. Verify devices are within range

### Frequent Disconnections

1. Check signal strength via `neighbor_discovered` RSSI
2. Increase `stabilityWindowSecs` in DORS config
3. Reduce `rebalanceInterval` frequency
4. Check for BLE interference

### High Battery Drain

1. Reduce connection count (native mesh config)
2. Verify DORS is selecting BLE over WiFi Direct
3. Check for excessive retry activity
4. Use `setBatteryLevel()` to inform mesh decisions

### Transport Not Switching

1. Verify transport is enabled in config
2. Check hysteresis threshold isn't too high
3. Ensure cooldown period has elapsed
4. Use `forceTransport()` to test manually

### Linking Error

If you see the linking error message:
1. Run `pod install` (iOS)
2. Rebuild the app after installing
3. Verify not using Expo Go (native modules required)

---

## License

The Offline Protocol SDK is **dual-licensed**:

- **AGPL-3.0-only** — see [LICENSE](../../LICENSE). Free for use in projects that comply
  with AGPL-3.0, including its network-use source-disclosure requirement (section 13).
- **Commercial License** — for proprietary apps that cannot or do not wish to comply with
  the AGPL. See [LICENSE-COMMERCIAL.md](./LICENSE-COMMERCIAL.md) (contact legal@offlineprotocol.com).

You may use the SDK under **either** license; you do not need both.
