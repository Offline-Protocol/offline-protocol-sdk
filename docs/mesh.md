# Mesh Networking Guide

## Overview

The Offline Protocol SDK implements a mesh networking layer that enables peer-to-peer message delivery without requiring internet connectivity. Messages are exchanged between directly connected devices using BLE and Wi-Fi Direct, creating a resilient communication fabric that works in disconnected environments.

This guide explains how the mesh network operates, from peer discovery and connection management to message delivery and reliability guarantees.

## What is Mesh Networking?

In traditional networking, messages travel through centralized infrastructure—routers, cell towers, or internet servers. Mesh networking takes a different approach: every participating device can exchange messages directly with nearby peers, without relying on central infrastructure.

The Offline Protocol mesh enables:
- **Direct Peer Communication**: Messages are exchanged directly with connected peers
- **Multi-Transport Support**: BLE, Wi-Fi Direct, and Internet transports work together
- **Resilient Connectivity**: No single point of failure; the network adapts as devices join and leave
- **Store-and-Forward**: Messages are queued and retried when delivery fails

## Architecture Overview

The mesh networking system consists of several layers:

### Transport Layer
Handles the physical transmission of messages over BLE, Wi-Fi Direct, or Internet connections. DORS (Dynamic Offline Relay Switch) automatically selects the best transport based on network conditions.

### Connection Management Layer  
The native `MeshController` (implemented in Swift/Kotlin) manages peer connections, including which peers to connect to and when to rebalance connections.

### Reliability Layer
Handles acknowledgments, retries, and deduplication to ensure messages are delivered reliably over inherently unreliable wireless links.

### Protocol Layer
The Rust core manages message lifecycle, events, and coordinates between layers.

## How the Mesh is Formed

The mesh network forms automatically as devices discover each other and establish connections. This section explains the complete mesh formation lifecycle.

### Phase 1: Initialization

When a device starts the Offline Protocol:

1. **Generate Device Identity**: Each device has a unique device ID used to identify it in the mesh
2. **Initialize Mesh Controller**: Creates the native `MeshController` that manages peer selection decisions
3. **Start Bluetooth Managers**: 
   - Central Manager (scanner) - finds other devices
   - Peripheral Manager (advertiser) - makes this device discoverable
4. **Begin Advertising**: Start broadcasting presence to nearby devices
5. **Begin Scanning**: Start looking for other devices in range

### Phase 2: Advertising

Each device broadcasts a BLE advertisement containing:
- The Offline Protocol service UUID
- Mesh metadata encoded in service data (when supported by platform)

The advertisement metadata includes:
- **Degree**: Current number of connections (0-15)
- **Free Slots**: Available connection capacity
- **Node Score**: Quality score (0-1) based on battery, uptime, stability
- **Uptime**: How long the device has been running
- **Battery Level**: Current battery percentage
- **Load Percent**: Current processing load
- **Node ID Hash**: Unique identifier for this device

This metadata allows other devices to make informed connection decisions before attempting to connect.

### Phase 3: Discovery

When a device discovers another device's advertisement:

1. **Parse Advertisement**: Extract mesh metadata from the advertisement packet
2. **Track RSSI**: Record signal strength for connection quality assessment
3. **Update MeshController**: Feed the advertisement to `observeAdvertisement()` for tracking
4. **Evaluate Connection**: Call `shouldInitiateOutbound()` to decide if we should connect

### Phase 4: Connection Decision

The MeshController evaluates whether to connect based on:

**Capacity Check**:
- Is there room for another connection? (under `maxConnections`)
- If at capacity, is there a worse peer we could swap out?

**Score Comparison**:
- Calculate the candidate's score using the weighted formula (RSSI, availability, uptime, battery, stability, load)
- If swapping, ensure the candidate scores higher than our worst current peer plus hysteresis

**Decision Outcomes**:
- **Accept**: Connect to the peer (intra-cluster or inter-cluster)
- **Accept with Eviction**: Connect but first disconnect a lower-scoring peer
- **Reject**: Don't connect (capacity full, local links preferred, etc.)

### Phase 5: Connection Establishment

When a connection is approved:

1. **Initiate BLE Connection**: Central manager connects to the peripheral
2. **Discover Services**: Find the Offline Protocol service on the peer
3. **Discover Characteristics**: Locate message and device ID characteristics
4. **Exchange Device IDs**: Read the peer's device ID characteristic
5. **Enable Notifications**: Subscribe to message notifications for incoming data
6. **Register Connection**: Inform MeshController that the connection is active

After this, the devices can exchange messages.

### Phase 6: Handling Inbound Connections

When another device connects to us:

1. **Receive Connection**: Peripheral manager detects the inbound connection
2. **Evaluate Request**: Call `shouldAcceptInboundConnection()` with any available metadata
3. **Accept or Reject**: Based on capacity and scoring (similar logic to outbound)
4. **Setup Communication**: If accepted, prepare characteristics for message exchange
5. **Register Connection**: Track the new peer in MeshController

### Phase 7: Continuous Rebalancing

The mesh continuously optimizes itself:

1. **Periodic Evaluation**: Every 15 seconds (configurable), check if connections should change
2. **Score All Candidates**: Re-evaluate all visible peers that we're not connected to
3. **Compare to Current Peers**: Find our lowest-scoring current connection
4. **Consider Swapping**: If a candidate beats our worst peer by the hysteresis threshold, swap
5. **Cluster Bridging**: Prioritize connections that bridge separate network clusters

This ensures the mesh topology adapts as devices move, battery levels change, or new devices appear.

### Phase 8: Disconnection Handling

When a peer disconnects:

1. **Detect Disconnection**: BLE notifies us the connection was lost
2. **Classify Disconnect**:
   - Temporary (signal loss, background state): Attempt reconnection
   - Permanent (timeout, pairing removed): Clean up and don't reconnect
3. **Update MeshController**: Unregister the disconnected peer
4. **Update Advertisement**: Refresh our advertisement to show updated degree/slots
5. **Trigger Rebalance**: Check if a waiting candidate should now connect

## Peer Discovery and Connection

### BLE Mesh Advertisements
Devices broadcast advertisement packets containing metadata about their current state:
- **Degree**: Number of active connections (0-15)
- **Free Slots**: Available connection capacity (0-15)
- **Node Score**: Overall quality score (0-1)
- **Uptime**: How long the device has been active
- **Battery Level**: Current battery percentage
- **Load**: Current processing load
- **Node ID Hash**: Unique identifier for the device

Other devices observe these advertisements to make informed connection decisions.

### Connection Decisions
The `MeshController` evaluates potential connections using a multi-factor scoring system:

| Factor | Weight | Description |
|--------|--------|-------------|
| RSSI | 35% | Signal strength to the peer |
| Availability | 20% | Connection capacity of the peer |
| Uptime | 15% | How long the peer has been running |
| Battery | 15% | Peer's battery level |
| Stability | 10% | Connection reliability history |
| Load | 5% | Current processing load on the peer |

### Connection Budget
Each device maintains a configurable number of connections:
- **Minimum Connections**: Default 1 (ensures basic connectivity)
- **Maximum Connections**: Default 4 (balances reach vs resource usage)

When at capacity, new connections are only accepted if the candidate scores better than the worst current peer.

### Connection Rebalancing
Periodically (default: every 15 seconds), the mesh evaluates whether to swap connections:
1. Find the worst-scoring current peer
2. Compare against best available candidate
3. If the candidate beats the current peer by the hysteresis threshold, swap
4. This prevents "connection flapping" from minor score fluctuations

## Message Delivery

### Sending Messages
When an application sends a message:
1. The message is assigned a unique ID, TTL, and timestamp
2. DORS selects the best transport (BLE, Wi-Fi Direct, or Internet)
3. The message is sent to all connected peers on that transport
4. The message is tracked for acknowledgment

### Receiving Messages
When a device receives a message:
1. Check deduplication (skip if already seen)
2. If the message is addressed to this device, deliver to the application
3. Send an acknowledgment back to the sender
4. Increment hop count (for tracking)

### Message Metadata
Each message includes:
- **Message ID**: Unique identifier for deduplication
- **TTL (Time-To-Live)**: Maximum hops allowed
- **Hop Count**: Number of hops taken so far
- **Timestamp**: When the message was created
- **Priority**: Affects retry ordering

## TTL (Time-To-Live) Management

TTL prevents messages from circulating indefinitely. Each hop decrements the TTL, and messages with TTL=0 are not processed.

### Default TTL
The SDK uses a base TTL of 8 hops, which is configurable. This provides reasonable reach for typical network sizes.

### TTL Considerations
- Higher TTL increases delivery probability but also network load
- Lower TTL is more efficient but may not reach distant peers
- Critical messages may warrant higher TTL values

## Reliability Layer

The reliability layer ensures messages are delivered despite the inherent unreliability of wireless links.

### Acknowledgment (ACK) Management
Messages that require acknowledgment are tracked:
- **Timeout**: Default 5 seconds to wait for ACK
- **Tracking**: Message ID, recipient, timestamp, retry count
- **Events**: `MessageDelivered` when ACK received, `MessageFailed` after max retries

When an ACK is received, the sender:
1. Marks the message as delivered
2. Emits a `MessageDelivered` event
3. Cancels any pending retries

### Retry Queue
Failed messages are queued for retry with exponential backoff:
- **Initial Delay**: 1 second
- **Backoff Factor**: 2x each retry (1s → 2s → 4s → 8s...)
- **Maximum Delay**: 30 seconds
- **Maximum Retries**: 5 attempts (configurable)

### Priority Ordering
The retry queue processes messages by priority:
1. **Critical**: Processed first, bypasses some safeguards
2. **High**: Processed before normal messages
3. **Normal**: Standard processing
4. **Low**: Processed last, may be delayed under load

Within the same priority, older messages go first.

### Outbox Persistence
Messages awaiting acknowledgment are stored in an outbox:
- Survives temporary transport failures
- Automatically cleaned up after successful delivery or max retries
- Configurable maximum lifetime

## Deduplication

Without deduplication, the same message could be processed multiple times. The deduplicator tracks seen message IDs.

### Bloom Filter Mode (Default)
For high-volume scenarios, a space-efficient bloom filter tracks message IDs:
- **Memory**: ~1MB per filter (constant regardless of message count)
- **False Positive Rate**: ~1% with default settings
- **Rotation**: Filters rotate every 15 minutes for automatic expiration

Bloom filters trade perfect accuracy for constant memory usage, making them suitable for resource-constrained devices.

### HashMap Mode
For exact tracking (no false positives):
- **Capacity**: 10,000 message IDs (configurable)
- **Retention**: 1 hour (configurable)
- **Eviction**: FIFO when capacity is reached

### Deduplication Behavior
When a message arrives:
1. Check if the message ID is in the deduplicator
2. If seen, silently drop the message
3. If new, process the message and add ID to the deduplicator

## Transport Layers

The mesh operates over multiple transport technologies, with DORS automatically selecting the best option.

### BLE (Bluetooth Low Energy)
- **Range**: ~10-30 meters
- **Throughput**: ~150 KB/s
- **Power**: Very low
- **Best For**: Dense environments, battery-constrained devices
- **Connection Model**: Maintains persistent connections to nearby peers

### Wi-Fi Direct
- **Range**: ~50-100 meters
- **Throughput**: ~2 MB/s
- **Power**: High
- **Best For**: File transfers, high-bandwidth needs
- **Connection Model**: Group-based connections

### Internet
- **Range**: Global
- **Throughput**: Variable (typically high)
- **Power**: Medium
- **Best For**: Hybrid apps, server integration
- **Connection Model**: WebSocket to relay server

## Connection Management Details

### Score-Based Peer Selection
The `MeshController` uses a weighted scoring formula to evaluate peers:

```
Score = (RSSI × 0.35) + (Availability × 0.20) + (Uptime × 0.15) 
      + (Battery × 0.15) + (Stability × 0.10) + (Load × 0.05)
```

Each factor is normalized to 0-1 before weighting:
- **RSSI**: Maps -100 to -20 dBm → 0 to 1
- **Availability**: Based on free slots vs max connections
- **Uptime**: Saturates at 1 hour (longer = 1.0)
- **Battery**: Direct percentage mapping
- **Stability**: Historical connection reliability
- **Load**: Inverse of current load (lower load = higher score)

### Hysteresis and Stability
To prevent connection flapping:
- **Score Hysteresis**: Default 5% improvement required to swap
- **Rebalance Interval**: Minimum 15 seconds between evaluations
- **Connection Cooldown**: 7.5 seconds after a new connection before considering removal

### Cluster Bridging
The mesh attempts to detect and bridge separate clusters:
- Tracks cluster signatures based on connected peer hashes
- Prioritizes connections that bridge different clusters
- Helps unite fragmented networks

## Cluster Formation and Detection

In real-world deployments, the mesh network often fragments into clusters—groups of devices that can communicate with each other but not with devices in other clusters. The MeshController includes mechanisms to detect and bridge these clusters.

### What is a Cluster?

A cluster is a connected subgraph of the mesh where all devices can reach each other through their connections. Clusters form naturally due to:
- **Physical Distance**: Devices too far apart can't establish BLE connections
- **Obstacles**: Walls, buildings, or terrain block radio signals
- **Movement**: Devices move in groups (e.g., people walking together)
- **Timing**: Devices that start at different times may miss each other

### Cluster Signatures

Each device maintains a **cluster signature**—a 64-bit value computed by XORing the node hashes of all connected peers:

```
clusterSignature = selfHash XOR peer1Hash XOR peer2Hash XOR ... XOR peerNHash
```

This provides a cheap way to detect if two nodes are in different "neighborhoods":
- Devices in the same cluster tend to have similar signatures (they share connected peers)
- Devices in different clusters have different signatures (different peer sets)

### Cluster Difference Estimation

When evaluating a connection candidate, the MeshController estimates how "different" their cluster is:

1. **Known Peer Check**: If we've seen this candidate as a peer of our connected nodes, they're likely in the same cluster (difference = 0.2)

2. **Signature Comparison**: If we have the candidate's cluster signature:
   - XOR our signature with theirs
   - Count the differing bits
   - Normalize to 0-1 (0 = identical, 1 = completely different)

3. **Unknown Candidate**: If we have no information, assume moderate difference (0.5)

### Bridge Detection

A candidate is considered a **bridge candidate** if their cluster difference exceeds 30%. Bridge candidates receive special treatment:

- **Swap Priority**: When evaluating whether to swap a current peer for a candidate, inter-cluster bridges get a score bonus (`bridgeFavor`, default 10%)
- **Connection Intent**: Bridge connections are marked as `interCluster` vs `intraCluster`
- **Role Assignment**: Bridge connections may be assigned the `.bridge` role instead of `.member`

### Bridge Swap Logic

When a candidate appears to bridge clusters, the swap evaluation is more lenient:

1. If the candidate is from a different cluster (difference > 50%), they get a 2x bridge favor bonus
2. Even if scores are similar, prefer the inter-cluster connection for network unity
3. Underserved candidates (few connections) in different clusters are prioritized

This ensures that when clusters come into range of each other, the mesh actively works to bridge them together.

## Hash Tables and Peer Tracking

The MeshController maintains several hash tables for efficient peer management. These data structures enable O(1) lookups for connection decisions and cache management.

### Primary Hash Tables

#### peersById: [String → PeerState]
Maps device IDs to their full peer state. Used for:
- Looking up peer information by device ID
- Iterating over known peers
- Managing connection lifecycle

```
"device-abc-123" → PeerState {
  deviceId: "device-abc-123"
  nodeHash: 0x1A2B3C4D5E6F7890
  role: .member
  metrics: PeerMetrics(rssi: -65, battery: 85, ...)
  lastUpdated: 2024-01-15T10:30:00Z
  lastActivity: 2024-01-15T10:29:55Z
  advertisedDegree: 3
  advertisedFreeSlots: 1
  advertisedScore: 0.78
  ...
}
```

#### peersByHash: [UInt64 → PeerState]
Maps 64-bit node hashes to peer state. Used for:
- Matching advertisement hashes to known peers
- Fast lookup when only the hash is known (from advertisements)
- Detecting if a candidate is already known

The node hash is computed from the device ID using SHA-256, truncated to 64 bits:
```
nodeHash = SHA256(deviceId)[0..8] as UInt64
```

#### activeConnections: [String → MeshRole]
Maps device IDs to their connection role. Only contains currently connected peers:
- `.member`: Normal cluster member
- `.bridge`: Inter-cluster bridge connection

Used for:
- Quick check if a peer is connected
- Counting current connections (degree)
- Computing cluster signatures

#### candidatesByHash: [UInt64 → RemoteCandidate]
Maps node hashes to candidate metadata. Contains peers we've seen via advertisements but aren't connected to:

```
0x1A2B3C4D5E6F7890 → RemoteCandidate {
  metadata: MeshAdvertisementData { degree: 2, freeSlots: 2, score: 0.72, ... }
  observedAt: 2024-01-15T10:28:00Z
  rssi: -70
}
```

Used for:
- Rebalancing decisions (finding better candidates)
- Estimating network density
- Tracking potential connections

#### observedClusterSignatures: [UInt64 → UInt64]
Maps node hashes to their cluster signatures (when known):
- Populated when we receive extended advertisements
- Used for cluster difference estimation
- Helps identify bridge candidates

### Cache Management

The hash tables are bounded to prevent memory exhaustion on resource-constrained devices.

#### Limits
- **maxPeerCacheSize**: 200 entries (default)
- **maxCandidateCacheSize**: 100 entries (default)
- **metadataTTL**: 120 seconds (entries expire after this)

#### Eviction Strategy

Peers are categorized into temperature tiers for eviction:

1. **Hot Peers** (actively connected): Never evicted
2. **Warm Peers** (recently seen, not connected): Evicted after cold peers
3. **Cold Peers** (not seen recently): Evicted first

Within each tier, Least Recently Used (LRU) ordering determines eviction priority. The `lastActivity` timestamp tracks when a peer was last seen or communicated with.

#### Pruning Process

Every 30 seconds (configurable via `cachePruneInterval`), the controller:

1. Prunes expired candidates (older than `metadataTTL`)
2. If candidates exceed `maxCandidateCacheSize`, evict oldest by `observedAt`
3. If peers exceed `maxPeerCacheSize`, evict cold peers first, then warm peers

### Node Hash Generation

Device IDs are converted to 64-bit hashes for compact representation in advertisements:

```swift
static func hash64(_ input: String) -> UInt64 {
    let digest = SHA256.hash(data: Data(input.utf8))
    return digest.prefix(8).reduce(0) { ($0 << 8) | UInt64($1) }
}
```

This provides:
- **Compactness**: 8 bytes vs potentially long device IDs
- **Privacy**: Original device ID is not broadcast
- **Collision Resistance**: SHA-256 provides strong distribution
- **Determinism**: Same input always produces same hash

## Rust Core: Path Selection and Routing

The Rust `offline-protocol-router` crate implements path selection using gossip-based probabilistic forwarding to prevent broadcast storms in large networks. The system also includes a gradient routing table for directed message delivery when routes are known.

### Path Selection Overview

The router uses a multi-factor scoring system to select the best neighbors for message forwarding:

1. **Gossip-Based Forwarding**: In large networks, messages are forwarded probabilistically to a subset of neighbors to prevent broadcast storms
2. **Top-K Selection**: Selects the top K neighbors (default: configurable via `forwardToTopK`) based on path scores
3. **Gradient Routing**: When routes are known, uses directed delivery; otherwise falls back to flooding

### Path Scoring

Each neighbor is scored based on:
- **RSSI**: Signal strength to the neighbor
- **Link Quality**: Historical connection reliability
- **Relay Information**: Congestion level and capacity if the neighbor is a relay
- **Hop Count**: Distance to destination (if known via gradient routing)

### Data Structures

#### routes: HashMap<String, Vec<RouteEntry>>
Maps destination user IDs to a list of possible routes:

```
"user-alice" → [
  RouteEntry { next_hop: "peer-A", hop_count: 2, quality: 0.85, last_seen: ... },
  RouteEntry { next_hop: "peer-B", hop_count: 3, quality: 0.72, last_seen: ... },
]
```

Each destination can have multiple routes (up to `max_routes_per_destination`, default 3), enabling failover if a route becomes unavailable.

#### neighbor_destinations: HashMap<String, Vec<String>>
Reverse mapping from neighbors to destinations reachable through them:

```
"peer-A" → ["user-alice", "user-bob", "user-charlie"]
"peer-B" → ["user-alice", "user-david"]
```

Used for efficient cleanup when a neighbor disconnects—removes all routes through that neighbor.

### Route Entry Structure

Each route entry contains:
- **next_hop**: The neighbor to forward messages through
- **hop_count**: Number of hops to destination (used for shortest-path selection)
- **quality**: Route quality score (0.0-1.0, higher is better)
- **last_seen_ms**: Timestamp (milliseconds since epoch) for TTL-based expiration

### Configuration

| Parameter | Default | Description |
|-----------|---------|-------------|
| `max_routes_per_destination` | 3 | Maximum alternate routes per destination |
| `route_ttl_secs` | 300 | Route expiration time (5 minutes) |
| `max_routing_table_size` | 1000 | Maximum total entries |

### UniFFI API

The gradient routing table is exposed via the `OfflineProtocol` interface with the following methods:

| Method | Description |
|--------|-------------|
| `learn_route(destination, next_hop, hop_count, quality)` | Record a learned route from an incoming message |
| `get_best_route(destination) -> RouteEntry?` | Get the highest-quality route to a destination |
| `get_all_routes(destination) -> [RouteEntry]` | Get all valid (non-expired) routes to a destination |
| `has_route(destination) -> bool` | Check if any route exists to the destination |
| `remove_neighbor_routes(neighbor_id)` | Remove all routes through a neighbor (call on disconnect) |
| `cleanup_expired_routes()` | Clean up expired routes (call periodically) |
| `get_routing_stats() -> RoutingStats` | Get routing table statistics for monitoring |
| `update_routing_config(config)` | Update routing configuration at runtime |

### Route Learning

When a message is received, the native layer should call `learn_route()`:
1. The sender can be reached through the delivering neighbor
2. The hop count is incremented from the message's hop count
3. Route quality is computed from link metrics (RSSI, connection stability)

If the destination already has maximum routes, the lowest-quality route is evicted.

### Route Selection

When sending to a known destination:
1. Filter out expired routes (older than TTL)
2. Sort by quality score
3. Return the highest-quality route

### Usage Example (Swift)

```swift
// On message receive - learn route to sender through delivering neighbor:
protocol.learnRoute(
    destination: message.sender,
    nextHop: neighborId,
    hopCount: UInt8(message.hopCount + 1),
    quality: computeQuality(rssi: rssi, stability: linkStability)
)

// On send - use directed delivery if route known:
if let route = protocol.getBestRoute(destination: recipient) {
    // Forward via learned route
    sendToNeighbor(route.nextHop, message: data)
} else {
    // Fall back to flooding
    broadcastToAllNeighbors(message: data)
}

// On peer disconnect - cleanup stale routes:
protocol.removeNeighborRoutes(neighborId: disconnectedPeerId)

// Periodic maintenance (e.g., every 30 seconds):
protocol.cleanupExpiredRoutes()
```

### Usage Example (Kotlin)

```kotlin
// On message receive - learn route to sender through delivering neighbor:
protocol.learnRoute(
    destination = message.sender,
    nextHop = neighborId,
    hopCount = (message.hopCount + 1).toUByte(),
    quality = computeQuality(rssi, linkStability)
)

// On send - use directed delivery if route known:
val route = protocol.getBestRoute(destination = recipient)
if (route != null) {
    // Forward via learned route
    sendToNeighbor(route.nextHop, message = data)
} else {
    // Fall back to flooding
    broadcastToAllNeighbors(message = data)
}

// On peer disconnect - cleanup stale routes:
protocol.removeNeighborRoutes(neighborId = disconnectedPeerId)

// Periodic maintenance (e.g., every 30 seconds):
protocol.cleanupExpiredRoutes()
```

### Cleanup Operations

- **removeNeighborRoutes()**: Called on disconnect, removes all routes through that neighbor
- **cleanupExpiredRoutes()**: Periodic cleanup of stale routes (recommended: every 30 seconds)

### Gradient Routing Table

The gradient routing table learns routes from incoming messages. When a message arrives from a neighbor, we record that neighbor as a route to the message's original sender. Over time, this builds a map of how to reach known destinations.

**Current Status**: The gradient routing table is available via UniFFI bindings and can be used by native implementations for directed delivery when routes are known. The Rust router supports both gradient routing (when routes exist) and gossip-based flooding (when routes are unknown).

### Monitoring

Use `getRoutingStats()` to monitor routing table health:

```swift
let stats = protocol.getRoutingStats()
print("Destinations: \(stats.destinationCount), Routes: \(stats.routeCount)")
```

## Events and Monitoring

### Transport Events
- `transport_switched`: Fired when DORS changes transport (includes reason)
- Transport metrics are continuously updated

### Message Events
- `MessageSent`: Message queued for delivery
- `MessageReceived`: Message received from peer
- `MessageDelivered`: ACK received, delivery confirmed
- `MessageFailed`: Max retries exceeded

### Connection Events (Native Layer)
The native `MeshController` emits connection-related events that can be observed for debugging.

## Configuration

### Protocol Configuration

```typescript
const config = {
  appId: 'my-app',
  userId: userId,
  initialTtl: 8,  // Default message TTL
  
  reliability: {
    ack: {
      defaultTimeoutMs: 5000,    // Wait 5s for ACK
    },
    retry: {
      maxRetries: 5,             // Max retry attempts
      initialDelayMs: 1000,      // First retry after 1s
      maxDelayMs: 30000,         // Cap delay at 30s
      backoffFactor: 2.0,        // Double delay each retry
    },
    dedup: {
      useBloomFilter: true,      // Use bloom filter mode
      maxTrackedMessages: 10000, // HashMap mode capacity
      retentionTimeSecs: 3600,   // 1 hour retention
    },
  },
};
```

### Mesh Controller Configuration (Native)
The native `MeshController` has additional settings:
- `minConnections`: Minimum peers to maintain (default: 1)
- `maxConnections`: Maximum peers allowed (default: 4)
- `rebalanceInterval`: Time between rebalance checks (default: 15s)
- `metadataTTL`: How long to cache peer metadata (default: 120s)

## Best Practices

### Battery Conservation
- The mesh automatically considers battery in peer scoring
- Devices with low battery are less likely to be selected as peers
- Charging devices are preferred for connections

### Message Priority
- Use **Critical** sparingly (bypasses some safeguards)
- **Normal** priority is appropriate for most messages
- **Low** priority for non-urgent background sync

### Network Monitoring
- Listen for `MessageFailed` events to detect delivery issues
- Monitor transport switch events to understand DORS behavior
- Track delivery success rates over time

### Connection Tuning
- Start with defaults and observe behavior
- Increase `maxConnections` for denser networks
- Decrease `rebalanceInterval` for faster adaptation

## Troubleshooting

### Messages Not Delivering
1. Check that both devices are on the same transport
2. Verify they're within range (BLE: ~10-30m)
3. Check for `MessageFailed` events to see retry counts
4. Ensure TTL is sufficient for the network topology

### Frequent Disconnections
1. Check signal strength (RSSI in advertisements)
2. Increase `connectionCooldown` to stabilize connections
3. Reduce `rebalanceInterval` frequency
4. Check for interference from other BLE devices

### High Battery Drain
1. Reduce `maxConnections` to limit active peers
2. Ensure DORS is selecting BLE over Wi-Fi Direct when appropriate
3. Check for excessive retry activity (may indicate poor connectivity)

### Duplicate Messages
1. Verify deduplication is enabled
2. Check bloom filter capacity if using high message volumes
3. Consider increasing `retentionTimeSecs` if messages are slow to propagate
