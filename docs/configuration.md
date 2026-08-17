# Configuration Guide

Complete guide to configuring the Offline Protocol SDK for different use cases.

The TypeScript examples below target the React Native package. Desktop apps on
macOS, Linux, and Windows can use the Python binding's snake_case
`ProtocolConfig`; see the [Python binding guide](../bindings/python/README.md).

## Configuration Structure

```typescript
{
  // Required fields
  appId: string,
  profile: string,        // Local namespace selector — NOT your identity.
                          // Your address is derived; read it with localAddress().
  
  // Optional configurations
  transports?: TransportsConfig,
  binaryWireEnabled?: boolean,    // Binary wire-codec kill switch (default: true)
  encryption?: EncryptionConfig,  // NEW: Auto-encryption settings
  dors?: DorsConfig,
  relay?: RelayConfig,
  path?: PathConfig,
  reliability?: ReliabilityConfig,
  network?: NetworkConfig,
}
```

## Use Case Configurations

### 1. Emergency Response App

**Requirements**: Maximum coverage, offline-only, high reliability.

```typescript
{
  appId: 'emergency-responder',
  profile: profile,
  
  transports: {
    ble: { enabled: true },
    wifiDirect: { enabled: true },
    internet: { enabled: false },  // Offline only
  },
  
  dors: {
    preferOnline: false,
    switchHysteresis: 10,  // More aggressive switching
    bleToWifiRetryThreshold: 1,  // Switch faster
    congestionDurationSecs: 5,  // Require 5s sustained congestion before escalating
    ttlEscalationHoldSecs: 30,  // Keep TTL alarm active for 30s
  },
  
  relay: {
    allowRelay: true,
    relayPriority: 'always',  // Always try to be relay
    minBatteryForRelay: 15,   // Lower threshold for emergencies
  },
  
  network: {
    initialTtl: 10,  // Higher TTL for wider coverage
  },
  
  reliability: {
    ack: {
      defaultTimeoutMs: 10000,  // Longer timeout
    },
    retry: {
      maxRetries: 5,  // More retries
      outboxMaxLifetimeMs: 2592000000,  // 30 days
    }
  }
}
```

### 2. Messaging App (Hybrid Mode)

**Requirements**: Online-first, automatic offline fallback, end-to-end encryption.

```typescript
{
  appId: 'chat-app',
  profile: profile,
  
  transports: {
    ble: { enabled: true },
    wifiDirect: { enabled: true },
    internet: { enabled: true },
  },
  
  // Auto-encryption enabled by default
  encryption: {
    enabled: true,           // Messages automatically encrypted
    autoKeyExchange: true,   // Key packages exchanged on discovery
    storePending: true,      // Queue messages until session established
    requireEncryption: true  // Fail-closed by default; set false for best-effort/plaintext
  },
  
  dors: {
    preferOnline: true,  // Online-first
    switchHysteresis: 15,
    switchCooldownSecs: 20,
    historyWindowSize: 12,
  },
  
  relay: {
    allowRelay: true,
    relayPriority: 'auto',
    minBatteryForRelay: 30,
  },
  
  network: {
    initialTtl: 8,  // Standard TTL
  }
}
```

### 3. File Sharing App

**Requirements**: High bandwidth, efficient chunking.

```typescript
{
  appId: 'file-share',
  profile: profile,
  
  transports: {
    ble: { enabled: false },  // BLE too slow for large files
    wifiDirect: { enabled: true },  // Prefer high bandwidth
    internet: { enabled: true },
  },
  
  dors: {
    preferOnline: true,
    bleToWifiRetryThreshold: 1,  // Quick escalation to WiFi
    queueRecoveryRatio: 0.4,  // De-escalate when queues recover to 40%
  },
  
  relay: {
    allowRelay: true,
    minBatteryForRelay: 40,  // Higher for heavy traffic
  },
  
  fileTransfer: {
    chunkSize: 64 * 1024,  // 64KB chunks for faster transfer
    maxFileSize: 500 * 1024 * 1024,  // 500MB max
  }
}
```

### 4. Off-Grid / Disaster Recovery App

**Requirements**: Maximum resilience, no infrastructure assumed, long-range.

```typescript
{
  appId: 'disaster-response',
  profile: profile,

  transports: {
    ble: { enabled: true },
    wifiDirect: { enabled: true },
    internet: { enabled: false },   // No infrastructure
    reticulum: { enabled: true },   // LoRa long-range fallback
  },

  dors: {
    preferOnline: false,
    switchHysteresis: 10,
    bleToWifiRetryThreshold: 1,
  },

  relay: {
    allowRelay: true,
    relayPriority: 'always',
    minBatteryForRelay: 15,
  },

  network: {
    initialTtl: 12,  // Higher TTL for sparse networks
  }
}
```

### 5. Battery-Conscious App

**Requirements**: Minimize power consumption.

```typescript
{
  appId: 'battery-saver-app',
  profile: profile,

  transports: {
    ble: { enabled: true },  // Low power
    wifiDirect: { enabled: false },  // Avoid high power WiFi
    internet: { enabled: true },
  },
  
  dors: {
    preferOnline: true,  // Internet when available
  },
  
  relay: {
    allowRelay: false,  // Don't relay to save battery
    relayPriority: 'never',
  },
  
  // Or if relay needed:
  relay: {
    allowRelay: true,
    relayPriority: 'auto',
    minBatteryForRelay: 50,  // Only relay with good battery
  }
}
```

### 6. Crowded Event (Dense Network)

**Requirements**: High congestion, many devices.

```typescript
{
  appId: 'event-app',
  profile: profile,
  
  transports: {
    ble: { enabled: true },
    wifiDirect: { enabled: true },
    internet: { enabled: true },
  },
  
  dors: {
    congestionQueueThreshold: 30,  // Lower threshold
    rssiSwitchThreshold: -80,  // Switch earlier on poor signal
  },
  
  path: {
    forwardToTopK: 2,  // Fewer relays to reduce congestion
    maxCongestionLevel: 0.6,  // Stricter congestion filtering
  },
  
  relay: {
    minBatteryForRelay: 50,  // Carry for others only well above half charge
  },
  
  reliability: {
    retry: {
      maxRetries: 2,  // Fewer retries to reduce traffic
    },
    dedup: {
      maxTrackedMessages: 20000,  // Track more in dense network
    }
  }
}
```

## Configuration Parameters

### Transport Configuration

| Parameter | Type | Default | Description |
|-----------|------|---------|-------------|
| `transports.ble.enabled` | boolean | true | Enable BLE mesh |
| `transports.wifiDirect.enabled` | boolean | true | Enable Wi-Fi Direct (Android only) |
| `transports.internet.enabled` | boolean | true | Enable Internet |
| `transports.reticulum.enabled` | boolean | false | Enable Reticulum mesh (requires external daemon) |
| `transports.nostr.enabled` | boolean | false | Enable Nostr relay transport (requires `relayUrls`) |
| `transports.nostr.relayUrls` | string[] | `[]` | Nostr relay WebSocket URLs (e.g. `["wss://relay.damus.io"]`) |
| `transports.nostr.connectionTimeout` | number | 30 | Connection timeout in seconds |
| `transports.nostr.autoReconnect` | boolean | true | Auto-reconnect on disconnect |
| `transports.nostr.reconnectDelay` | number | 1000 | Reconnect delay in ms |
| `transports.nostr.maxReconnectAttempts` | number | 0 | Max reconnect attempts per relay (0 = infinite) |

### Encryption Configuration

Controls automatic MLS end-to-end encryption. See [MLS Integration Guide](./mls-integration.md) for details.

| Parameter | Type | Default | Description |
|-----------|------|---------|-------------|
| `enabled` | boolean | true | Enable automatic encryption/decryption |
| `autoKeyExchange` | boolean | true | Auto-exchange key packages on peer discovery |
| `storePending` | boolean | true | Queue messages when no session exists |
| `requireEncryption` | boolean | true | Fail send unless encryption is applied (fail-closed) |
| `compactEnvelopeEnabled` | boolean | true | Emit the compact MLS envelope to recipients that advertise support (kill switch — see [Wire Format Kill Switches](#wire-format-kill-switches)) |
| `richPayloadEnabled` | boolean | true | Seal rich extras (reply context, media metadata, forward attribution) inside the MLS ciphertext for capable recipients (kill switch — see [Wire Format Kill Switches](#wire-format-kill-switches)) |
| `cryptoRecoveryEnabled` | boolean | true | Recover an undecryptable 1:1 message instead of dropping it and ACKing anyway (kill switch — see [Crypto-Failure Recovery](#crypto-failure-recovery)) |
| `pendingQueue.maxPendingPerPeer` | number | 64 | Max inbound encrypted messages held per peer awaiting session readiness |
| `pendingQueue.maxPendingGlobal` | number | 4096 | Max inbound encrypted messages held across all peers |
| `pendingQueue.pendingTtlMs` | number | 1800000 | TTL for held encrypted messages (30 minutes) |
| `pendingQueue.overflowPolicy` | string | `drop_oldest` | Overflow action: `drop_oldest` or `drop_newest` |

`pendingQueue` bounds the **inbound** pending-*decryption* queue — messages that
arrived before the MLS session or group state was ready. Under the deferred-ACK
model such a message is not delivery-ACKed on receipt, so this queue is the
primary recovery window before the session confirms; that is why the TTL default
is 30 minutes rather than the 2 minutes earlier releases used. The Rust
`PendingQueueConfig` additionally carries `max_pending_bytes_per_peer` (4 MiB) and
`max_pending_bytes_global` (32 MiB) — memory bounds that the count limits alone
cannot provide, since a queued media chunk is far larger than a text message.
Those two are not carried on the FFI dictionary; binding callers get the
defaults.

The **outbound** queue — messages you sent that are waiting for a session to be
established — is a different queue with its own bounds and its own configurable
lifetime (`pendingMessageMaxLifetimeMs`); see
[Reliability Configuration](#reliability-configuration) below.

Encryption is **required by default** (fail-closed): sends fail with a typed error
instead of ever silently degrading to plaintext — including when MLS was never
initialized. To deliberately operate in plaintext, set `requireEncryption: false`
explicitly; every plaintext send then emits a `PLAINTEXT_SEND` security warning
event (once per peer). Internal control messages (key exchange, connection
requests, service discovery) are exempt and unaffected.

Pending encrypted-message queue behavior (before MLS session readiness):
- Queueing is bounded by both per-peer and global limits.
- TTL eviction uses monotonic clock semantics.
- Overflow behavior is explicit and deterministic (`drop_oldest` / `drop_newest`).
- Every overflow/TTL drop emits structured warning logs with reason and triggered limit.

Under the default strict mode:
- `sendMessage` / `sendMessageViaTransport` fail fast with typed errors (`SessionNotReady`, `EncryptFailed`) and do not send transport payloads on failure. With `storePending: true` (default), messages for peers whose session is not yet confirmed are queued and sent encrypted once it is.
- `SessionNotReady` carries establishment progress (`NoKeyPackage`, `HaveKeyPackage`, `SessionPending`, `SessionConfirmed`) for retry/UI decisions.
- Internal control messages (`sendConnectionRequest`, `acceptConnectionRequest`, `rejectConnectionRequest`, key packages, service discovery) are exempt — they are plaintext bootstrap messages and continue to work.
- Inbound plaintext content is rejected — text messages and legacy media chunks alike. Rejected plaintext is never surfaced as `message_received` (plaintext carries no sender authentication, so anyone could inject it under a contact's name); a `SecurityWarning` with reason code `PLAINTEXT_RECEIVE_REJECTED` is emitted once per peer. Even with `requireEncryption: false`, inbound plaintext from a peer **known to run MLS** is rejected as a downgrade/forgery attempt. "Known to run MLS" means an MLS session with them exists, or they have signed a control message this install verified — not merely that a session is *confirmed*. An honest peer never sends cleartext in that state: while a session is pending its sender queues rather than downgrading, so plaintext from such a peer is always either an injection or a genuine downgrade. Peers that have shown no MLS signal at all are still readable, which is what makes `enabled: true` + `requireEncryption: false` usable for legacy interop.
- The `message_received` event carries `encrypted: true` when the content arrived MLS-encrypted and was auto-decrypted, and `encrypted: false` for plaintext accepted under the opt-out.

Rust migration note:
- `EncryptionConfig::default()` sets `require_encryption: true` (fail-closed). Nodes that never call `initialize_mls` now fail sends with `EncryptFailed` instead of silently sending plaintext.
- Disabling encryption (`enabled: false`) requires also setting `require_encryption: false` — config validation rejects the combination otherwise.
- If you construct `EncryptionConfig` with a struct literal, include `require_encryption` explicitly or use `..Default::default()`.

**Example: Disable auto-encryption (use manual MLS APIs)**:
```typescript
{
  encryption: {
    enabled: false,
    // Explicit opt-out required: plaintext operation is never implicit.
    requireEncryption: false,
  }
}
```

**Example: Auto-encrypt but require explicit key exchange**:
```typescript
{
  encryption: {
    enabled: true,
    autoKeyExchange: false,  // Must manually exchange key packages
    storePending: true,
    // requireEncryption defaults to true (strict, fail-closed)
  }
}
```

### Wire Format Kill Switches

The negotiated wire formats are advertised per peer via the signed key package
and can be disabled at runtime — without an SDK release — if a field interop
issue ever surfaces:

| Parameter | Type | Default | Description |
|-----------|------|---------|-------------|
| `binaryWireEnabled` (top level) | boolean | true | Emit the compact binary wire codec on mesh hops to peers that advertise support (`wire_versions`) |
| `encryption.compactEnvelopeEnabled` | boolean | true | Emit the compact MLS envelope on encrypted messages to recipients that advertise support (`env_versions`) |
| `encryption.richPayloadEnabled` | boolean | true | Seal the rich payload (quoted-reply context, rich media metadata, forward attribution) inside the MLS ciphertext toward recipients that advertise support (`rich_versions`) |

The three switches are independent — each format degrades separately.
Disabling a switch stops **advertising and emitting** that format, so both
directions fall back as key packages refresh: the wire and envelope switches
fall back to the permanent JSON floor, and disabling `richPayloadEnabled` drops
rich extras from outbound sends (messages degrade to plain text — rich fields
are never sent cleartext). Parsing of inbound compact/sealed formats stays on
regardless — the switches can never make a device unable to read a peer, and a
disabled fleet interoperates with an enabled one automatically.

```typescript
{
  binaryWireEnabled: false,         // Hop-local: mesh framing back to JSON
  encryption: {
    compactEnvelopeEnabled: false,  // End-to-end: MLS envelope back to JSON
    richPayloadEnabled: false,      // End-to-end: stop sealing rich extras
  }
}
```

### Crypto-Failure Recovery

`encryption.cryptoRecoveryEnabled` (default `true`) is a fourth runtime kill
switch. It is not a wire format — nothing is negotiated and no peer has to
support it — so it degrades independently of the three above.

An inbound encrypted message on an *established* 1:1 session can fail to
decrypt — most importantly when the session has fallen out of epoch sync with
the peer (the two sides disagree on the MLS epoch, e.g. after a fork). Such a
failure used to be delivery-ACKed and dropped: silent loss behind an ACK that
claimed delivery. With the switch on:

- the failure **withholds the delivery ACK**, so the sender keeps retrying
  instead of marking the message delivered;
- the sender **re-seals each resend** against the peer's current session, so the
  message is delivered rather than merely retried;
- *if* the failure is a proven epoch mismatch, a **rate-limited session re-key**
  (one per peer per 30 s, via a `session_reset` key package) additionally
  rebuilds the channel.

Failures that are *not* an epoch mismatch — AEAD/authentication failures,
discarded ratchet generations, malformed frames — get the un-ACK and the resend
re-seal, but never the re-key: turning every malformed frame into a session
teardown would be an unbounded churn vector.

Because these failures no longer settle the message, `message_decryption_failed`
is **advisory** and fires once per failed *attempt* (bounded by the sender's ACK
retry budget), not once per message. The terminal signals remain
`message_failed` and, for media, `file_receive_failed`.

**The re-key trigger is unauthenticated by construction**: an MLS epoch is
checked during framing validation, before the sender is verified, so any party
able to inject a frame can drive a re-key without key material or captured
ciphertext. It is safe because it is bounded — confined to that peer's own
session slot, one per peer per 30 s, destroying no queued message, and reported
as a `SESSION_REKEY_TRIGGERED` security warning. See
[MLS Integration](./mls-integration.md#crypto-failure-recovery) for the full
threat model and the residual.

| Parameter | Type | Default | Description |
|-----------|------|---------|-------------|
| `encryption.cryptoRecoveryEnabled` | boolean | true | Recover an undecryptable 1:1 message (un-ACK + resend re-seal; rate-limited re-key on epoch desync only) |

Setting it to `false` reverts to the legacy drop-and-ACK behaviour. Media chunks
have no resend re-seal (chunks are re-encoded, not replayed) and recover through
the `media_resend_required` path instead. See
[MLS Integration](./mls-integration.md#crypto-failure-recovery) for the full
mechanism.

### DORS Configuration

| Parameter | Type | Default | Description |
|-----------|------|---------|-------------|
| `preferOnline` | boolean | false | Prefer Internet when available |
| `switchHysteresis` | number | 15.0 | Min score improvement to switch |
| `switchCooldownSecs` | number | 20 | Cooldown after switching (seconds) |
| `bleToWifiRetryThreshold` | number | 2 | Retries before escalating |
| `rssiSwitchThreshold` | number | -85 | RSSI threshold (dBm) |
| `congestionQueueThreshold` | number | 50 | Queue depth for congestion |
| `stabilityWindowSecs` | number | 8 | Stability check window |
| `poorSignalDurationSecs` | number | 10 | Seconds RSSI must remain poor before escalating |
| `ttlEscalationThreshold` | number | 2 | TTL value considered near exhaustion |

### Relay Configuration

| Parameter | Type | Default | Description |
|-----------|------|---------|-------------|
| `allowRelay` | boolean | true | Allow device to act as relay |
| `minBatteryForRelay` | number | 30 | Min battery % for relay |
| `relayPriority` | string | 'auto' | 'auto', 'always', or 'never' |

### Mesh Forwarding Configuration

The shape of carrying other people's traffic, once `relay.relayPriority` and the
battery floors have already decided that this device does. Whether to forward at
all is `relay.allowRelay`'s job; this section is only *how*.

Applied at construction. There is no runtime update, because the governor takes
its snapshot when it is built and re-pointing it mid-flight would have to
rebuild the token buckets and suppression cache underneath in-flight forwards.

| Parameter | Type | Default | Description |
|-----------|------|---------|-------------|
| `maxTtl` | number | 8 | Hop budget a forwarded frame is clamped to |
| `denseMaxTtl` | number | 5 | Hop budget once the neighborhood is dense |
| `denseDegree` | number | 6 | Neighbor count at which the dense budget applies |
| `fanout` | number | 3 | Neighbors a frame is forwarded to (must be ≥ 1) |
| `jitterMinMs` | number | 20 | Shortest pre-transmit delay |
| `jitterMaxMs` | number | 200 | Longest pre-transmit delay at low density |
| `ratePerSec` | number | 10 | Sustained forwarding rate, frames per second |
| `burst` | number | 30 | Burst allowance above the sustained rate |
| `peerRatePerSec` | number | 5 | Sustained per-neighbor acceptance rate |
| `peerBurst` | number | 15 | Per-neighbor burst allowance |
| `queueCapacity` | number | 256 | Maximum forwards awaiting transmission |
| `biasMinScale` | number | 0.25 | Smallest share of full forwarding effort capability bias scales down to; `1.0` disables bias |
| `biasMaxHandicapMs` | number | 400 | Longest extra pre-transmit delay bias adds to a weaker device |
| `activityWindowMs` | number | 60000 | How long a stretch of forwarding activity is measured over |
| `activityMinForwards` | number | 3 | Frames carried in one window at or above which this device reads as an active relay |
| `activityIdleWindows` | number | 2 | Consecutive quiet windows before an active relay reads as inactive |

Every field is optional, and an omitted one keeps the default above rather than
being restated by a binding. That is deliberate: the defaults live in the Rust
core and nowhere else, so a section naming one dial moves only that dial. The
suppression-cache sizing (`seen`) is not exposed, being internal memory sizing
rather than a policy dial.

To read what is actually in force, including every default the app never set,
use `getMeshRelayTunables()`. Unlike the config, its result has every field
populated, so no caller needs a fallback literal. Counters are
`getMeshRelayStats()`; see [mesh.md](mesh.md#reading-the-numbers) for how to
read them.

### Path Configuration

| Parameter | Type | Default | Description |
|-----------|------|---------|-------------|
| `forwardToTopK` | number | 3 | Number of relays to forward to |
| `maxCongestionLevel` | number | 0.7 | Max congestion threshold (0-1) |

### Reliability Configuration

**ACK Config**:
| Parameter | Type | Default | Description |
|-----------|------|---------|-------------|
| `defaultTimeoutMs` | number | 10000 | ACK timeout (milliseconds) |
| `maxPendingAcks` | number | 1000 | Max pending ACKs |

**Retry Config**:
| Parameter | Type | Default | Description |
|-----------|------|---------|-------------|
| `maxRetries` | number | 10 | Max ACK retry attempts (transport retries are unlimited) |
| `initialDelayMs` | number | 1000 | Initial retry delay |
| `maxDelayMs` | number | 300000 | Max retry delay (5 min) |
| `backoffMultiplier` | number | 2.0 | Backoff multiplier |
| `outboxMaxLifetimeMs` | number | 604800000 | Max message lifetime (7 days) |
| `pendingMessageMaxLifetimeMs` | number | 604800000 | Max lifetime while waiting for MLS session establishment (7 days) |

**Fixed (not configurable) message-plane limits**, listed here because they can
surface as errors or as `message_failed` events:

| Limit | Value | Effect when exceeded |
|-------|-------|----------------------|
| `sendMessage` content size | 256 KiB | call fails with `InvalidArgument` — use `sendMedia`, which chunks |
| Pending queue, per peer | 64 messages / 2 MiB | oldest evicted with `message_failed` |
| Pending queue, global | 4096 messages / 16 MiB | globally oldest evicted with `message_failed` |
| Single protocol-state record | 4 MiB | refused on write, dropped on read |

Group sends have no durable pre-session queue and are exempt from the queue
bounds and the content cap. They are **not** otherwise unbounded: by default a
group send over the Internet transport is one ordinary message frame per member,
so each member's copy carries its own outbox entry, ACK, and retry ladder. See
[Message Delivery](message-delivery.md#reliability-parameters) for the reasoning
behind each bound, and [Group sends](message-delivery.md#group-sends) for the
per-member delivery path.

**Dedup Config**:
| Parameter | Type | Default | Description |
|-----------|------|---------|-------------|
| `maxTrackedMessages` | number | 1000 | Max message IDs to track (must be > 0) |
| `retentionTimeSecs` | number | 3600 | Retention time (1 hour; must be > 0) |

Both fields are now **rejected at `0`**. Neither failed safe: at
`maxTrackedMessages: 0` the exact-match tracker evicts on every insert, so it
holds a single id and duplicate suppression — a replay defence — was effectively
off for a configuration the SDK used to accept in silence;
`retentionTimeSecs: 0` expires every entry immediately for the same result. This
refuses only the degenerate value, not an unwise one: sizing the window for your
deployment is still your call.

### Group Configuration

| Parameter | Type | Default | Description |
|-----------|------|---------|-------------|
| `maxGroupMembers` | number | 256 | Maximum members in a single group (must be > 0) |
| `relayEnabled` | boolean | `true` | Register groups with the relay server |
| `relayBroadcastEnabled` | boolean | `true` | Allow a relay-synced group to send via one O(1) relay broadcast instead of per-member fan-out — taken only against a relay that advertised the `group_delivery_v3` capability |
| `enforceAdminCommits` | boolean | `false` | Refuse an incoming MLS membership commit whose committer the local admin overlay does not authorize, instead of applying and reporting it |

**These two relay flags are not the same switch.**

`relayEnabled` gates *registration*. The relay's group registry is what invite
links resolve against, so turning it off breaks invite links. Leave it on.

`relayBroadcastEnabled` gates the *send path*, and the flag alone never selects
it: the broadcast is additionally gated on the connected relay having advertised
the `group_delivery_v3` capability in its `Authenticated` answer — the settled
delivery-report contract *plus* an address-aware group path (members named by
the registered `off1…` identifiers, so the report is comparable against the
MLS roster; a `group_delivery_v2` relay's username-keyed path deliberately
fails this gate). Such a relay
answers every broadcast with a *settled* per-recipient delivery report, and the
SDK re-sends a per-member copy — through the ordinary outbox/ACK/park ladder —
to every MLS roster member the report does not account for, surfacing the
result as the `group_message_delivery_report` event. That report-plus-backstop
is what gives the broadcast a delivery contract and is why the default is now
**on**. Against an older relay the capability gate fails closed and every send
takes per-member fan-out; the v1 relay's fire-and-forget broadcast (no
presence check, no push fallback, no persistence, "sent" answered before
delivery was known — a miss was *undetectable*, since MLS application messages
do not advance the group epoch) is never taken regardless of this flag.

Set it to `false` to force per-member fan-out even against a capable relay: one
ordinary message frame per member, each inheriting the full direct-message
ladder — outbox, ACK, retry, relay write-ack, offline push carrying the
ciphertext, and park-on-unreachable with presence-driven flush. The cost is
O(N) frames, which does not risk the relay's rate limiter at any group size
(the bridge's own token bucket is tighter and defers rather than drops), but
does mean drain latency and, past roughly 118 members, duplicate sends from the
ACK timer starting at local enqueue — the strongest reason large groups should
leave the broadcast on. See
[Group sends](message-delivery.md#group-sends) for the full analysis.

#### `enforceAdminCommits` — opt-in, and a partition decision

Leaving this off does **not** mean unauthorized membership changes go
unnoticed. They are applied and reported: `group_unauthorized_membership_change`
fires and the affected `group_member_added` / `group_member_removed` events
carry `authorized: false`. The flag only decides whether the commit is *also*
refused.

Turning it on is a decision about partition risk, not a hardening tweak.
Refusing a commit means declining the MLS merge, so the refusing device's epoch
stays behind every member that accepted it — and MLS cannot heal that. The
device stops being able to decrypt the group and has to be re-invited by the
app. Enforcement is fork-free only if *every* member reaches the same verdict,
and the admin overlay is replicated best-effort: role changes ride
unreconciled mesh notifications, and a joiner receives a point-in-time
snapshot.

The check therefore fails open on every *absent* input — no group metadata, no
admin role stored yet, an unreadable roster — so the common "my role map is
behind" case merges normally. What it cannot detect is *divergent* knowledge:
two members who each hold a non-empty but disagreeing admin set will refuse
each other's commits. That residual risk is why this is opt-in. Enable it only
for a closed deployment that controls role distribution, and never on part of a
fleet — members with it off will apply a commit that members with it on
rejected. Note also that rejection is receiver-local: the sender's frame is
still acknowledged, so a committer gets no signal that anyone refused.

Pure key-update commits (which carry no membership change) and 1:1 sessions are
never gated.

**React Native:** the `group` section is plumbed through both bridges. Its
documented home is the nested object (top-level flat keys are also accepted,
camelCase or snake_case, nested winning over flat — the same shape rules as
`encryption`):

```typescript
const config = {
  group: {
    maxGroupMembers: 256,        // default
    relayEnabled: true,          // default — invite links depend on it
    relayBroadcastEnabled: true, // default — capability-gated; false forces per-member fan-out
    enforceAdminCommits: false,  // default — see the partition warning above before enabling
  },
};
```

### Network Configuration

| Parameter | Type | Default | Description |
|-----------|------|---------|-------------|
| `initialTtl` | number | 8 | Initial TTL for messages |

## Validation Rules

`ProtocolConfig::validate()` runs on construction — `OfflineProtocol::new` fails
with `InvalidConfiguration` rather than starting on a configuration that cannot
work. Nothing is partially applied.

**Identity and routing**

1. `appId` must not be empty
2. `profile` must not be empty
3. `initialTtl` must be > 0
4. At least one transport must be enabled

**ACK and retry**

5. `ack.defaultTimeoutMs` must be > 0
6. `ack.maxPendingAcks` must be > 0
7. `retry.initialDelayMs` and `retry.maxDelayMs` must be > 0, and
   `initialDelayMs` must be ≤ `maxDelayMs`
8. `retry.backoffMultiplier` must be finite and ≥ 1.0
9. `retry.outboxMaxLifetimeMs` and `retry.pendingMessageMaxLifetimeMs` must each
   be in `1..=i64::MAX`

**Deduplication**

10. `dedup.maxTrackedMessages` must be > 0
11. `dedup.retentionTimeSecs` must be > 0
12. When `useBloomFilter` is set: `bloomFilterBits`, `bloomHashCount`,
    `bloomFilterCount`, and `bloomRotationSecs` must each be > 0 (Rust-only —
    the FFI `DedupConfig` carries no Bloom fields)

**Encryption and groups**

13. `requireEncryption: true` requires `enabled: true`
14. `pendingQueue.maxPendingPerPeer` and `maxPendingGlobal` must be > 0, and
    `maxPendingGlobal` must be ≥ `maxPendingPerPeer`
15. `pendingQueue.pendingTtlMs` must be > 0
16. `maxGroupMembers` must be > 0

**Mesh forwarding**

17. `meshRelay.fanout` must be > 0. Zero is not a cheaper forward but a silent
    drop: the frame is already admitted and recorded as seen, so no copy goes
    out and this node's suppression entry stops it arriving by another path
18. `meshRelay.maxTtl` and `meshRelay.denseMaxTtl` must each be > 0. A hop
    ceiling of zero clamps every arriving budget to nothing, so the frame is
    refused before it is ever queued. The dense ceiling only applies in a
    crowded room, so a zero there fails exactly where the mesh is most needed
19. `meshRelay.queueCapacity` must be > 0. A queue that holds nothing refuses
    every admission as queue-full
20. `meshRelay.ratePerSec`, `burst`, `peerRatePerSec` and `peerBurst` must each
    be finite and > 0. The token buckets clamp their inputs to zero, so a
    negative, zero or `NaN` value yields a bucket that never releases a token
    and forwarding stops for good with no error and no counter
21. `meshRelay.biasMinScale` must be finite and in `(0.0, 1.0]`
22. `meshRelay.jitterMinMs` must not exceed `meshRelay.jitterMaxMs`. An
    inverted window collapses the delay spread to a single millisecond, so
    neighbors stop separating in time, and it would slip past rule 23 below
23. `meshRelay.jitterMaxMs + meshRelay.biasMaxHandicapMs` must stay under the
    5s overdue cut-off, past which a forward is abandoned rather than late
24. `meshRelay.activityWindowMs`, `activityMinForwards` and
    `activityIdleWindows` must each be > 0

Rules 17 through 20 all guard one failure: a dial that reads like a
conservative setting but is in fact an off switch, leaving the device running,
reporting no error, and carrying nothing. Refusing them at construction is what
keeps that distinguishable from a quiet neighborhood.

Note what is *not* validated: `minBatteryForRelay` is a `u8` and is clamped by
its type rather than range-checked. Earlier versions of this guide claimed it
was validated; it never was.

### Runtime updates are validated the same way

`updateAckConfig`, `updateRetryConfig`, and `updateDedupConfig` are **fallible**
(`Result` in Rust, `[Throws=ProtocolError]` over UniFFI — Swift callers need
`try`). Each builds the candidate configuration and runs the same
`ProtocolConfig::validate` above, rather than re-checking a hand-rolled subset
that would drift. On rejection the **previous configuration is kept**.

Two consequences worth knowing:

- A rejection can name a field you did not pass. The updaters validate the whole
  candidate configuration, so an already-installed bad value surfaces on the next
  unrelated update.
- On React Native, a `reliability` block passed to the `OfflineProtocol`
  **constructor** is applied during `start()`, where a rejection is logged and
  swallowed — the SDK keeps its defaults. **A silently-defaulted reliability block
  looks like it worked**; grep your logs for `Failed to apply … configuration`. A
  direct `updateDedupConfig(...)` call rejects the promise instead.

## Platform-Specific Considerations

### Android

**Available Transports**: Internet, BLE, Wi-Fi Direct, Reticulum, Nostr

**Permissions Required**:
- `BLUETOOTH`, `BLUETOOTH_SCAN`, `BLUETOOTH_CONNECT`
- `ACCESS_FINE_LOCATION` (for BLE scanning)
- `ACCESS_WIFI_STATE`, `NEARBY_WIFI_DEVICES`

### iOS

**Available Transports**: Internet, BLE, Reticulum, Nostr (no Wi-Fi Direct)

**Permissions Required**:
- `NSBluetoothAlwaysUsageDescription`

**Recommended Config**:
```typescript
{
  transports: {
    ble: { enabled: true },
    wifiDirect: { enabled: false },  // Not available on iOS
    internet: { enabled: true },
  }
}
```

### Desktop (Python)

The Python binding supports macOS, Linux, and Windows. Internet/WebSocket, BLE,
Reticulum, and Nostr are available; Wi-Fi Direct is not implemented on desktop.
Configuration uses the generated snake_case fields:

```python
from offline_protocol_sdk.offline_protocol import ProtocolConfig, OverflowPolicy

config = ProtocolConfig(
    app_id="desktop-app",
    profile="alice",
    ble_enabled=True,
    wifi_direct_enabled=False,
    internet_enabled=True,
    reticulum_enabled=False,
    nostr_enabled=False,
    prefer_online=True,
    initial_ttl=8,
    encryption_enabled=True,
    auto_key_exchange=True,
    store_pending=True,
    require_encryption=True,
    max_pending_per_peer=64,
    max_pending_global=4096,
    pending_ttl_ms=1_800_000,  # 30 min (the SDK default)
    overflow_policy=OverflowPolicy.DROP_OLDEST,
)
```

`ProtocolManager` also requires a `state_root` (or `OFFLINE_PROTOCOL_STATE_ROOT`
in the environment): Python has no portable uninstall-scoped container, so the
SDK refuses to guess one. **Your installer must remove that directory on
uninstall.**

Build and package details are in
[`bindings/python/README.md`](../bindings/python/README.md).

### Web

**Available Transports**: Internet only

**Recommended Config**:
```typescript
{
  transports: {
    ble: { enabled: false },        // Not available in browsers
    wifiDirect: { enabled: false }, // Not available in browsers
    internet: { enabled: true },
  }
}
```

## Advanced Tuning

### Low Battery Optimization

```typescript
{
  relay: {
    minBatteryForRelay: 50,  // Only relay with good battery
  },
  dors: {
    // Prefer low-power transports
  }
}
```

### High Reliability

```typescript
{
  reliability: {
    retry: {
      maxRetries: 5,
      outboxMaxLifetimeMs: 2592000000,  // 30 days
    }
  },
  path: {
    forwardToTopK: 5,  // More redundancy
  }
}
```

### Low Latency

```typescript
{
  dors: {
    switchHysteresis: 5,  // Switch faster
    bleToWifiRetryThreshold: 1,  // Escalate immediately
  },
  reliability: {
    ack: {
      defaultTimeoutMs: 2000,  // Shorter timeout
    }
  }
}
```

## Environment-Specific Configs

### Dense Urban Area

- More relays, lower TTL
- Stricter congestion management
- BLE preferred (short range sufficient)

### Open Rural Area

- Fewer relays, higher TTL
- Reticulum with LoRa preferred (multi-km range)
- Wi-Fi Direct for nearby high-bandwidth transfers
- Higher battery thresholds

### Indoor Building

- Medium TTL
- BLE mesh optimal
- Many relays (walls attenuate signal)

### High-Speed Movement

- Quick transport switching
- Lower hysteresis
- Shorter stability window
