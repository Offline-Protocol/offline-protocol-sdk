# Service Discovery & Request/Response

Nodes can register services they offer, discover services across the mesh via gossip-propagated queries, and perform typed request/response interactions with providers — all offline, with no central registry.

## Overview

Service discovery turns an Offline Protocol mesh into a decentralized service marketplace. Any node can advertise capabilities (e.g. "I serve weather data" or "I have a first-aid knowledge base"), and any other node can find and invoke those services — even if the provider is multiple hops away.

The system has three phases:

1. **Registration**: a node declares what services it offers locally.
2. **Discovery**: a node broadcasts a query that gossips through the mesh; a provider answers the peer it heard the query from, and each hop forwards the answer toward the originator.
3. **Request/Response**: the consumer sends a typed request to a chosen provider and receives a response.

On a LAN there is a fourth path beside the mesh: a node can publish its registrations as DNS-SD instances and read its neighbours', under the [DNS-SD mapping](spec/dns-sd-mapping.md). A LAN import is unsigned and arrives as a `service_discovered` event with `source: "lan"`; the Python binding ships the bridge (`dnssd_bridge.py`).

```
Node A (consumer)                    Mesh                    Node B (provider)
      |                                |                           |
      |  register_service()            |                           |  register_service()
      |                                |                           |
      |-- discover_services() -------->|-- (gossip forward) ----->|
      |                                |                           |  (matches local registry)
      |                                |<-- ServiceDiscovered ----|
      |<-- ServiceDiscovered ----------|                           |
      |                                |                           |
      |-- send_service_request() ----->|------------------------->|
      |                                |           ServiceRequestReceived
      |                                |                           |  (app handles request)
      |                                |<-- respond_to_service_ --|
      |<-- ServiceResponseReceived ----|     request()             |
```

## Core Concepts

### ServiceDescriptor

Every registered service is described by a `ServiceDescriptor`:

| Field | Type | Description |
|-------|------|-------------|
| `service_id` | `ServiceId` | Unique identifier, e.g. `"weather.v1"`, `"wiki.first-aid"`. Validated on construction: not empty, not whitespace only, at most 256 bytes, and not starting with `__`, which is reserved for control-message prefixes |
| `version` | `String` | An opaque label the provider chooses, e.g. `"2.0"`. Nothing parses or compares it; a consumer that wants an ordering defines its own |
| `capabilities` | `HashMap<String, String>` | Key-value metadata advertising features, formats, limits, etc. |

The `capabilities` map lets providers advertise what they support **before** any request is made. Consumers inspect these in `ServiceDiscovered` events to pick the right provider. Example capabilities:

```json
{
  "format": "json",
  "max_payload_kb": "512",
  "topics": "first-aid,cooking,survival",
  "language": "en"
}
```

### Gossip-Based Discovery

Discovery queries propagate through the mesh using **gossip flooding**:

1. The originator sends the query to at most 20 of its known peers.
2. Each receiving node checks its local service registry for matches and answers the peer it heard the query from, never the `originator` the payload names: a spoofed originator field would otherwise make every provider leak its service list to an arbitrary peer. A node that receives an answer it did not originate forwards it as a unicast toward the originator and emits no event; only the originator emits `ServiceDiscovered`.
3. Each receiving node forwards the query to at most 5 of its other known peers (excluding the sender and originator), chosen deterministically from the query id.
4. A **deduplication window** (60 seconds, at most 10,000 remembered ids) prevents query storms: each node tracks query IDs it has already processed.
5. A **max-hops limit** (default 10) prevents unbounded propagation: each forward decrements the remaining hop counter.

```
    A ──── B ──── D
    │      │
    └──── C ──── E

A calls discover_services():
  → sends query to B, C
  B checks local services, responds if match, forwards to C, D
  C checks local services, responds if match, forwards to B, E
  C receives forwarded query from B — dedup drops it (already seen)
  B receives forwarded query from C — dedup drops it (already seen)
  D checks local services, responds if match (no more peers to forward to)
  E checks local services, responds if match (no more peers to forward to)
```

Discovery responses include a `hop_count` field derived from the message's actual hop count, so consumers can reason about **proximity** to the provider. A hop count of 0 means the provider is a direct neighbor.

### Peer Tracking

Service discovery broadcasts to **known peers**, not just those with established MLS encryption sessions. Peers are tracked independently of encryption state: any peer the engine learns of through `on_neighbor_discovered()` or as the sender of a message is eligible for service discovery messages, until it has not been seen for the known-peer lifetime. This means service discovery works even when encryption is disabled or before key exchange completes.

### Encryption Interaction

Service frames are internal control messages, not user content, so they are **exempt from `require_encryption`**: service APIs work unchanged under the default fail-closed encryption policy, just like key packages, Welcome messages and connection requests. What is encrypted depends on the frame.

- **Discovery** (`discover_services` and its answers) is always **signed plaintext** (Ed25519 control-message signing, not MLS). Every hop reads it to match the service and forward the query, and there is no single recipient whose session could seal it. Do not put anything sensitive in a service descriptor.
- **Requests and responses** are **sealed inside the recipient's 1:1 MLS session** when `encryption.encryptServiceMessages` is on (the default), the recipient runs a release that advertises support, and the session with the recipient is confirmed. The provider's `service_request_received.sender` is then the MLS-authenticated requester.
- Otherwise a request or response leaves as **signed plaintext**. That covers a peer on an older release, the switch off on either side, and any request sent before the session confirms, which includes **the first request to a provider this device has never met**. The attempt to seal that first request is what starts the session, so the next one is sealed.
- Once a peer has sent this device a sealed request or response, its bodies stay sealed even if a later (or replayed) key package stops advertising support. That proof is durable.
- If sealing fails on a confirmed session, `send_service_request` and `respond_to_service_request` return the error. The body is never sent in plaintext instead.

Each `service_request_received` and `service_response_received` event carries `encrypted`, so the receiving app can tell which form arrived and refuse a plaintext one if it must.

If a body must never travel in plaintext, make sure a session exists first (any exchanged direct message does it), or send it as an ordinary direct message instead of through the service API, as the HTTP front does. Threat model [R9](security/threat-model.md#r9-service-bodies-are-sealed-only-toward-capable-peers-discovery-is-signed-not-encrypted) records what the fallback leaves exposed.

## Rust API

### Registering Services

```rust
use offline_protocol_core::service::{ServiceDescriptor, ServiceId};

// Register a service this node offers
protocol.register_service(ServiceDescriptor {
    service_id: ServiceId::new("weather.v1")?,
    version: "2.0".to_string(),
    capabilities: HashMap::from([
        ("format".into(), "json".into()),
        ("coverage".into(), "us,eu".into()),
    ]),
})?;

// Unregister — returns true if found and removed, false if not found
let was_registered: bool = protocol.unregister_service("weather.v1")?;
```

`ServiceId::new()` returns `Err(Error::InvalidServiceId)` for an empty or whitespace-only id, an id over 256 bytes, or one starting with `__` (the control-message prefix family). Registering an id that is already registered replaces its descriptor.

### Discovering Services

```rust
// Discover ALL services on the mesh
let query_id: String = protocol.discover_services(None)?;

// Discover a specific service by ID
let query_id: String = protocol.discover_services(Some("weather.v1"))?;
```

This broadcasts the query to all known peers. Responses arrive **asynchronously** as `ServiceDiscovered` events — there is no synchronous return of results.

The returned `query_id` (UUID) can be used to correlate responses back to a specific discovery query. Multiple providers may respond to the same query.

### Sending Service Requests

```rust
// Send a typed request to a specific provider (discovered earlier)
let request_id: String = protocol.send_service_request(
    "provider_peer_id",  // from ServiceDiscovered event
    "weather.v1",        // service ID
    "get_forecast",      // method name (application-defined)
    r#"{"city": "NYC"}"#, // request body (application-defined, typically JSON)
)?;
```

The `request_id` (UUID) correlates the eventual `ServiceResponseReceived` event.

Service requests are sent with **High** message priority (vs Medium for discovery), ensuring they are prioritized in the transport layer.

### Responding to Service Requests

```rust
// After receiving a ServiceRequestReceived event, the provider responds:
let message_id: MessageId = protocol.respond_to_service_request(
    "request_id",       // from the ServiceRequestReceived event
    "requester_peer_id", // from the event's `sender` field
    "weather.v1",        // service ID
    "ok",                // status (application-defined)
    r#"{"temp": 72, "unit": "F"}"#, // response body
)?;
```

The status is one of exactly three values: `"ok"`, `"not_found"` or `"error"`. The engine refuses any other status on send (`ServiceError::InvalidStatus`) and a requester drops a response carrying any other status on receipt, so a status of your own is a response nobody receives. Put application-level outcomes in the body.

## Events

All three service events are delivered through the standard `EventCallback` / `poll_event` system.

### ServiceDiscovered

Emitted when a provider responds to a discovery query you sent.

| Field | Type | Description |
|-------|------|-------------|
| `query_id` | `String` | The query ID returned by `discover_services()` |
| `service_id` | `String` | The service identifier |
| `version` | `String` | The service version |
| `provider_peer_id` | `String` | Peer ID of the node offering this service |
| `capabilities` | `HashMap<String, String>` | Service capabilities metadata |
| `hop_count` | `u8` | Number of hops from the provider (0 = direct neighbor) |

```json
{
  "type": "service_discovered",
  "query_id": "550e8400-e29b-41d4-a716-446655440000",
  "service_id": "weather.v1",
  "version": "2.0",
  "provider_peer_id": "bob",
  "capabilities": { "format": "json", "coverage": "us,eu" },
  "hop_count": 1
}
```

You may receive multiple `ServiceDiscovered` events for the same `query_id` — one per matching service per provider node. Use `hop_count` to prefer closer providers.

### ServiceRequestReceived

Emitted on the **provider** node when a consumer sends a request to a locally registered service. The app must handle this and call `respond_to_service_request`.

| Field | Type | Description |
|-------|------|-------------|
| `request_id` | `String` | Unique request identifier (use in response) |
| `service_id` | `String` | Which service is being invoked |
| `method` | `String` | Application-defined method name or action |
| `body` | `String` | Request payload (typically JSON) |
| `sender` | `String` | Peer ID of the requester (use as `requester` in response) |
| `encrypted` | `bool` | Whether the request arrived sealed in the 1:1 MLS session. `false` means signed plaintext that every hop could read; see [Encryption Interaction](#encryption-interaction) |

```json
{
  "type": "service_request_received",
  "request_id": "7c9e6679-7425-40de-944b-e07fc1f90ae7",
  "service_id": "weather.v1",
  "method": "get_forecast",
  "body": "{\"city\": \"NYC\"}",
  "sender": "alice",
  "encrypted": true
}
```

### ServiceResponseReceived

Emitted on the **consumer** node when a provider responds to a request.

| Field | Type | Description |
|-------|------|-------------|
| `request_id` | `String` | Matches the request ID from `send_service_request()` |
| `service_id` | `String` | The service that responded |
| `status` | `String` | One of `"ok"`, `"not_found"`, `"error"`; a response with any other status is dropped before this event |
| `body` | `String` | Response payload |
| `provider_peer_id` | `String` | Peer ID of the provider |
| `encrypted` | `bool` | Whether the response arrived sealed in the 1:1 MLS session |

```json
{
  "type": "service_response_received",
  "request_id": "7c9e6679-7425-40de-944b-e07fc1f90ae7",
  "service_id": "weather.v1",
  "status": "ok",
  "body": "{\"temp\": 72, \"unit\": \"F\"}",
  "provider_peer_id": "bob",
  "encrypted": true
}
```

### Auto Not-Found Response

If a service request arrives at a node that does **not** have the requested service registered, the protocol automatically responds with `status: "not_found"` and an empty body. No `ServiceRequestReceived` event is emitted to the provider's app — this is handled entirely at the protocol level.

## Mobile / React Native API

All methods are exposed via UniFFI to Swift, Kotlin, and React Native.

### TypeScript API

```typescript
// Register a service
await protocol.registerService('weather.v1', '2.0', {
  format: 'json',
  coverage: 'us,eu',
});

// Unregister
const wasRegistered: boolean = await protocol.unregisterService('weather.v1');

// Discover services (pass undefined/null to discover all)
const queryId: string = await protocol.discoverServices('weather.v1');
const queryIdAll: string = await protocol.discoverServices(); // discover all

// Send a request to a provider
const requestId: string = await protocol.sendServiceRequest(
  providerPeerId,
  'weather.v1',
  'get_forecast',
  JSON.stringify({ city: 'NYC' })
);

// Respond to a request (in your event handler)
await protocol.respondToServiceRequest(
  requestId,
  requesterPeerId,
  'weather.v1',
  'ok',
  JSON.stringify({ temp: 72, unit: 'F' })
);
```

### Full Event Handling Example

```typescript
protocol.onEvent((event) => {
  switch (event.type) {
    case 'service_discovered':
      // A provider responded to our discovery query
      console.log(`Found ${event.service_id} v${event.version} at ${event.provider_peer_id} (${event.hop_count} hops)`);
      console.log('Capabilities:', event.capabilities);

      // Optionally send a request to this provider
      protocol.sendServiceRequest(
        event.provider_peer_id,
        event.service_id,
        'get_data',
        JSON.stringify({ query: 'example' })
      );
      break;

    case 'service_request_received':
      // Another node is requesting our service — handle and respond
      const result = handleRequest(event.service_id, event.method, event.body);
      protocol.respondToServiceRequest(
        event.request_id,
        event.sender,
        event.service_id,
        result.status,
        JSON.stringify(result.data)
      );
      break;

    case 'service_response_received':
      // A provider responded to our request
      if (event.status === 'ok') {
        const data = JSON.parse(event.body);
        console.log('Got response:', data);
      } else if (event.status === 'not_found') {
        console.log('Service not available on that provider');
      } else {
        console.log('Error:', event.body);
      }
      break;
  }
});
```

Events are delivered as JSON strings via the existing `EventCallback` / `poll_event` system. Parse the JSON and switch on `"type"`.

## Wire Protocol Details

Service messages use internal control-message prefixes to distinguish them from user messages. These are consumed by the protocol and never surfaced as regular messages.

| Prefix | Message Type | Direction |
|--------|-------------|-----------|
| `__SVC_DISC_Q__` | Discovery query | Broadcast + gossip forwarded |
| `__SVC_DISC_R__` | Discovery response | To the peer the query came from; each hop forwards it toward the originator |
| `__SVC_REQ__` | Service request | Direct to provider |
| `__SVC_RESP__` | Service response | Direct to requester |

### Message Priority

| Message Type | Priority | Rationale |
|-------------|----------|-----------|
| Discovery query | Medium | Background discovery, not time-critical |
| Discovery response | Medium | Background discovery |
| Service request | **High** | Active user-initiated interaction |
| Service response | **High** | Active user-initiated interaction |

## Configuration & Limits

| Parameter | Value | Description |
|-----------|-------|-------------|
| Dedup TTL | 60 seconds | How long a query ID is remembered to prevent re-processing |
| Dedup entries | 10,000 | Remembered query ids; the oldest is evicted first |
| Max hops | 10 | Maximum gossip forwarding depth for discovery queries |
| Initial broadcast | 20 peers | How many known peers the originator sends a query to |
| Gossip fanout | 5 peers | How many known peers each hop forwards a query to, chosen deterministically from the query id |
| Service payload | 128 KiB | A control frame over this is dropped unparsed |
| Request or response body | 64 KiB | Refused on send, dropped on receipt |
| Method name | 256 bytes | Refused on send, dropped on receipt |
| Response status | `ok`, `not_found`, `error` | Refused on send, dropped on receipt |
| ServiceId | 1 to 256 bytes | Not whitespace only, not starting with `__` |

These values are compile-time constants in `crates/offline-protocol-services/src/payloads.rs`. The dedup map is swept by the engine's periodic cleanup.

## Architecture Integration

Service discovery is built on top of existing protocol infrastructure:

- **Transport**: All service messages route through the DORS multi-transport selector (BLE, WiFi Direct, Internet, Reticulum). The best available transport is chosen automatically.
- **Reliability**: ACK/retry mechanisms from the reliability layer are automatically applied to request/response messages.
- **Encryption**: Discovery is signed plaintext. Requests and responses are sealed in the 1:1 MLS session toward peers that advertise support once the session is confirmed, and signed plaintext otherwise; see [Encryption Interaction](#encryption-interaction). All are exempt from `require_encryption`.
- **Routing**: Discovery queries use the existing gossip and message forwarding infrastructure with the same deduplication and hop-counting as other mesh messages.
- **Events**: Integrated with the existing `EventCallback` system — no separate event subscription needed.

## Subscriptions

A request gets one answer. A **subscription** gets every change as it happens:
a consumer holds a lease on one topic of a provider's service, and the
provider sends it updates until the lease ends. The decision record is
[ADR 0035](adr/0035-a-service-can-be-subscribed-to.md) and the runtime
behaviour is the [service subscriptions](state-machines/service-subscriptions.md)
state machine.

**Every subscription frame is sealed** in the 1:1 MLS session between the two
devices, with no plaintext fallback: a subscription made before a session
exists waits for it, and starts it. This is unlike requests and responses
(see [Encryption Interaction](#encryption-interaction)).

### Topics and modes

A topic is a name under a service, 1 to 128 bytes of `A-Z a-z 0-9 . _ - /`.
Its mode is fixed by its first publish. Only the application creates a
topic: a subscription to one not yet published waits for it, and ends with
`mode_mismatch` if the first publish is in the other mode.

| Mode | For | Publish | A consumer that was away gets |
|------|-----|---------|------------------------------|
| `latest` | State: a battery level, a position, who is leading | With a key. Only the newest value per key matters | The current value of every key |
| `log` | Events: a door opened, an alarm fired | Without a key. Every update matters, and each carries its `seq` | What it missed from the last 256 updates, or the checkpoint and what follows it |

### Consumer

```rust
use offline_protocol::{SubscriptionMode};

let sub_id = protocol.subscribe_service(&provider, "fleet.v1", "battery", SubscriptionMode::Latest, "")?;
// ... service_subscription_changed { state: "active", reason: "accepted" }
// ... service_update_received { key: Some("pct"), body: "81", snapshot: true }
protocol.resync_service_subscription(&sub_id)?;   // the whole state again, now
protocol.unsubscribe_service(&sub_id)?;
```

One subscription per (provider, service, topic): subscribing again to a
topic this device already subscribes to fails with `InvalidArgument`, naming
the existing `sub_id`.

A subscription can be narrowed, and the provider applies both narrowings, so
what was not asked for never leaves its device:

```rust
use offline_protocol::SubscribeOptions;

// Every room, including rooms first published after this call; each update
// names its topic ("rooms/kitchen"), and each topic is recovered on its own.
protocol.subscribe_service(&provider, "home.v1", "rooms/*", SubscriptionMode::Latest, "")?;

// Only these keys of a `latest` topic: exact, or a prefix ending in `*`.
protocol.subscribe_service_with(&provider, "fleet.v1", "car", SubscribeOptions {
    keys: vec!["battery".into(), "tyres.*".into()],
    ..SubscribeOptions::new(SubscriptionMode::Latest)
})?;
```

A pattern is a prefix and one final `*` (`*` alone is every topic), and
follows at most 64 topics. A key filter applies to `latest` only: a `log`
consumer tracks a contiguous position that a withheld update would leave a
permanent gap in.

### Provider

```rust
use offline_protocol::{SubscriptionPolicy, RulesFallback};

protocol.register_service(descriptor)?;
protocol.set_service_subscription_policy("fleet.v1", SubscriptionPolicy::Rules {
    allow_peers: vec![],
    allow_groups: vec![fleet_group_id],
    fallback: RulesFallback::Ask,
})?;
let report = protocol.publish_service_update("fleet.v1", "battery", Some("pct"), "81")?;
protocol.publish_service_update("fleet.v1", "doors", None, r#"{"door":"front","open":true}"#)?;
protocol.set_service_topic_checkpoint("fleet.v1", "doors", r#"{"front":true,"back":false}"#)?;
```

Who may subscribe is the service's **policy**:

| Policy | A new subscriber is |
|--------|--------------------|
| `Restricted` (the default) | Held, and the application is asked with `service_subscribe_received`; it calls `accept_service_subscription` or `reject_service_subscription`. Unanswered for 60 seconds, it is rejected |
| `AcceptAll` | Admitted. Any peer with a session can watch the service |
| `Rules` | Admitted when listed in `allow_peers` or a member of a group in `allow_groups` (in this device's roster of that group); anyone else is asked (`Ask`) or refused (`Reject`) |

`renewal` on `service_subscribe_received` means the subscriber says it held
the subscription before this device restarted or its lease lapsed. It is the
subscriber's claim, which any peer can make: use it to word a prompt, never
to admit without one. A subscriber to be admitted without asking belongs in
`allow_peers`. Under `AcceptAll` the first to arrive fill the caps, and hold
them for as long as they renew; a service that must stay reachable to
particular peers uses `Rules`.

`publish_service_update` returns how each subscriber was reached:
`sent_live` (once, over a live direct link), `queued` (on the reliable
ladder, across hops; a `latest` value replaces the one queued before it for
its key), `coalesced` (held back by the 50 ms interval; the newest value
follows) and `failed` (no confirmed session; the next renewal catches it up).
`end_service_subscription` ends one subscription, `clear_service_topic`
starts a topic over, and `service_subscribers` lists who is subscribed.
Blocking a peer (`block_user`) ends every subscription with it at once, on
both sides, and nothing is sent to it.

### Getting the state back

A consumer gets the state back without asking: each renewal (every minute by
default) carries what it holds, and the provider sends what is missing. A
provider that restarted or cleared the topic is noticed (`stream_reset`), and
the new state follows. `resync_service_subscription` asks for it at once. A
`log` consumer whose position has fallen out of the buffer gets the
checkpoint and what follows it; with no checkpoint it is told
`history_lost`, so set one whenever the state is worth summarising.

A `log` topic is delivered in arrival order, not `seq` order: an update that
arrives ahead of one still missing is delivered at once, and the gap is
filled when the missing one arrives or at the next renewal. An application
that needs order sorts by `seq`.

Nothing of a subscription survives a restart on either side, its frames
waiting to be sent included: they are never written to the outbox's store.
A consumer that restarts holds no subscriptions and subscribes again. A
provider that restarts holds no subscribers and no topics, and every stream
is new; an application whose `latest` state matters republishes it at
start-up. A frame that had already left, to a relay mailbox or a custodian,
still arrives, and is dropped: each side remembers the streams and
subscriptions it moved away from.

### Events

| Event | Side | Fields |
|-------|------|--------|
| `service_subscribe_received` | provider | `sub_id`, `service_id`, `topic`, `mode`, `params`, `sender`, `renewal` |
| `service_subscription_changed` | both | `sub_id`, `role`, `service_id`, `topic`, `peer`, `state`, `reason` |
| `service_update_received` | consumer | `sub_id`, `service_id`, `topic`, `key`, `seq`, `body`, `snapshot`, `provider_peer_id` |

`state` is `pending`, `active`, `paused` or `ended`. `reason` is one of
`accepted`, `resumed`, `stream_reset`, `history_lost`, `renewal_unanswered`,
`provider_unreachable`, `no_reply`, `session_timeout`, `unsubscribed`,
`revoked`, `replaced`, `expired`, `approval_timeout`, `service_unregistered`,
`policy`, `invalid_subscription`, `rejected`, `not_found`, `mode_mismatch`,
`full`, `pattern_full` (both sides: a pattern subscription follows 64 topics
and will follow no more, reported once), or a reason the provider's
application gave.

### Limits

| Limit | Value |
|-------|-------|
| Update or checkpoint body | 64 KiB |
| Key | 1 to 256 bytes, no zero byte |
| `params` | 4 KiB |
| Subscriptions a provider holds, admitted or awaiting the application | 256, and 16 from one peer; refused, never evicted, at either |
| Subscriptions a consumer holds | 64 |
| Queued frames | Share the outbox with messages: a `latest` value waits as one entry per subscriber and key, a `log` update as one entry each |
| `latest` topic | 256 keys, 256 KiB |
| `log` buffer | 256 updates or 512 KiB |
| Lease | 30 s to 600 s, default 120 s (`service_subscriptions.lease_ms`) |

Subscriptions are switched by `ProtocolConfig::service_subscriptions` (on by
default) and need MLS. The bindings do not carry that section yet, so a device
on a binding runs the defaults.

## Example: MeshWiki

The `examples/mesh-wiki/` directory contains a full React Native app demonstrating service discovery in a real-world scenario: a decentralized offline knowledge base.

Each node offers **knowledge packs** as services (First Aid, Cooking, DIY Repair, Survival). Other nodes discover available knowledge packs on the mesh, browse topics, and request specific Q&A entries — all without any central server or internet connection.

Key patterns demonstrated:
- Registering multiple services with rich capabilities metadata
- Handling discovery responses and building a local catalog
- Sending structured JSON requests and parsing responses
- Real-time activity logging for debugging
