# Service Discovery & Request/Response

Nodes can register services they offer, discover services across the mesh via gossip-propagated queries, and perform typed request/response interactions with providers — all offline, with no central registry.

## Overview

Service discovery turns an Offline Protocol mesh into a decentralized service marketplace. Any node can advertise capabilities (e.g. "I serve weather data" or "I have a first-aid knowledge base"), and any other node can find and invoke those services — even if the provider is multiple hops away.

The system has three phases:

1. **Registration** — a node declares what services it offers locally.
2. **Discovery** — a node broadcasts a query that gossips through the mesh; providers respond directly to the originator.
3. **Request/Response** — the consumer sends a typed request to a chosen provider and receives a response.

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
| `service_id` | `ServiceId` (non-empty string) | Unique identifier, e.g. `"weather.v1"`, `"wiki.first-aid"` |
| `version` | `String` | Semantic version of the service, e.g. `"2.0"` |
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

1. The originator sends the query to all its known peers.
2. Each receiving node checks its local service registry for matches and responds directly to the originator.
3. Each receiving node forwards the query to all its other known peers (excluding the sender and originator).
4. A **deduplication window** (60 seconds) prevents query storms — each node tracks query IDs it has already processed.
5. A **max-hops limit** (default 10) prevents unbounded propagation — each forward decrements the remaining hop counter.

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

Service discovery broadcasts to **all known peers**, not just those with established MLS encryption sessions. Peers are tracked independently of encryption state — any peer discovered via `on_neighbor_found()` is eligible for service discovery messages. This means service discovery works even when encryption is disabled or before key exchange completes.

### Encryption Interaction

Service discovery, requests, and responses use `send_internal_message()` and are sent as **plaintext control messages**. If `require_encryption` is set to `true` in the protocol config, all service APIs will return an error:

```
"discover_services sends plaintext control messages; disable require_encryption for bootstrap flows"
```

To use service discovery, either:
- Leave `require_encryption` as `false` (default), or
- Set it to `false` during the bootstrap/discovery phase, then enable it later.

Note: If MLS encryption **is** established with a peer but `require_encryption` is false, service messages still benefit from the encryption layer automatically through `send_internal_message`.

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

`ServiceId::new()` validates the ID is non-empty and returns `Err(Error::InvalidServiceId)` if it is.

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

Common status values: `"ok"`, `"error"`, `"not_found"`. The status field is application-defined — use whatever values make sense for your service protocol.

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

```json
{
  "type": "service_request_received",
  "request_id": "7c9e6679-7425-40de-944b-e07fc1f90ae7",
  "service_id": "weather.v1",
  "method": "get_forecast",
  "body": "{\"city\": \"NYC\"}",
  "sender": "alice"
}
```

### ServiceResponseReceived

Emitted on the **consumer** node when a provider responds to a request.

| Field | Type | Description |
|-------|------|-------------|
| `request_id` | `String` | Matches the request ID from `send_service_request()` |
| `service_id` | `String` | The service that responded |
| `status` | `String` | Application-defined status (`"ok"`, `"error"`, `"not_found"`, etc.) |
| `body` | `String` | Response payload |
| `provider_peer_id` | `String` | Peer ID of the provider |

```json
{
  "type": "service_response_received",
  "request_id": "7c9e6679-7425-40de-944b-e07fc1f90ae7",
  "service_id": "weather.v1",
  "status": "ok",
  "body": "{\"temp\": 72, \"unit\": \"F\"}",
  "provider_peer_id": "bob"
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
| `__SVC_DISC_R__` | Discovery response | Direct to originator |
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
| Max hops | 10 | Maximum gossip forwarding depth for discovery queries |
| ServiceId | Non-empty string | Validated on construction; empty strings are rejected |

These values are compile-time constants. The dedup map is automatically cleaned up during the protocol's periodic `cleanup_expired_entries()` cycle.

## Architecture Integration

Service discovery is built on top of existing protocol infrastructure:

- **Transport**: All service messages route through the DORS multi-transport selector (BLE, WiFi Direct, Internet, Reticulum). The best available transport is chosen automatically.
- **Reliability**: ACK/retry mechanisms from the reliability layer are automatically applied to request/response messages.
- **Encryption**: When MLS sessions exist, service messages are encrypted transparently. When they don't, messages are sent in plaintext (as long as `require_encryption` is false).
- **Routing**: Discovery queries use the existing gossip and message forwarding infrastructure with the same deduplication and hop-counting as other mesh messages.
- **Events**: Integrated with the existing `EventCallback` system — no separate event subscription needed.

## Example: MeshWiki

The `examples/mesh-wiki/` directory contains a full React Native app demonstrating service discovery in a real-world scenario: a decentralized offline knowledge base.

Each node offers **knowledge packs** as services (First Aid, Cooking, DIY Repair, Survival). Other nodes discover available knowledge packs on the mesh, browse topics, and request specific Q&A entries — all without any central server or internet connection.

Key patterns demonstrated:
- Registering multiple services with rich capabilities metadata
- Handling discovery responses and building a local catalog
- Sending structured JSON requests and parsing responses
- Real-time activity logging for debugging
