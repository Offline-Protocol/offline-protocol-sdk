# Nostr Transport

## Overview

The Nostr transport routes messages over [Nostr](https://nostr.com/) relays via WebSockets, providing a censorship-resistant, decentralized fallback when direct mesh and ordinary Internet endpoints are unreachable. Addressing uses a public *routing tag* deterministically derived from the `userId`, so peers can compute where to send without exchanging keys. Relays simply rebroadcast the signed events to subscribers.

Outgoing frames are **sealed into [NIP-59](https://github.com/nostr-protocol/nips/blob/master/59.md) gift wraps** (kind `1059`, [NIP-44 v2](https://github.com/nostr-protocol/nips/blob/master/44.md) inner encryption), each signed by a fresh single-use key. A relay sees an opaque routing tag, an unlinkable per-event pubkey, a jittered timestamp, and ciphertext — and nothing that identifies either party. See [What a relay can see](#what-a-relay-can-see).

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
- Hiding *that* a given user is reachable — the routing tag is derived from the `userId`, so anyone who can guess a username can watch that inbox for traffic volume and timing
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
| `sealingEnabled` | boolean | `true` | Seal outgoing frames into NIP-59 gift wraps |
| `coldContactEnabled` | boolean | `true` | Publish key packages and resolve peers' — see [Published key packages](#published-key-packages-and-cold-first-contact) |

### Transport-Specific Constants

| Constant | Value | Description |
|----------|-------|-------------|
| Connection timeout | 30 s | Default time to wait for relay connection |
| Pending confirmation timeout | 30 s | Time before treating an unconfirmed publish as failed |
| Max signing retries | 3 | Per-message signing-failure budget before permanent fail |
| Max event size | 64 KiB | Measured on the whole `["EVENT", …]` message. Over-cap events are dropped permanently (no retry) — see below |
| Initial query limit | 500 | `limit` on the REQ filter, capping stored-event replay per (re)connect |
| First-run backfill | 24 h | How far back `since` reaches when no receive watermark exists yet |
| `since` overlap | 1 h + 5 min | Jitter window plus clock-skew margin subtracted from the watermark |
| `created_at` jitter | 1 h | Gift-wrap timestamps are randomized uniformly into the *past* by up to this much. Must stay ≤ the `since` overlap above, or a jittered event falls outside the very query meant to fetch it |
| Max tracked peer keys | 1000 | Peers whose advertised Nostr key is remembered for sealing; at capacity the map resets. A peer who publishes key packages is re-resolved on the next send; one who does not stays on the bootstrap leg until re-exchange or restart |
| Key-package slots | 5 | Single-use key packages published for cold contact, one per addressable slot |
| Resolution retry interval | 5 min | Minimum gap between resolution attempts for the same peer. Not consumed by a request the queue refuses for capacity |
| Max pending resolutions | 64 | Queued and in-flight peer lookups |
| Max events per query | 64 | Records one resolution query will accept, opened or not — a relay may ignore the REQ's `limit`. Duplicates of a record already taken are additionally dropped by event id |
| Publication backoff | 60 s → 30 min | Delay before republishing a slot after *consecutive* publication failures; the first failure retries on the next refresh |
| Future-dated tolerance | 15 min | How far ahead of local time an event's `created_at` may sit and still advance the watermark |
| DORS tie-break priority | 4 (lowest) | Internet (0) > WiFi Direct (1) > BLE (2) > Reticulum (3) > Nostr (4) |
| Bandwidth max | 1 MB/s | Practical upper bound for relay-bounded throughput |
| Energy baseline | 55 | Same radio class as Internet |

The event-size cap applies to the complete relay message, because that is what a
relay accepts or rejects — it is larger than the protocol message inside it by
the base64 overhead. An oversized event is dropped on the first attempt rather
than retried (no number of attempts shrinks it) and is counted as a transport
failure so DORS steers away from Nostr.

This bites media in particular. `binary_content` serializes as a JSON array of
decimal numbers (~3.6×) before the event's base64 (~1.33×), so a chunk at the
engine's 32 KiB `DEFAULT_CHUNK_SIZE` is ~156 KB on the wire — over this cap and
over the 64–128 KB relays typically accept. Such chunks were never deliverable;
they now fail at the transport instead of being dropped at the relay. A 4 KiB
chunk still fits, so apps sending media over Nostr should lower `chunkSize`
accordingly.

## Platform Bridge Lifecycle

The platform bridge interacts with `NostrTransport` through these UniFFI calls:

1. **Initialize** the WebSocket pool against the configured relay URLs.
2. **Report status** via `nostrStatusChanged(true)` once at least one relay is connected.
3. **Drain outgoing messages** event-driven via the `NostrTransportCallback.on_messages_available()` hook (with `nostrGetNextMessage()` as the polling fallback).
4. **Publish** the `event_json` to every connected relay.
5. **Confirm** via `nostrConfirmSent(messageId)` when ≥1 relay returns `["OK", event_id, true, ...]`, or `nostrSendFailedWithReason(messageId, reason)` if every relay rejects.
6. **Receive** subscribed events and pass to `nostrMessageReceivedAt(senderId, data, createdAt)`, with `createdAt` taken verbatim from the event's `created_at` field.
7. **Issue key-package queries** — see [Resolution queries](#resolution-queries) below.
8. **Report disconnection** via `nostrStatusChanged(false)` if all relays drop.
9. **Reconnect** automatically per `autoReconnect`/`reconnectDelay` config.

### Resolution queries

Alongside the standing message subscription, the transport asks the platform to
run short-lived queries that fetch a peer's published key packages. The contract
is four calls:

```
nostrGetNextQuery()                        -> NostrQuery? { queryId, reqJson }
nostrQueryEventReceived(queryId, eventJson)   // one matching event
nostrQueryCompleted(queryId)                  // after the relay's EOSE
```

1. Poll `nostrGetNextQuery()` on the same timer that drains outgoing messages.
2. Send `reqJson` to **every** connected relay, using `queryId` verbatim as the
   NIP-01 subscription id. Broadcast rather than primary-only: a peer's records
   may sit on relays we share with them but not with the first in our list, and
   nothing would tell us we asked the wrong one.
3. Route `["EVENT", queryId, {...}]` to `nostrQueryEventReceived` with the event
   **object** (the third element) serialized — *not* through the message path.
   These are not messages; their content is sealed to a different key.
4. On `["EOSE", queryId]`, send `["CLOSE", queryId]` and call
   `nostrQueryCompleted`. The first EOSE closes it: leaving the subscription
   open for the other relays' stragglers would keep a live filter on a peer's
   routing tag for the life of the connection.
5. If no relay is connected when a query is drained, call
   `nostrQueryCompleted(queryId)` immediately rather than dropping it — the
   transport otherwise holds an entry no answer will arrive for.
6. When *all* relays drop, release every in-flight query the same way. A query
   issued just before a disconnect never sees an EOSE, so without this the
   bridge holds its subscription id for the life of the process and the
   transport holds the entry until its own cap evicts something — possibly a
   live query. Nothing is lost: the next send to those peers re-queues the
   lookup once the resolution rate limit lapses.

Both bundled bridges implement this. A bridge that does not is unaffected apart
from losing cold contact: publication still works (records ride the ordinary
`nostrGetNextMessage` path, whose `event_json` is opaque to the bridge), and
sends fall back to the bootstrap leg as before.

### Subscription Filters

The Rust core builds NIP-01 filters scoped to the local routing tag (events addressed to this device); the tag is derived from the `userId`, so senders can compute it without a key exchange. Use `nostrGetSubscriptionFilter(subscriptionId)` to fetch the JSON filter to send via `["REQ", subscriptionId, filter]`. Use `nostrGetPublicKey()` to retrieve the install's hex-encoded signing public key (for diagnostics); read it after MLS initialization, since that is when the persisted signing key is installed.

> **`nostrGetPublicKey()` is no longer a self-event filter.** Every sealed event is signed by a fresh single-use key, so comparing an inbound event's `pubkey` against this value never matches our own sealed traffic — and by design nothing on a gift wrap identifies its author. The bundled bridges keep the comparison only for the legacy unsealed form. Self-delivery is prevented by the `#p` filter (our outbound events are addressed to a *peer's* tag, not ours) and, for self-addressed messages, by the engine's deduplication.

The filter carries two independent bounds on how much history a (re)connect pulls down, and they do different jobs:

- **`since`** is the real bound. It is derived from a **persisted receive watermark** — the newest `created_at` this install has accepted — so a device that reconnects a hundred times only re-fetches what it has not already seen. The watermark advances only for frames that decode into a protocol message, is clamped so a future-dated event cannot push the window past real messages, and only ever moves forward. `since` sits an hour and five minutes *below* the mark (a jitter window plus a clock-skew margin), so events legitimately stamped in the recent past are not skipped; NIP-01's `since` is inclusive, so the boundary event is re-delivered.
- **`limit`** (500) caps what one initial query may return out of that window. It is advisory in both directions — `limit` is a SHOULD, and NIP-11 `max_limit` lets a relay clamp it silently.

With no watermark yet — a fresh install, a `wipePersistedState` logout, or a subscription built before protocol-state storage has been restored — `since` falls back to 24 hours ago. It is never zero or absent: that is the unbounded filter the watermark exists to remove.

Bridges that still call the timestamp-less `nostrMessageReceived(senderId, data)` keep working, but never advance the watermark, so every reconnect re-fetches a full backfill window.

**Two residuals worth knowing, neither of which loses messages:**

*The replayed overlap is not fully deduplicated.* Message-id dedup retains ids for an hour by default (`reliability.dedup.retention_time_secs`, and at most `max_tracked_messages` of them), while `since` reaches back an hour and five minutes. A reconnect sooner than the retention window has its overlap absorbed; a reconnect after longer — an app reopened the next day, the common case — re-processes it instead. That costs work, not correctness: a replayed ciphertext whose ratchet generation is spent fails closed and is dropped, a past-epoch one triggers at most one rate-limited re-key, and a replayed group copy TTLs out of the pending buffer.

*Junk can crowd out stored history.* The routing tag is `SHA-256(userId)`, so anyone who knows a username can publish events to it, and only the *decodability* of a frame gates the watermark — parsing a `Message` needs no signature. An attacker who floods more than `limit` decodable events can therefore both push real messages out of a truncated initial query and advance the mark past them, leaving those below the next `since`. What makes this recoverable rather than terminal is that it is not the only delivery path: ACK-gated messages stay in the sender's outbox for 7 days and are retransmitted with a fresh `created_at`, which lands above any watermark. The exposure is delay on a Nostr-only route, and it needs a sustained flood rather than a single event.

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

    fun onIncomingEvent(senderHex: String, payload: ByteArray, createdAt: Long) {
        protocol.nostrMessageReceivedAt(senderHex, payload.toList(), createdAt)
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

### What a relay can see

A sealed event carries exactly five things, and none of them names anybody:

| Field | Value | What it reveals |
|---|---|---|
| `kind` | `1059` | That this is a gift wrap — the same kind ordinary NIP-17 DM clients publish. (Published key packages use `30443`, the kind Marmot publishes them under, for the same anonymity-set reason) |
| `pubkey` | fresh single-use key | Nothing. A new key per event, so no two events we publish are linkable to each other or to this device |
| `tags` | `[["p", <recipient routing tag>]]` | An opaque 32-byte label. Computable *from* a `userId`, but not invertible back to one |
| `created_at` | jittered up to 1 h into the past | A coarse time bucket, deliberately not the publication time |
| `content` | NIP-44 v2 ciphertext | Length, rounded up to a power-of-two bucket |

**Before sealing, the same event published the entire protocol envelope in cleartext**, base64'd into `content`:

```json
{"id":"…","sender":"alice_real_username","recipient":"bob_real_username",
 "app_id":"fernweh","priority":"medium","ttl":8,"hop_count":0,
 "timestamp":1785919090277,"lamport_clock":0,"content_type":"text",
 "content":"__MLS_ENC__…","metadata":{…},"requires_ack":true}
```

Only `content` was MLS ciphertext. Both usernames, the app id, the app's metadata map, the content type and a millisecond timestamp were readable by every relay, permanently. Relays are not hops — they are third-party-operated, public, and archival — so this was a durable social-graph disclosure, not transient hop metadata. Sealing closes it.

**What remains observable** is traffic to `SHA-256(userId)` for a username an observer already guessed: that someone is publishing to that inbox, roughly when, and roughly how much. Sealing does not hide that, and rotating rendezvous tags — which would — is deliberately deferred: tying addressing to MLS epoch state turns a desync from "fails to decrypt" into "peers become mutually unreachable". (The Marmot protocol reached the same conclusion and likewise kept stable 1:1 inbox addressing.)

### Published key packages and cold first contact

Sealing a frame requires the recipient's Nostr public key, and until it is known the frame falls back to the recipient's **publicly computable** key — the same value as their routing tag. That fallback is bulk-collection resistance only: a relay scraping everything cannot read it, but anyone who guesses the username holds the matching private half.

To make the fallback rare rather than routine, each install **publishes MLS key packages as fetchable relay records**, and resolves a peer's before sealing to them.

- **Publishing.** `NOSTR_KEY_PACKAGE_SLOTS` (5) single-use key packages are published as NIP-33 addressable events (kind `30443`), one per slot, tagged `[["d", <slot id>], ["p", <our routing tag>]]` and signed by the install's real Nostr key. Slots refresh on the process tick: a package consumed by a Welcome, or expired, is replaced and republished under the same slot id.
- **Resolving.** A send to a peer whose per-install key we lack queues a query — `{"kinds":[30443],"#p":[<their routing tag>]}` — issued to every connected relay. *That send still goes out on the bootstrap leg*: blocking on a relay round-trip would turn a metadata upgrade into latency, and the round-trip may have nothing to return. The answer upgrades the next frame.

This is what makes **cold first contact** possible at all: a peer known only by username becomes reachable over Nostr with no prior key-package exchange over some other transport.

**Why slots, and not one record.** An MLS key package's init key is consumed by the first peer who uses it. One replaceable record would mean a stranger who fetches it after it was spent builds a Welcome that can never be processed. Consumption is *local* — an init key leaves provider storage only when this node processes a Welcome built against it — so a stranger can drive it only by actually establishing sessions, and each burnt slot is refilled on the next tick. The slot count covers the *sequential* gap between refreshes; it does not absorb concurrent cold contacts, since nothing distributes simultaneous fetchers across slots (two at once generally race for one init key, and the loser recovers through the reverse key-package exchange).

**Two failure modes, neither silent.** A refill that cannot proceed — an MLS or storage error — emits the `NOSTR_KEY_PACKAGE_SLOT_EXHAUSTED` security warning rather than leaving a stale record standing (once per refresh pass, suppressed for 5 minutes: these causes persist, and one event per slot per refresh would bury the signal in its own repetition). A record that was built but never reached a relay — rejected, timed out, or in flight when the connection dropped — is reported back by the transport, and the next tick republishes it under the same slot id with the same (still unconsumed) package. Without that report the slot would stay marked published for the life of the process while the relays held nothing.

The first such failure retries promptly, since a relay hiccup should not cost a window; *consecutive* failures back off, doubling from the refresh interval to a 30-minute ceiling, so a relay that rejects the kind outright converges instead of being retried once per slot per minute indefinitely. A slot quiet for longer than the ceiling starts its streak over.

**Publication outcomes never touch the transport's delivery metrics.** DORS scores reliability on `success / (success + failure)` over lifetime counters that never decay, and an idle install publishes far more than it sends — so counting publications would score the transport on something other than its ability to carry messages. A relay rejecting kind `30443` would otherwise drive the ratio toward zero and make DORS deprioritise Nostr for traffic that delivers perfectly well, while publications that succeed would equally mask real message failures.

#### Why the published record is sealed, though it is public by intent

An MLS key package carries its owner's username twice: in the `KeyPackagePayload` field, and — unremovably — in the leaf credential, since this SDK uses basic credentials holding the raw user id. Published in the clear, `{"kinds":[30443]}` with no tag and no author would return **a directory of every username on the relay**, handing over exactly the preimages the `SHA-256(userId)` routing tag exists to withhold.

The record's content is therefore NIP-44-sealed to our own publicly computable key. That costs nothing in reach — fetching the record requires the routing tag, which requires the username, which is the same knowledge needed to reconstruct the key and open it — while a scraper filtering by kind sees an opaque blob.

So the computable keypair keeps exactly one encryption use: a self-published record whose only reader already knows whose it is. Real messages seal to the per-install key resolved from it. It must still **never** back NIP-42 AUTH or any authentication decision — its private half is public by construction, and a fetcher takes the peer's Nostr key from the Ed25519-signed payload inside the record, never from the event's self-attesting `pubkey` field.

#### What publication costs

This is the first thing the transport emits **unprompted**. A small set of records sits at this install's routing tag and is refreshed as slots are consumed, whether or not a message is ever sent. Their content is sealed, but *the existence of a record at a given tag, and the timing of its refreshes*, are visible to every relay published to — a liveness signal the transport otherwise does not emit.

`transports.nostr.coldContactEnabled` (RN) / `nostr_cold_contact_enabled` (UniFFI, core `TransportConfig`), default on, turns both halves off and keeps the transport silent until it has traffic. The price is that cold contact stops working: peers become reachable over Nostr only after a key-package exchange over some other transport.

#### Residual: a squatter can replay a spent record

The routing tag is public, so anyone may publish to it and a query returns whatever the relay holds there. Two things that buys, both bounded:

- **Crowding.** Foreign records displace real ones from the query's `limit`, costing the metadata upgrade and nothing else — the send falls back to the bootstrap leg exactly as before.
- **Replaying a spent record.** Every published record is openable by anyone who knows the username (that is the design), so a squatter can unseal one of a peer's *consumed* records, re-seal the untouched and genuinely Ed25519-signed payload under their own author key, and stand it back up with a fresh `created_at`. Nothing detects this: the inner signature is real, and no freshness binding ties a record to the live slot. The resolver imports a genuine-but-consumed key package and builds a Welcome the peer can never process — worse than crowding, because it commits to a dead session rather than staying on the working bootstrap leg.

  It does not strand the pair. Importing any key package pushes ours back under `auto_key_exchange`, and the peer then establishes from their side against a package that is actually live, so the cost is delivery delayed by one exchange — the same bounded class as the already-accepted `key_package_data` substitution vector. Closing it outright needs the record to carry slot-bound freshness (its slot id plus a signed issue time), which is future work.

#### Residual: a cached key can still go stale

If a peer wipes their storage, their per-install Nostr key rotates, and frames sealed to the cached key are readable by nobody — the transport has no delivery feedback that would reveal it, since an unsealable frame is indistinguishable from one addressed to someone else.

Resolution narrows this considerably: a peer who publishes is re-resolved whenever we hold no key for them, including after the peer-key map's reset-at-capacity. It does not close it entirely, because a *cached* key is not re-resolved — only a missing one is. It is also narrow to begin with: a storage wipe destroys the peer's MLS session too, so the conversation needs rebuilding regardless, and any contact over mesh, Internet, or a peer-initiated Nostr message heals it. The unblock clean slate clears the cached key explicitly, reverting to the bootstrap key rather than a dead one.

### Identity

- **The signing key is a per-install secret** — derived via HKDF from a random 32-byte secret persisted through the app's `MlsStorage` on first `initialize_mls`, never from any public identifier. It is advertised in outgoing key packages so peers can seal to it, and it is *not* what signs sealed events (those use a throwaway key each). Wiping app storage rotates the install's Nostr identity.
- **The routing tag is derived from `userId`** — running the same `userId` on two devices means they share an inbox (both receive events tagged to that ID). Use distinct user IDs per device for separate inboxes.
- **Payload is end-to-end encrypted by MLS** before reaching this transport. Gift-wrap sealing is an additional, hop-local layer over the whole envelope; it does not replace MLS.
- **Telemetry scrubbing** — when telemetry `scrub_ids` is on (default), pubkeys flowing through the SDK's telemetry sink are SHA-256 hashed before emission.

### Interoperability and the kill switch

**Compatibility with pre-sealing builds is asymmetric.** Inbound is fully compatible: the receive path always accepts both forms — gift wraps and the legacy unsealed kind-4 event — and the subscription requests both kinds permanently, so a peer on an older build can still reach us.

Outbound to such a peer, however, does **not** work while sealing is on. A pre-sealing build's REQ filter is `{"kinds":[4]}`, so a relay never delivers a kind-1059 event to it, and it carries no NIP-44 layer to unseal one with. The failure is visible rather than silent — no ACK returns, so the send fails through the ordinary retry ladder and DORS demotes Nostr for that peer — but the message does not arrive. Reaching a not-yet-upgraded peer over Nostr requires turning sealing off on the sender until they upgrade.

Sealing is therefore safe to enable or disable on one device without coordinating a fleet **of sealed-capable builds** — unlike the negotiated wire/envelope switches, it needs no peer capability, and no state becomes unreadable either way.

`transports.nostr.sealingEnabled` (RN) / `nostr_sealing_enabled` (UniFFI, core `TransportConfig`) turns sealing off, restoring the cleartext kind-4 form above. Set it only for a relay that rejects kind 1059, or to reach a peer on a pre-sealing build.

**Sealing costs size.** NIP-44 pads to a power-of-two bucket, so a payload just past a boundary nearly doubles before the MAC and base64 are applied — considerably more than base64's ~33% alone. The 64 KiB event cap is measured on the final sealed event, so a message that fits unsealed may not fit sealed.

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
