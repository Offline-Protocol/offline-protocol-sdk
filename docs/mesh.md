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
3. The message is sent to its recipient over that transport; if the recipient
   is out of range, it is handed to nearby devices to carry
4. The message is tracked for acknowledgment

### Receiving Messages
When a device receives a message:
1. If it is addressed to someone else, consider carrying it onward (see
   [What a device does with a frame for someone else](#what-a-device-does-with-a-frame-for-someone-else))
   and stop — a message passing through is not part of this device's own
   exchange
2. Check deduplication (skip if already seen)
3. Deliver to the application
4. Send an acknowledgment back to the sender, which travels through the mesh if
   the sender is not directly reachable

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
- **Timeout**: Default 10 seconds to wait for ACK
- **Tracking**: Message ID, recipient, timestamp, retry count
- **Events**: `MessageDelivered` when ACK received, `MessageFailed` after max ACK retries

When an ACK is received, the sender:
1. Marks the message as delivered
2. Emits a `MessageDelivered` event
3. Removes the message from both the retry queue and outbox

### Retry Queue
Failed messages are queued for retry with exponential backoff:
- **Initial Delay**: 1 second
- **Backoff Factor**: 2x each retry (1s → 2s → 4s → 8s...)
- **Maximum Delay**: 30 seconds
- **No attempt limit**: The retry queue is a pure scheduling mechanism. Only ACK timeouts (not transport failures) count toward the retry limit (default: 10).

When a transport becomes available (peer discovered, internet reconnects), pending messages are flushed immediately — bypassing backoff timers.

For the full delivery lifecycle including client-side persistence patterns, see [Message Delivery & Reliability](message-delivery.md).

### Priority Ordering
The retry queue processes messages by priority:
1. **Critical**: Processed first, bypasses some safeguards
2. **High**: Processed before normal messages
3. **Normal**: Standard processing
4. **Low**: Processed last, may be delayed under load

Within the same priority, older messages go first.

### Outbox
Messages awaiting acknowledgment are stored in an in-memory outbox:
- Survives temporary transport failures within the process lifetime
- Automatically cleaned up after successful delivery or max ACK retries
- Configurable maximum lifetime (default: 7 days); expiry emits a terminal `message_failed` event
- Regular-message entries are persisted when a message storage backend is configured and restored on the next `start()` — see [Client-Side Persistence](message-delivery.md#client-side-persistence)

## Deduplication

Without deduplication, the same message could be processed multiple times. The deduplicator tracks seen message IDs.

### HashMap Mode (Default)
Exact tracking with no false positives:
- **Capacity**: 1,000 message IDs (configurable)
- **Retention**: 1 hour (configurable)
- **Eviction**: LRU when capacity is reached

### Bloom Filter Mode
For high-volume scenarios, a space-efficient bloom filter tracks message IDs:
- **Memory**: ~1MB per filter (constant regardless of message count)
- **False Positive Rate**: ~1% with default settings
- **Rotation**: Filters rotate every 15 minutes for automatic expiration

Bloom filters trade perfect accuracy for constant memory usage, making them suitable for resource-constrained devices. Enable with `useBloomFilter: true`.

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

A message addressed to a device that is out of range is carried by the devices in between. Each one that receives it hands it onward to a bounded set of its own neighbors, until it reaches its recipient or runs out of hops. The recipient's acknowledgement travels back the same way.

### What a device does with a frame for someone else

Every frame passing through runs the same sequence before it goes anywhere, and each step covers something the others cannot:

1. **Handled once** — an id a device has already dealt with is ignored. Without this, copies circulate until their hop budget runs out and every device repeats every copy.
2. **Accepted from this neighbor?** — each link has its own allowance, so one noisy or hostile neighbor cannot consume the device's whole capacity for carrying traffic.
3. **Hop budget** — the remaining budget a frame claims is cut down to what local policy would have issued, then spent. Nothing authenticates that claim, so it is never taken at face value. In a dense neighborhood the ceiling is lower: with more devices in range, a message covers the same ground in fewer hops.
4. **Held briefly, and dropped if covered** — a frame waits a short randomized moment before going out, and is dropped if the same frame arrives again while it waits. In a crowded room the first device to transmit covers everyone in earshot and the rest stand down. This is what keeps cost tied to coverage rather than to the number of links, and it adapts as a room fills without any threshold to tune.
5. **Sent to a few, never backwards** — the frame goes to a bounded number of neighbors chosen by signal strength and capacity, never to the device it came from or the one that wrote it. If the recipient is a neighbor, it goes straight there instead.
6. **Within budget** — a device caps how many frames it forwards per second. The steps above assume frames are what they appear to be; this one holds regardless. Whatever arrives, a device cannot be made to transmit faster than its budget, so the failure mode under overload or attack is added delay rather than a radio that never goes quiet.

Carrying traffic is subject to the same relay policy as everything else (`allowRelay`, battery floors): a device that declines carries nothing, while still sending and receiving its own messages. Between "carries nothing" and "carries everything" the effort is **scaled rather than switched**: battery level and charging state continuously set how long this device waits before transmitting a forward, how many neighbors it fans out to, and how fast its forwarding budget refills. A device on mains power waits the shortest time and therefore usually transmits first, at which point its neighbors holding the same frame stand down having spent nothing; a device at 20% waits longest and most often ends up not transmitting at all. In a sparse neighborhood the weak device's wait always exceeds the capable one's, so the capable device wins every frame; in a crowded one the delay windows overlap and it becomes a strong tendency rather than a guarantee — deliberately, because a handicap grown to preserve the guarantee at any density would eventually exceed the point at which a held frame is abandoned instead of sent. Nothing switches off at a threshold, which is the point — a threshold that misjudges a device removes a link the network may have needed, while a scale that misjudges one costs only some redundancy. A device configured `relayPriority: 'always'`, or one that is charging, is exempt from the scaling and from the soft battery floor alike, stopped only by the hard 15% floor.

The scaling applies strictly to **other people's** traffic. A device's own messages — and its acknowledgements above all — are handed to the mesh at full fan-out and full rate however low its battery, because nobody else is holding a copy of those: for a forward a narrower fan-out costs redundancy, but for a frame this device originated the fan-out *is* the delivery attempt. Only the share of the radio spent carrying the room's traffic shrinks.

Whether an app is told this device "is a relay" (`isRelay`, `relay_promoted`, `relay_demoted`) is answered from the other end: it reports frames this device has **actually carried** over the last minute, not the conditions that would let it. So a device with every capability to relay reports `false` until traffic needs it — including any device with a working internet relay, since the mesh is only offered frames nothing else can deliver. Handing over its *own* messages — an acknowledgement above all — is deliberately not gated on that setting, or a device that declined to carry traffic could never answer anything that reached it across the mesh, and its sender would report a failure for a message that was delivered and read.

#### What "handled once" costs

Step 1 is what keeps a message from multiplying, and it has a price worth knowing. Once a device takes a frame on, it ignores that id for the next ten minutes — including the sender's own retransmissions of it. So if the device that accepted a frame then fails to pass it on (it walks out of range, its battery drops below the floor, its queue is full of newer traffic), that particular route is closed for the rest of the window, and the message has to arrive some other way: through a device that never accepted it, or over the internet relay once one of the two comes back online.

This is the trade every controlled flood makes — the alternative is a device re-forwarding the same message each time a copy reaches it, which is exactly the storm the step exists to prevent. Three things keep the cost bounded in practice: a frame is handed to several neighbors at once, so a single carrier walking away rarely closes every route; a device only records an id once it has genuinely *accepted* the frame, so one turned away for rate, hop budget or queue space leaves the way open for the next copy; and a frame that was accepted but then dropped **without ever being transmitted** — displaced by more urgent traffic, abandoned after waiting too long, or refused room on its way back to the queue — releases its id again, because nothing went on the air that a later copy could duplicate.

What this means for an app: a message is not lost when this happens, but it can be slow. Treat delivery as settled by the acknowledgement, never by elapsed time.

#### Reading the numbers

`mesh_relay_stats()` reports what a device has been carrying. Two counters are easy to confuse:

- `forwarded` — messages moved on someone else's behalf, counted once each. This is the contribution figure to show a user.
- `transmissions` — times a frame was put on a link, counting each link separately and including the device handing over its own messages. This is what the per-second budget bounds, so it is the one to compare against the ceiling.

`rate_deferred` rising means forwarding is hitting that ceiling — those frames are delayed, not dropped. `peer_rate_limited` means a single neighbor is sending more than its share. `dropped_for_capacity` should stay at zero; anything else means the device is seeing more traffic than it can remember having handled.

Two counters say whether that back-pressure is costing anything, and they are the ones to read before concluding a device is coping. `refused_queue_full` counts frames turned away because the pending queue was full, on arrival or on their way back to it, and `abandoned_overdue` counts queued forwards given up on after waiting too long past their due time. Both are real losses: the frame reached nobody, and only a copy behind it or the sender's own retransmission carries it now. Deferral is free to look healthy while either climbs, which is exactly the case a device that is quietly shedding traffic presents.

Every counter is cumulative for the lifetime of the instance, so a rate is a difference between two reads. `awaiting_transmission` is the exception: it is a gauge, the queue depth right now, and it goes down as well as up.

The device's own sends draw on the same per-second budget — it is one radio — but keep a small reserve that forwarding never touches, so a device carrying a busy neighborhood can still get its own messages and acknowledgements out.

Tunables live in `ProtocolConfig::mesh_relay`, and both the tunables and these counters cross to every binding. From React Native the section is `meshRelay` in the `create()` config, and the counters are `getMeshRelayStats()`; `getMeshRelayTunables()` reports what is actually in force, read from the governor rather than echoed back from what was passed in.

Two properties of that surface are deliberate. Every config field is **optional**, and an omitted one keeps the core's default rather than being restated by a binding: the defaults live here and nowhere else, so a partial section moves only the dials it names. The read side is the opposite, **every field required**, so no caller writes a fallback literal for an absent one. The section is applied at construction only. There is no runtime update, because the governor takes its snapshot when it is built and re-pointing it mid-flight would have to rebuild the token buckets and the suppression cache underneath in-flight forwards.

The suppression-cache sizing (`seen`) stays core-only. It is internal memory sizing rather than a policy dial.

#### An online device in a mixed neighborhood

At the moment of sending, a device offers frames to its neighbors only when it holds no other way to reach the recipient. A carrier that does its own routing — Internet, Nostr, Reticulum — normally counts as reachable for every recipient, and nothing is handed to the mesh. That keeps the ordinary online case free: a device with a working relay connection does not spend its neighbors' battery on traffic the relay serves perfectly well.

The catch is that the question being asked there is mostly about this device's own carriers, not about the recipient. So a mixed neighborhood — someone online standing next to someone who is not — needs more, and three things provide it:

- **A live link to the recipient.** A recipient this device holds a mesh link to is sent to over that link, whatever its own carriers are doing. This is the one answer that needs no claim from anybody: the link either exists at that instant or it does not. It matters when scoring would otherwise put an infrastructure carrier first (`preferOnline`, or a congested radio), because the alternative is routing to the relay, waiting for it to answer that the recipient is not there, and only then asking the neighbor who was addressable the whole time.
- **The relay's verdict.** When the relay reports it cannot reach the recipient, that is the one per-peer reachability fact a device ever receives, and it contradicts what the carrier status implied. The message is parked as before *and* handed to the neighbors, who may well be able to reach someone the relay cannot. Re-offered on each subsequent park, so a recipient who was out of range when the message was first parked is still reached later. Media chunks are offered the same way.
  A verdict is also **remembered**, not just acted on. It is recorded as a fact about that recipient on that carrier, so the *next* send does not have to repeat the failure to learn the same thing: a carrier that has said it cannot reach someone stops counting as a way to reach them until the fact ages out (ten minutes) or a presence answer supersedes it. Facts decay on purpose. A remembered "unreachable" that never expired would keep a path shut long after the recipient came back, and since nothing is ever settled by a claim, the worst a stale one can cost is latency.
- **How the message arrived.** An acknowledgement for a message that reached this device across the mesh goes back the way it came, whatever this device's own carriers say. Otherwise an online recipient answers over the relay, where an offline sender cannot see it — and that sender retransmits a message that was delivered and read, eventually reporting it failed. When this device is also online, the answer goes both ways; the duplicate costs one frame.

Because parking removes the pending acknowledgement, a parked message that is then delivered across the mesh is settled from the acknowledgement alone. Apps see the ordinary `message_delivered` event.

What is still not covered: a device whose only infrastructure is **Nostr** never receives an unreachable verdict at all — a broadcast relay reports no per-recipient delivery — so nothing contradicts the initial "reachable" answer and no mesh fallback fires for it. That gap is permanent for Nostr rather than unfinished: there is no verdict to be had. Reticulum is in the same position today for the opposite reason, one that can be closed: nothing it talks to answers per-recipient yet, and the parking machinery is already keyed to the verdict rather than to the relay, so a Reticulum gateway that reports one drives the same fallback with no further change here. Note also that carrier status is reported by the platform bridge and means "this carrier is up", not "the relay connection is authenticated"; a bridge that reports a connection it never authenticates produces no verdicts either, and its messages settle by acknowledgement timeout as they always did.

### How a forwarding device chooses

There is no routing table and no remembered path. A device that decides to
carry a frame offers it to the neighbors it can address **right now**, and the
choice among them is made from live link state: which links are up, their
signal, and what the forwarding governor will admit for that peer.

This is deliberate, and it is the whole reason the earlier learned-route layer
was deleted rather than finished. In the environments this is built for (a
crowd, a venue, a march) links appear and vanish in seconds, so a remembered
route is usually stale by the time it would be used, while a fresh choice among
current neighbors never is. Suppression makes the redundancy cheap: a neighbor
that hears someone else carry the frame stands down, so offering to several
costs far less than the arithmetic suggests.

The transport a frame leaves on is DORS's decision (see [DORS](dors.md)), and
DORS scores carriers, not recipients. It is deliberately recipient-blind; the
per-recipient facts that override it (a relay's unreachable verdict, a live
mesh link to the recipient) are applied at the send and acknowledgement seams
described above, never as another weight inside the score.

### Do apps need to do any of this?

No. Carrying messages for nearby devices is handled inside the SDK — an app
sends a message and the SDK works out whether it can be delivered directly,
handed to a neighbor to carry, or held for retry. There is no app-side
forwarding to write, and writing one is a mistake: a second forwarder would
transmit copies the SDK's own accounting knows nothing about, so neither the
handled-once check nor the per-second budgets would cover it.

There is no routing API to call either. Earlier releases exposed a
learned-route table over UniFFI (`learn_route`, `get_best_route`,
`get_all_routes`, `has_route`, `remove_neighbor_routes`,
`cleanup_expired_routes`, `get_routing_stats`, `update_routing_config`); it fed
a table nothing read, and the whole layer was removed in 0.23.0. Nothing
replaced it: forwarding needs no help from the app.

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
  profile: userId,
  initialTtl: 8,  // Default message TTL
  
  reliability: {
    ack: {
      defaultTimeoutMs: 10000,   // Wait 10s for ACK
    },
    retry: {
      maxRetries: 10,            // Max ACK retry attempts
      initialDelayMs: 1000,      // First retry after 1s
      maxDelayMs: 300000,        // Cap delay at 5 min (default)
      backoffFactor: 2.0,        // Double delay each retry
    },
    dedup: {
      useBloomFilter: false,     // HashMap mode (default); set true for bloom filter
      maxTrackedMessages: 1000,  // HashMap mode capacity
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
