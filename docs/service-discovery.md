# Service Discovery & Request/Response

Nodes can register services they offer, discover services across the mesh via gossip-propagated queries, and perform typed request/response interactions with providers — all offline, with no central registry.

## How It Works

```
Node A (consumer)                    Mesh                    Node B (provider)
      |                                |                           |
      |-- discover_services() -------->|-- (gossip forward) ----->|
      |                                |                           |
      |                                |<-- ServiceDiscovered ----|
      |<-- ServiceDiscovered ----------|                           |
      |                                |                           |
      |-- send_service_request() ----->|------------------------->|
      |                                |           ServiceRequestReceived
      |                                |                           |
      |                                |<-- respond_to_service_ --|
      |<-- ServiceResponseReceived ----|     request()             |
```

1. A consumer broadcasts a discovery query. It gossips through the mesh with dedup.
2. Any node with a matching registered service responds directly to the originator.
3. The consumer picks a provider and sends a typed request.
4. The provider's app handles the request and responds.

Discovery queries include a `hop_count` so consumers can reason about proximity.

## API

### Registering Services

```rust
// Register a service this node offers
protocol.register_service(ServiceDescriptor {
    service_id: ServiceId::new("weather.v1")?,
    version: "2.0".to_string(),
    capabilities: HashMap::from([("format".into(), "json".into())]),
})?;

// Remove it later
protocol.unregister_service("weather.v1")?;
```

### Discovering Services

```rust
// Discover all services on the mesh
let query_id = protocol.discover_services(None)?;

// Or filter by service ID
let query_id = protocol.discover_services(Some("weather.v1"))?;
```

Responses arrive asynchronously as `ServiceDiscovered` events.

### Request/Response

```rust
// Send a request to a specific provider
let request_id = protocol.send_service_request(
    "provider_peer_id",
    "weather.v1",
    "get_forecast",
    r#"{"city": "NYC"}"#,
)?;

// Provider responds after receiving ServiceRequestReceived event
protocol.respond_to_service_request(
    "request_id",
    "requester_peer_id",
    "weather.v1",
    "ok",
    r#"{"temp": 72}"#,
)?;
```

## Events

### ServiceDiscovered

Emitted when a provider responds to a discovery query.

```json
{
  "type": "service_discovered",
  "query_id": "uuid",
  "service_id": "weather.v1",
  "version": "2.0",
  "provider_peer_id": "bob",
  "capabilities": { "format": "json" },
  "hop_count": 1
}
```

### ServiceRequestReceived

Emitted when another node sends a request to a locally registered service. The app should handle this and call `respond_to_service_request`.

```json
{
  "type": "service_request_received",
  "request_id": "uuid",
  "service_id": "weather.v1",
  "method": "get_forecast",
  "body": "{\"city\": \"NYC\"}",
  "sender": "alice"
}
```

### ServiceResponseReceived

Emitted when a provider responds to a request this node sent.

```json
{
  "type": "service_response_received",
  "request_id": "uuid",
  "service_id": "weather.v1",
  "status": "ok",
  "body": "{\"temp\": 72}",
  "provider_peer_id": "bob"
}
```

The `status` field is application-defined (e.g. `"ok"`, `"error"`). If a request targets a service the provider hasn't registered, the protocol auto-responds with `status: "not_found"` and no event is emitted to the provider's app.

## Mobile (UniFFI)

All methods are exposed via UniFFI to Swift, Kotlin, and React Native.

```typescript
// Register
await protocol.registerService("weather.v1", "2.0", { format: "json" });

// Discover
const queryId = await protocol.discoverServices("weather.v1");

// Handle events
protocol.onEvent((event) => {
  switch (event.type) {
    case "service_discovered":
      // Found a provider — send a request
      protocol.sendServiceRequest(
        event.provider_peer_id, event.service_id, "get_forecast", '{"city": "NYC"}'
      );
      break;

    case "service_request_received":
      // Incoming request — respond
      const result = handleRequest(event.method, event.body);
      protocol.respondToServiceRequest(
        event.request_id, event.sender, event.service_id, "ok", result
      );
      break;

    case "service_response_received":
      // Got a response back
      console.log(`${event.status}: ${event.body}`);
      break;
  }
});
```

Events are delivered as JSON strings via the existing `EventCallback` / `poll_event` system. Parse the JSON and switch on `"type"`.

## Behavior Notes

- **Gossip flooding**: Discovery queries propagate to all known peers. Each node forwards to peers it hasn't already sent to. A 60-second dedup window prevents query storms.
- **No central registry**: Discovery is fully decentralized. Every node checks its own local service registry when a query arrives.
- **Transport-agnostic**: Service messages use the same `send_internal_message` path as all other control messages — they benefit from DORS transport selection, reliability (ACKs/retries), and MLS encryption automatically.
- **Capabilities**: The `capabilities` HashMap on `ServiceDescriptor` lets providers advertise supported features (formats, versions, limits) before any request is made. Consumers can inspect these in `ServiceDiscovered` events to pick the right provider.
