# Reticulum Transport

## Overview

The Reticulum transport provides long-range, resilient mesh networking via the [Reticulum](https://reticulum.network/) network stack. It supports LoRa, TCP, UDP, serial, I2P, and other mediums, making it ideal for off-grid communication, disaster recovery, and infrastructure-sparse environments where BLE range is insufficient and Internet connectivity is unavailable.

Reticulum is one of five transports in the Offline Protocol SDK, alongside BLE, Wi-Fi Direct, Internet and Nostr. It is disabled by default because it requires external infrastructure (a running Reticulum instance, an RNode radio, or a gateway).

> **No counterpart daemon ships yet.** The Rust transport opens no Reticulum link of its own: it manages queues, metrics and the send-confirmation loop, and expects the platform to bridge to a real Reticulum stack. Both mobile managers speak the protocol below to a configurable address, and nothing in this repository or any companion repository answers on the other end. Until a daemon exists, enabling Reticulum gives you a transport that queues and never drains. See [the gateway contract](spec/gateway-contract.md), which specifies what that daemon has to be.

## When to Use Reticulum

| Scenario | Why Reticulum |
|----------|---------------|
| Off-grid / wilderness | LoRa reaches 2-15+ km line-of-sight, far beyond BLE's ~50m |
| Disaster response | Works without cell towers, Internet, or power infrastructure |
| Rural / sparse networks | Bridges gaps where devices are too far apart for BLE mesh |
| Censorship resistance | I2P transport option for anonymized routing |
| Hardware-constrained setups | RNode devices are inexpensive and self-contained |

Reticulum is **not** suitable for:
- High-bandwidth transfers (media, files) — typical LoRa throughput is ~0.7 KB/s, peak ~2.7 KB/s
- Low-latency applications — LoRa multi-hop paths can add seconds of latency
- Environments where all devices are within BLE range — BLE is faster and simpler

## Architecture

The Offline Protocol SDK's `ReticulumTransport` manages message queues, delivery metrics, and the confirmation loop on the Rust side. The **platform layer** is responsible for bridging to an actual Reticulum instance. There are several ways to achieve this bridge, each with different trade-offs (see [Integration Strategies](#integration-strategies) below).

```
┌──────────────────────┐
│  Offline Protocol    │
│  (Rust Core)         │
│                      │
│  ReticulumTransport  │◄── Transport trait implementation
│  - send_queue        │    (same interface as BLE, WiFi, Internet)
│  - receive_queue     │
│  - pending_confirm   │
│  - metrics           │
└──────────┬───────────┘
           │ Platform Bridge (UniFFI)
           ▼
┌──────────────────────┐
│  Platform Layer      │
│  (iOS/Android)       │
│                      │
│  Integration via:    │
│  - Embedded Python   │◄── Most proven (Sideband pattern)
│  - reticulum-rs      │◄── Pure Rust (emerging)
│  - HDLC IPC bridge   │◄── Desktop/server only
│  - TCP gateway       │◄── Via TCPClientInterface
└──────────┬───────────┘
           │
           ▼
┌──────────────────────┐
│  Reticulum Stack     │
│                      │
│  Interfaces:         │
│  - RNode (LoRa)      │
│  - TCP / UDP         │
│  - I2P               │
│  - Serial / KISS     │
│  - AutoInterface     │
└──────────────────────┘
```

## Important: Reticulum Integration Reality

Reticulum is a Python-first project. The [reference implementation](https://github.com/markqvist/Reticulum) (`pip install rns`) is the authoritative definition of the protocol. There is **no stable C API or documented wire protocol spec** for external programs to use. This has important implications for mobile integration.

**How existing Reticulum apps work:** Every production Reticulum mobile app (notably [Sideband](https://github.com/markqvist/Sideband)) embeds the full Python runtime and RNS library into the app using frameworks like python-for-android. They do not connect to an external daemon via TCP.

**The shared instance IPC** (`rnsd` daemon on port 37428) uses an internal protocol (HDLC-framed Reticulum packets over a Unix domain socket, with TCP as fallback). This is designed for multiple Python programs on the same host to share one Reticulum instance — not as a general-purpose API for non-Python apps.

This means the platform bridge you implement must choose one of the strategies described below, depending on your deployment target.

## Integration Strategies

### Strategy 1: Embedded Python (Most Proven — Mobile)

Embed the Python runtime and RNS library directly in your mobile app. This is how Sideband works and is the most battle-tested approach.

**Android:** Use [Chaquopy](https://chaquo.com/chaquopy/) or [python-for-android](https://github.com/kivy/python-for-android) to embed a Python interpreter. Call `import RNS` from embedded Python and bridge to your native code.

**iOS:** Use [Kivy-iOS](https://github.com/kivy/kivy-ios) or a similar Python embedding framework. Note that iOS Reticulum support is still maturing — currently limited to TCP and multicast UDP interfaces (no BLE/serial yet).

**Pros:** Full Reticulum protocol support, proven by Sideband
**Cons:** Adds ~30MB+ for the Python runtime, complex build setup

### Strategy 2: reticulum-rs Crate (Emerging — Pure Rust)

The [`reticulum`](https://crates.io/crates/reticulum) crate by BeechatNetworkSystemsLtd is a Rust port of the Reticulum protocol stack. It was presented at FOSDEM 2026 and is under active development.

If `reticulum-rs` achieves full wire-format compatibility with the Python reference, it could be linked directly into the Offline Protocol SDK with zero Python dependency. This is the ideal long-term path.

**Pros:** Native Rust, no Python dependency, small binary (<1MB)
**Cons:** Still maturing; wire-format interoperability with the Python reference is not yet guaranteed

### Strategy 3: HDLC Shared Instance Bridge (Desktop/Server)

On desktop/server platforms where `rnsd` is running, a non-Python app can connect to the shared instance socket and exchange HDLC-framed raw Reticulum packets:

- **Linux/macOS:** Unix domain socket (abstract socket `\0rns/default`)
- **TCP fallback:** `127.0.0.1:37428` (used when domain sockets are unavailable, or when `shared_instance_type = tcp` is set in config)

The HDLC framing uses `0x7E` flag bytes with `0x7D` escape byte-stuffing. The payloads are raw Reticulum wire-format packets.

**Critical caveat:** This gives you raw packet-level access only. You still need to implement Reticulum's cryptography (X25519, Ed25519, AES-256-CBC, HMAC-SHA256), identity management, destination resolution, and link establishment yourself. The shared instance is a packet relay, not a high-level messaging API.

**Pros:** Lightweight, no Python in your process, uses system daemon
**Cons:** Desktop only, requires reimplementing Reticulum protocol logic, undocumented IPC protocol

### Strategy 4: TCP Gateway (Simplest — Any Platform)

Connect to a remote Reticulum transport node using Reticulum's `TCPClientInterface`. In this model, a Reticulum gateway server handles the protocol complexity, and your app communicates with it over a standard TCP connection.

This requires a Reticulum node configured as a TCP server that your app connects to as a client. The Reticulum transport node handles identity, routing, and encryption.

**Pros:** Standard TCP, works on any platform, no local daemon required
**Cons:** Requires a remote gateway server, adds network dependency

## Configuration

### Enabling Reticulum

**React Native (TypeScript)**:
```typescript
const protocol = new OfflineProtocol({
  appId: 'my-app',
  profile: 'user123',
  transports: {
    ble: { enabled: true },
    reticulum: { enabled: true },
  },
});
```

**Kotlin (Android)**:
```kotlin
val config = ProtocolConfig(
    appId = "my-app",
    profile = "user123",
    bleEnabled = true,
    reticulumEnabled = true,
    // ... other fields
)
```

**Swift (iOS)**:
```swift
let config = ProtocolConfig(
    appId: "my-app",
    profile: "user123",
    bleEnabled: true,
    reticulumEnabled: true,
    // ... other fields
)
```

### Transport-Specific Constants

| Constant | Value | Description |
|----------|-------|-------------|
| Connection timeout | 60s | Enforced by the native managers, which each hold their own constant. The identically-valued Rust `ReticulumConfig` field is not read by anything. |
| Pending confirmation timeout | 120s | Time before treating an unconfirmed send as failed (vs 15s for Internet). Enforced in Rust. |
| Max payload size | 64 KB | Declared as `RETICULUM_MAX_PAYLOAD_SIZE` and **not currently enforced anywhere**: no code reads it. Treat it as intent, not as a limit you can rely on. |
| Reticulum encrypted MDU | 383 bytes | Single-packet maximum for encrypted data; plain MDU is 465 bytes |
| Reticulum MTU | 500 bytes | Total wire-format maximum including headers |

The longer timeouts reflect the high-latency reality of LoRa multi-hop paths.

## Platform Bridge Lifecycle

Regardless of which integration strategy you choose, the platform bridge interacts with `ReticulumTransport` through the same UniFFI API:

1. **Initialize** your Reticulum integration (embedded Python, `reticulum-rs`, gateway connection, etc.)
2. **Report status** via `reticulumStatusChanged(true)` when the Reticulum stack is ready
3. **Poll for outgoing messages** via `reticulumGetNextMessage()` in a loop
4. **Send** each message through your Reticulum integration
5. **Confirm** each send via `reticulumConfirmSent(messageId)` or `reticulumSendFailed(messageId)`
6. **Receive** incoming messages and pass to `reticulumDataReceived(data)` or `reticulumDataReceivedFrom(data, peerId)`
7. **Report disconnection** via `reticulumStatusChanged(false)` on connection loss
8. **Reconnect** automatically if `auto_reconnect` is enabled (default)

### Daemon TCP Protocol

> **Normative home:** this protocol is specified in
> [the gateway contract](spec/gateway-contract.md#gateway-daemon-contract-v1),
> which is the document to implement against. What follows describes what the
> shipped managers do today: contract v1 minus the verbs that make a gateway a
> gateway (address declaration, per-recipient verdicts, presence, capabilities,
> the version field). `Identify`, `SendMessage`, `MessageReceived` and
> `StatusUpdate` are the whole of what they speak.
>
> One difference is easy to miss when implementing the daemon side, because it
> is a silence rather than a message: **the shipped clients confirm a send on
> the socket write, not on a daemon answer.** They call
> `reticulumConfirmSent()` as soon as the line is written and ignore both
> `MessageSent` and `DeliveryError`, so a daemon that answers contract v1
> correctly gets no verdict handling from today's bridges. Teaching them to
> settle on the daemon's answer is part of the transport work, not the daemon's.
>
> Note what this section used to imply and does not: **no Reticulum daemon
> speaks this protocol.** `rnsd`'s own shared-instance IPC is HDLC-framed
> Reticulum packets over a Unix domain socket (Strategy 3 above), not this. This
> is a bespoke protocol whose counterpart has never existed, which is why the
> gateway contract promotes it rather than inventing a third one.

The built-in `ReticulumManager` (iOS and Android) speaks a newline-delimited JSON protocol over TCP to a configurable `daemonAddress` (default `localhost:4242`). Both platforms implement the same message types to stay in sync.

**Client-to-daemon messages:**

| Type | Fields | Description |
|------|--------|-------------|
| `Identify` | `device_id` (string) | Sent immediately after TCP connect to register this device with the daemon |
| `SendMessage` | `recipient` (string), `content` (string), `encoding` (string, `"base64"`), `reply_to_msg` (string, optional) | Request the daemon to deliver a message to a recipient. Content is base64-encoded binary. |

**Daemon-to-client messages:**

| Type | Fields | Description |
|------|--------|-------------|
| `MessageReceived` | `sender` (string), `content` (string), `encoding` (string, optional `"base64"`) | An incoming message from a remote peer. If `encoding` is `"base64"`, content is base64-decoded; otherwise treated as UTF-8. |
| `StatusUpdate` | `status` (string) | Informational daemon status (logged, not acted upon) |

All messages are JSON objects terminated by a newline (`\n`). Each TCP read is buffered and split on newlines to handle partial reads.

### Send Confirmation Loop

The send confirmation loop is critical for accurate DORS scoring. Without it, DORS cannot measure Reticulum's actual delivery performance.

```
┌─────────────┐        ┌──────────────┐        ┌──────────────┐
│  Rust Core   │  poll  │   Platform   │  send  │  Reticulum   │
│  send_queue  │───────►│   Bridge     │───────►│  Stack       │
│              │        │              │        │              │
│  pending_    │◄───────│  confirm/    │◄───────│  delivery    │
│  confirmation│ report │  fail        │ status │  callback    │
└─────────────┘        └──────────────┘        └──────────────┘
```

**Important**: Messages enter `pending_confirmation` state when dequeued by `reticulumGetNextMessage()`. The platform **must** call either `reticulumConfirmSent(messageId)` or `reticulumSendFailed(messageId)` for every message. Unconfirmed messages automatically expire after 120 seconds and are counted as failures.

### Example: Platform Bridge Skeleton (Android/Kotlin)

This shows the SDK-facing bridge logic. The actual Reticulum communication (`sendViaReticulum` / `receiveFromReticulum`) depends on your chosen integration strategy.

```kotlin
class ReticulumBridge(
    private val protocol: OfflineProtocol,
    private val scope: CoroutineScope
) {
    fun onReticulumReady() {
        protocol.reticulumStatusChanged(true)
        scope.launch(Dispatchers.IO) { sendLoop() }
    }

    fun onReticulumDisconnected() {
        protocol.reticulumStatusChanged(false)
    }

    private suspend fun sendLoop() {
        while (isActive) {
            val next = protocol.reticulumGetNextMessage()
            if (next != null) {
                val (messageId, data) = next
                try {
                    sendViaReticulum(data) // Your integration strategy
                    protocol.reticulumConfirmSent(messageId)
                } catch (e: Exception) {
                    protocol.reticulumSendFailed(messageId)
                }
            } else {
                delay(100)
            }
        }
    }

    fun onDataReceived(data: ByteArray, peerId: String) {
        protocol.reticulumDataReceivedFrom(data.toList(), peerId)
    }

    // Implement based on your chosen strategy:
    // - Embedded Python: call RNS.Packet.send() via Chaquopy
    // - reticulum-rs: call the Rust crate directly
    // - TCP gateway: write to TCP socket
    private suspend fun sendViaReticulum(data: ByteArray) { /* ... */ }
}
```

## DORS Scoring

DORS evaluates Reticulum alongside all other transports using the same multi-factor scoring system. Reticulum's scoring profile reflects its characteristics:

### Scoring Weights

| Factor | Weight | Rationale |
|--------|--------|-----------|
| Reliability | 30% | Most important — if Reticulum delivers, that's what matters |
| Energy | 25% | LoRa is relatively energy-efficient |
| Proximity | 20% | Hop count matters on multi-hop LoRa paths |
| Congestion | 15% | Queue pressure signals overload |
| Signal | 5% | RSSI from radio interfaces (when available) |
| Bandwidth | 5% | Low weight because bandwidth is inherently limited |

### Scoring Parameters

| Parameter | Value | Description |
|-----------|-------|-------------|
| Base score | 0 | No bonus — must earn selection on merit |
| Media penalty | -40 | Strongly penalized when message requests `transport_preference = "internet"` |
| Bandwidth max | 2,700 B/s | LoRa peak throughput at SF7/BW500kHz for normalization |
| Bandwidth default | 20 | Conservative default score when no measurement available |
| Energy baseline | 75 | Between BLE (90) and Internet (60) |
| High-power | No | Not penalized during low battery |
| Tie-break priority | 3 (second-lowest) | Internet (0) > WiFi Direct (1) > BLE (2) > Reticulum (3) > Nostr (4) |

### LoRa Throughput Reference

Actual throughput depends heavily on LoRa parameters. These are raw bitrates before Reticulum protocol overhead:

| Configuration | Raw Bitrate | Effective Throughput | Range |
|--------------|-------------|---------------------|-------|
| SF7 / BW500kHz / CR4:5 | ~21.9 kbps | ~2.7 KB/s | Short |
| SF7 / BW125kHz / CR4:5 | ~5.5 kbps | ~0.67 KB/s | Medium |
| SF8 / BW125kHz / CR4:5 | ~3.1 kbps | ~0.38 KB/s | Long |
| SF12 / BW125kHz / CR4:5 | ~0.29 kbps | ~0.04 KB/s | Maximum |

Higher spreading factors (SF) increase range at the cost of throughput. The SDK uses 2,700 B/s as the peak normalization value for DORS scoring.

### When DORS Selects Reticulum

Reticulum will be selected when:
- Other transports are unavailable (Internet down, no BLE peers in range, no WiFi Direct)
- Reticulum has significantly better reliability scores than degraded alternatives
- Battery is low and high-power transports (WiFi Direct, Internet) are penalized

Reticulum will **not** be selected when:
- Higher-bandwidth transports are available with comparable reliability
- The message requests Internet preference (media/file transfers)
- Scores are tied (tie-break favors Internet > WiFi > BLE > Reticulum > Nostr)

## File Transfer Behavior

Reticulum uses BLE-like chunk sizes for file transfers due to its low bandwidth:

| Parameter | Value | Comparison |
|-----------|-------|------------|
| Chunk size | BLE chunk size | Same as BLE (smaller chunks for low throughput) |
| Media window | BLE media window | Same as BLE |
| Media transfer eligibility | Excluded | Reticulum is not in the preferred transport list for media transfers |

Large file transfers over Reticulum are technically possible but will be very slow. The SDK automatically excludes Reticulum from the preferred media transfer transport list. If Reticulum is the only available transport, transfers will still work but at LoRa speeds.

## Reticulum Setup

The Offline Protocol SDK does not include a Reticulum stack — you must provide one via your chosen integration strategy. Below is reference information for setting up the Python Reticulum daemon, which is useful for desktop deployments and as a gateway for mobile apps.

### Installing Reticulum

```bash
pip install rns       # Standard installation
pip install rnspure   # Dependency-free variant for constrained systems
```

This installs the daemon (`rnsd`) and CLI tools (`rnstatus`, `rnpath`, `rnprobe`, `rncp`, `rnx`, `rnodeconf`, `rnid`).

### RNode Hardware

An [RNode](https://unsigned.io/rnode/) is a LoRa transceiver that runs open-source firmware. RNode hardware includes:

- **ESP32-based:** LilyGO T-Beam, T3S3, LoRa32; Heltec LoRa32; Unsigned RNode v2.x
- **nRF52-based:** RAK4631, LilyGO T-Echo, Heltec T114

Connection methods: USB serial, Bluetooth, or WiFi/TCP.

Radio bands: 433 MHz, 868 MHz, 915 MHz, and 2.4 GHz ISM bands.

### Configuration File

The Reticulum configuration lives at `~/.reticulum/config` (created on first run). A minimal configuration for LoRa:

```ini
[reticulum]
  enable_transport = True
  share_instance = Yes

[logging]
  loglevel = 4

[interfaces]
  [[Default Interface]]
    type = AutoInterface
    enabled = Yes

  [[RNode LoRa Interface]]
    type = RNodeInterface
    port = /dev/ttyUSB0
    frequency = 867200000
    bandwidth = 125000
    txpower = 7
    spreadingfactor = 8
    codingrate = 5
```

### Interface Types

Reticulum supports many interface types. The most relevant:

| Interface | Use Case |
|-----------|----------|
| `RNodeInterface` | LoRa via RNode hardware (USB/BLE/WiFi) |
| `AutoInterface` | Automatic local LAN discovery |
| `TCPClientInterface` | Connect to a remote Reticulum TCP server |
| `TCPServerInterface` | Accept incoming TCP connections |
| `UDPInterface` | UDP transport |
| `I2PInterface` | Anonymized routing over I2P |
| `SerialInterface` | Raw serial port |
| `KISSInterface` | KISS TNC protocol |
| `PipeInterface` | Named pipe / stdin/stdout |

### Running the Daemon

```bash
rnsd              # Start daemon (foreground)
rnsd --service    # Start as background service
rnstatus          # Check interface status
rnstatus -a       # Show all interfaces
rnpath -t         # Show routing table
```

## Lock Ordering

When acquiring multiple locks in `ReticulumTransport`, follow this order to prevent deadlocks:

1. `status`
2. `pending_confirmation`
3. `send_queue`
4. `metrics`
5. `receive_queue`
6. `reconnect_attempts` / `platform_handle`

This is documented in the source at `crates/offline-protocol-transport/src/reticulum.rs`.

## Metrics and Monitoring

Reticulum reports the same `TransportMetrics` as other transports:

| Metric | Source | Notes |
|--------|--------|-------|
| `rssi` | Radio interface (if available) | LoRa RSSI from RNode |
| `latency_ms` | Platform measurement | Round-trip through Reticulum stack |
| `bandwidth_bps` | Platform estimate | ~700 typical (SF7/BW125), ~2,700 peak (SF7/BW500) |
| `congestion` | Auto-calculated | Based on send queue depth |
| `queue_depth` | Auto-tracked | Current send queue size |
| `success_count` | Confirmation loop | Incremented on `confirmSent` |
| `failure_count` | Confirmation loop | Incremented on `reportSendFailure` or timeout |
| `delivery_ratio` | Auto-calculated | `success / (success + failure)` |

The `update_metrics` method preserves confirmation loop counts (success/failure) when the platform reports new metrics, preventing count resets.

## Reconnection

Reconnection is owned entirely by the native managers, and is configured from
the app's transport config, **not** from the Rust `ReticulumConfig`, whose
fields are inert (nothing constructs it with non-default values, and
`reconnect_delay` has no reader at all).

| Parameter | Where it lives | Default | Description |
|-----------|----------------|---------|-------------|
| `daemonAddress` | app config (`transports.reticulum`) | `localhost:4242` | Host and port of the daemon |
| `autoReconnect` | app config | `true` | Reconnect after disconnection |
| `maxReconnectAttempts` | app config | `0` (infinite) | Attempts before giving up |
| Backoff | native managers | 1s doubling to 30s | Not configurable |

On disconnection:
1. All pending confirmations are failed immediately
2. Send queue is preserved (messages will be sent after reconnection)
3. Reconnection attempts begin after the current backoff interval
4. Reconnect counter resets on successful connection

## Troubleshooting

### Reticulum Not Connecting

1. Verify the Reticulum stack is running: `rnstatus`
2. Check that the shared instance is enabled: `share_instance = Yes` in `~/.reticulum/config`
3. Check daemon logs: `~/.reticulum/logfile`
4. For TCP gateway: verify the remote host is reachable and the port is open

### Messages Not Delivering

1. Check `reticulumConfirmSent` / `reticulumSendFailed` are being called for every message
2. Verify Reticulum has active interfaces: `rnstatus -a`
3. Check if pending confirmations are timing out (120s) — may indicate the Reticulum stack is not reporting delivery
4. Monitor DORS transport switch events — Reticulum may be deprioritized if other transports score higher

### High Failure Rate

1. Check LoRa signal quality (RSSI) — move devices or adjust antenna
2. Lower `spreadingfactor` in Reticulum config for higher throughput (at cost of range)
3. Increase `txpower` if permitted by regulations
4. Check for radio interference on the frequency band
5. Verify both ends are using compatible LoRa parameters (frequency, bandwidth, SF)

### DORS Not Selecting Reticulum

1. Verify `reticulumEnabled: true` in config
2. Verify `reticulumStatusChanged(true)` was called after the Reticulum stack is ready
3. Check DORS scores — Reticulum has a low tie-break priority (only Nostr is lower), so it needs to outscore alternatives
4. Reticulum is excluded from media transfers by design

## Further Reading

- [Reticulum Network](https://reticulum.network/) — Official Reticulum documentation
- [Reticulum GitHub](https://github.com/markqvist/Reticulum) — Reference implementation (Python)
- [reticulum-rs](https://crates.io/crates/reticulum) — Rust port (emerging)
- [Sideband](https://github.com/markqvist/Sideband) — Reference mobile Reticulum messenger
- [RNode](https://unsigned.io/rnode/) — LoRa transceiver hardware
- [RNode Firmware](https://github.com/markqvist/RNode_Firmware) — Open-source RNode firmware
- [Transport Architecture](transport-architecture.md) — How all transports fit together
- [DORS Deep Dive](dors.md) — Transport selection algorithm
- [DORS Configuration](dors-configuration.md) — Tuning transport selection
- [Configuration Guide](configuration.md) — All SDK configuration options
