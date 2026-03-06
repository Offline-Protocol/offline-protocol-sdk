# Service Discovery Example

A React Native app demonstrating the **Service Discovery & Request/Response** feature of the Offline Protocol SDK. Nearby devices can register, discover, and invoke services over the mesh network with zero internet connectivity.

## What this demonstrates

### Service Registration (Provide tab)
- Register services your device offers (e.g., `echo.v1`, `notes.v1`, `weather.v1`)
- Each service has an ID, version, and key-value capabilities
- Auto-responds to incoming requests with method-specific handlers

### Service Discovery (Discover tab)
- Broadcast discovery queries to find services on the mesh
- Filter by specific service ID or discover all available services
- See provider peer ID, hop count, version, and capabilities
- Send typed requests to discovered providers and receive responses

### Activity Log (Logs tab)
- Real-time log of all service discovery events
- Tracks inbound requests, outbound responses, peer joins/leaves

## How it works

```
Device A                          Device B
--------                          --------
registerService("echo.v1")
                                  discoverServices()
    <-- discovery query (gossip) --
    -- discovery response -->
                                  [ServiceDiscovered event]
                                  sendServiceRequest(A, "echo.v1", "ping", {...})
    <-- service request --
    [ServiceRequestReceived event]
    respondToServiceRequest(...)
    -- service response -->
                                  [ServiceResponseReceived event]
```

All communication uses BLE mesh networking. Discovery queries propagate via gossip flooding with 60-second deduplication. Requests and responses route through the DORS transport selector with automatic reliability (ACKs + retries).

## Running

```bash
# Install dependencies
npm install

# iOS
npx react-native run-ios

# Android
npx react-native run-android
```

## Testing with two devices

1. Install on two physical devices (BLE requires real hardware)
2. Start the protocol on both devices
3. On Device A: Register a service (e.g., "echo.v1")
4. On Device B: Tap "Discover All Services"
5. Device B will see Device A's echo service appear
6. On Device B: Tap "Send ping Request" on the discovered service
7. Device A auto-responds; Device B sees the response in logs

## Key SDK APIs used

```typescript
// Create a MeshServices instance (separate from OfflineProtocol)
const services = new MeshServices();

// Register a service
await services.registerService('echo.v1', '1.0', { format: 'json' });

// Discover services on the mesh
const queryId = await services.discoverServices();           // all
const queryId = await services.discoverServices('echo.v1');  // filtered

// Send a request to a discovered provider
const requestId = await services.sendServiceRequest(
  providerPeerId, 'echo.v1', 'ping', '{"message": "hello"}'
);

// Respond to an incoming request (in event handler)
protocol.on('service_request_received', async (event) => {
  await services.respondToServiceRequest(
    event.request_id, event.sender, event.service_id, 'ok', responseBody
  );
});

// Handle discovery and response events
protocol.on('service_discovered', (event) => { ... });
protocol.on('service_response_received', (event) => { ... });
```
