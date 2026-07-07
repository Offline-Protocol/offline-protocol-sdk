# Nostr Transport

## Overview

The Nostr transport routes messages over [Nostr](https://nostr.com/) relays via WebSockets, providing a censorship-resistant, decentralized fallback when direct mesh and ordinary Internet endpoints are unreachable. Each install signs events with a BIP-340 Schnorr keypair derived from a random per-install secret (persisted via the app's `MlsStorage`); addressing uses a public *routing tag* deterministically derived from the `userId`, so peers can compute where to send without exchanging keys. Relays simply rebroadcast the signed events to subscribers.

Nostr is the fifth transport in the Offline Protocol SDK, alongside BLE, WiFi Direct, Internet, and Reticulum. It is disabled by default because it requires at least one relay URL.

## When to Use Nostr

| Scenario | Why Nostr |
|----------|-----------|
| Censorship circumvention | Many independent relays — blocking one doesn't take the network down |
| Cross-network reach | Works wherever WebSockets work, including over hostile NAT or transparent proxies |
| Public discoverability | Anyone subscribing to your `pubkey` filter on a shared relay can receive |
| Lightweight infrastructure | No daemon, no LoRa hardware, no custom server — just a relay URL |

Nostr is **not** suitable for:
- Latency-sensitive workloads — relay round-trips add tens of ms at minimum
- Strict private metadata — relays see sender pubkey, recipient pubkey, and timing
- Pure offline scenarios — relays are reachable only when the device has Internet

## Architecture

The Rust `NostrTransport` owns the queue, signing, and confirmation loop. The platform side owns the WebSocket connections and relay-protocol framing (`["EVENT", ...]`, `["REQ", ...]`, `["OK", ...]`).

```
┌──────────────────────┐
│  Offline Protocol    │
│  (Rust Core)         │
│                      │
│  NostrTransport      │◄── BIP-340 Schnorr signing (k256)
│  - send_queue        │    Per-message retry budget (3)
│  - receive_queue     │
│  - pending_confirm   │
│  - metrics           │
└──────────┬───────────┘
           │ Platform Bridge (UniFFI)
           ▼
┌──────────────────────┐
│  Platform Layer      │
│  (NostrManager)      │
│                      │
│  - WebSocket pool    │◄── iOS: URLSessionWebSocketTask
│  - REQ subscriptions │    Android: OkHttp
│  - OK demux          │
└──────────┬───────────┘
           │ wss://
           ▼
┌──────────────────────┐
│  Nostr Relays        │
│  (any NIP-01 relay)  │
└──────────────────────┘
```

The Rust side signs with the per-install Schnorr keypair (stable once `initialize_mls` persists the install secret), addresses the event to the recipient's routing tag (derived from their `userId`), builds canonical Nostr event JSON, and hands the platform a `NostrMessage` with `{message_id, event_id, event_json}`. The platform never touches secret material.

## Configuration

### Enabling Nostr

**React Native (TypeScript)**:
```typescript
const protocol = new OfflineProtocol({
  appId: 'my-app',
  userId: 'user123',
  transports: {
    ble: { enabled: true },
    nostr: {
      enabled: true,
      relayUrls: [
        'wss://relay.damus.io',
        'wss://nos.lol',
        'wss://relay.nostr.band',
      ],
      autoReconnect: true,
    },
  },
});
```

**Kotlin (Android)**:
```kotlin
val config = ProtocolConfig(
    appId = "my-app",
    userId = "user123",
    bleEnabled = true,
    nostrEnabled = true,
    // ... other fields
)
```

**Swift (iOS)**:
```swift
let config = ProtocolConfig(
    appId: "my-app",
    userId: "user123",
    bleEnabled: true,
    nostrEnabled: true,
    // ... other fields
)
```

The platform managers pull relay URLs and reconnect parameters from the JSON config they receive from JavaScript; the UDL-level boolean only gates whether the transport is wired up.

### NostrTransportConfig (TypeScript)

| Field | Type | Default | Description |
|-------|------|---------|-------------|
| `enabled` | boolean | `false` | Enable Nostr transport |
| `relayUrls` | string[] | `[]` | Nostr relay WebSocket URLs (`wss://…`) |
| `connectionTimeout` | number | `30` | Connection timeout in seconds |
| `autoReconnect` | boolean | `true` | Auto-reconnect on disconnect |
| `reconnectDelay` | number | `1000` | Reconnect delay in milliseconds |
| `maxReconnectAttempts` | number | `0` | Max attempts per relay (0 = infinite) |

### Transport-Specific Constants

| Constant | Value | Description |
|----------|-------|-------------|
| Connection timeout | 30 s | Default time to wait for relay connection |
| Pending confirmation timeout | 30 s | Time before treating an unconfirmed publish as failed |
| Max signing retries | 3 | Per-message signing-failure budget before permanent fail |
| DORS tie-break priority | 4 (lowest) | Internet (0) > WiFi Direct (1) > BLE (2) > Reticulum (3) > Nostr (4) |
| Bandwidth max | 1 MB/s | Practical upper bound for relay-bounded throughput |
| Energy baseline | 55 | Same radio class as Internet |

## Platform Bridge Lifecycle

The platform bridge interacts with `NostrTransport` through these UniFFI calls:

1. **Initialize** the WebSocket pool against the configured relay URLs.
2. **Report status** via `nostrStatusChanged(true)` once at least one relay is connected.
3. **Drain outgoing messages** event-driven via the `NostrTransportCallback.on_messages_available()` hook (with `nostrGetNextMessage()` as the polling fallback).
4. **Publish** the `event_json` to every connected relay.
5. **Confirm** via `nostrConfirmSent(messageId)` when ≥1 relay returns `["OK", event_id, true, ...]`, or `nostrSendFailedWithReason(messageId, reason)` if every relay rejects.
6. **Receive** subscribed events and pass to `nostrMessageReceived(senderId, data)`.
7. **Report disconnection** via `nostrStatusChanged(false)` if all relays drop.
8. **Reconnect** automatically per `autoReconnect`/`reconnectDelay` config.

### Subscription Filters

The Rust core builds NIP-01 filters scoped to the local routing tag (events addressed to this device); the tag is derived from the `userId`, so senders can compute it without a key exchange. Use `nostrGetSubscriptionFilter(subscriptionId)` to fetch the JSON filter to send via `["REQ", subscriptionId, filter]`. Use `nostrGetPublicKey()` to retrieve the install's hex-encoded signing public key (for diagnostics and self-event filtering); read it after MLS initialization, since that is when the persisted signing key is installed.

### Send Confirmation Loop

Confirmation is keyed on the Nostr `event_id` (SHA-256 of the canonical event), which the relay echoes back in `["OK", event_id, true, …]`. The platform must correlate that response to the protocol `messageId` it dequeued.

```
┌─────────────┐        ┌──────────────┐        ┌──────────────┐
│  Rust Core   │  poll  │   Platform   │  EVENT │   Nostr      │
│  send_queue  │───────►│   Bridge     │───────►│   Relay      │
│              │        │              │        │              │
│  pending_    │◄───────│  confirm/    │◄───────│  ["OK",...]  │
│  confirmation│ report │  fail        │ status │              │
└─────────────┘        └──────────────┘        └──────────────┘
```

Pending confirmations expire after 30 seconds and are counted as failures.

### Example: Platform Bridge Skeleton (Android/Kotlin)

```kotlin
class NostrBridge(
    private val protocol: OfflineProtocol,
    private val relays: List<String>,
    private val scope: CoroutineScope,
) {
    fun start() {
        protocol.setNostrTransportCallback(object : NostrTransportCallback {
            override fun onMessagesAvailable() {
                scope.launch(Dispatchers.IO) { drainQueue() }
            }
        })
        connectRelays()
    }

    private suspend fun drainQueue() {
        while (true) {
            val next = protocol.nostrGetNextMessage() ?: break
            val sent = publishToRelays(next.eventJson)
            if (sent) {
                // Confirmation deferred — wait for ["OK", event_id, true, ...]
                pending[next.eventId] = next.messageId
            } else {
                protocol.nostrSendFailedWithReason(next.messageId, "no relays connected")
            }
        }
    }

    fun onRelayOk(eventId: String, accepted: Boolean, reason: String?) {
        val messageId = pending.remove(eventId) ?: return
        if (accepted) protocol.nostrConfirmSent(messageId)
        else protocol.nostrSendFailedWithReason(messageId, reason ?: "rejected")
    }

    fun onIncomingEvent(senderHex: String, payload: ByteArray) {
        protocol.nostrMessageReceived(senderHex, payload.toList())
    }
}
```

## DORS Scoring

DORS scores Nostr alongside other transports using the standard multi-factor system. Nostr's profile reflects its strengths (reliability when relays accept) and weaknesses (relay round-trip latency, bandwidth ceiling).

| Factor | Weight | Rationale |
|--------|--------|-----------|
| Reliability | 35% | Most important — confirmation depends on relays accepting |
| Bandwidth | 20% | Limited by relay throughput, not raw radio |
| Congestion | 20% | Queue pressure / relay backpressure |
| Energy | 15% | Same radio as Internet, no extra cost |
| Load | 10% | Send queue utilization |

| Parameter | Value | Description |
|-----------|-------|-------------|
| Base score | 5 | Modest base, below Internet's `preferOnline` boost |
| Media penalty | 0 | Nostr can carry media transfers |
| Bandwidth max | 1,000,000 B/s | Practical relay-bounded ceiling |
| Energy baseline | 55 | Internet-class |
| Has signal | No | No RSSI; uses default signal score (50) |
| Tie-break priority | 4 (lowest) | Last resort across the standard transports |

### When DORS Selects Nostr

Nostr will be selected when:
- All higher-priority transports are unavailable (no Internet endpoint, no peers, no Reticulum)
- Internet is degraded enough that Nostr's reliability score wins outright
- Battery-aware escalation rules permit (Nostr is treated as high-power, same as Internet)

Nostr will **not** be selected when:
- A higher-priority transport is healthy and competitive
- Scores are tied (Nostr loses every tie-break)

## Identity & Privacy

- **The signing key is a per-install secret** — derived via HKDF from a random 32-byte secret persisted through the app's `MlsStorage` on first `initialize_mls`, never from any public identifier. Before storage is available the transport signs with an ephemeral key that rotates per process. Wiping app storage rotates the install's Nostr identity.
- **The routing tag is derived from `userId`** — running the same `userId` on two devices means they share an inbox (both receive events tagged to that ID), but each install still signs with its own key. Use distinct user IDs per device for separate inboxes.
- **Relays see metadata** — sender pubkey, recipient routing tag, event size, and timing are visible to every relay you publish through.
- **Payload is end-to-end encrypted by MLS** before reaching this transport. The Nostr layer does not add or replace encryption.
- **Telemetry scrubbing** — when telemetry `scrub_ids` is on (default), pubkeys flowing through the SDK's telemetry sink are SHA-256 hashed before emission.

## Troubleshooting

### Nostr Not Connecting

1. Verify `relayUrls` is non-empty and uses `wss://` (not `ws://`)
2. Check the device has Internet — Nostr is not a substitute for connectivity
3. Try a known-good public relay (`wss://relay.damus.io`) to isolate relay-specific issues
4. Watch the `transport_state` telemetry record for the Nostr transport's status transitions

### Messages Sent But Not Delivered

1. Verify both devices subscribe to filters matching their own routing tags (built automatically by the SDK)
2. Confirm at least one relay is shared between sender and recipient — relays do not federate
3. Check pending confirmation timeouts (30 s) — relays sometimes accept events without sending `["OK"]`

### "Signing failed" Errors After Retries

The signing path retries up to 3 times per message before permanently failing. Sustained failure usually means the recipient's routing tag could not be derived (corrupt recipient ID) or platform crypto is unavailable — surface the failure reason from `nostrSendFailedWithReason`.

### DORS Not Selecting Nostr

1. Verify `nostrEnabled: true` and at least one relay is connected
2. Check DORS scores — Nostr has the lowest tie-break priority and a modest base score
3. Confirm no higher-priority transport is healthier; install a `TelemetrySink` with `routingDiagnostic: true` to see the per-factor breakdown

## Further Reading

- [Nostr Protocol](https://nostr.com/) — Project overview
- [NIPs](https://github.com/nostr-protocol/nips) — Nostr Implementation Possibilities (the wire-format specs)
- [examples/nostr-example/](../examples/nostr-example/README.md) — Minimal RN app demonstrating end-to-end Nostr messaging
- [Transport Architecture](transport-architecture.md) — How all transports fit together
- [DORS Deep Dive](dors.md) — Transport selection algorithm
- [DORS Configuration](dors-configuration.md) — Tuning transport selection
- [Configuration Guide](configuration.md) — All SDK configuration options
