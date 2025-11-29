# Offline Protocol SDK

> Offline-first messaging protocol with intelligent multi-transport switching and mesh networking

## Quick Start

### For React Native Apps:

```bash
npm install @offline-protocol/mesh-sdk
```

```typescript
import { OfflineProtocol, MessagePriority } from '@offline-protocol/mesh-sdk';

const protocol = new OfflineProtocol({
  appId: 'my-app',
  userId: 'user123',
  bleEnabled: true,
  preferOnline: false,
});

await protocol.start();
const messageId = await protocol.sendMessage(
  'recipient456',
  'Hello!',
  MessagePriority.Medium
);
```


## Building the SDK

### Prerequisites:
- Rust (via rustup)
- uniffi-bindgen: `cargo install uniffi --features="cli"`
- ndk: `cargo install cargo-ndk`

### Build UniFFI Libraries:

```bash
cd bindings/react-native

# Build for all platforms
npm run build:uniffi:all

# Or build individually
npm run build:uniffi:ios      # iOS only
npm run build:uniffi:android  # Android only

# Regenerate bindings after UDL changes
npm run generate:bindings
```


## Architecture

The SDK consists of modular Rust crates:

- **offline-protocol-core** - Core types and data structures
- **offline-protocol-transport** - Multi-transport abstraction (BLE, WiFi, Internet)
- **offline-protocol-router** - DORS routing and relay management
- **offline-protocol-reliability** - ACKs, retries, deduplication
- **offline-protocol** - Main protocol engine
- **offline-protocol-uniffi** - UniFFI bindings for Swift/Kotlin (NEW!)

## DORS: Dynamic Offline Relay Switch

DORS is the intelligent transport selection engine at the heart of the Offline Protocol SDK. It automatically chooses and switches between Internet, BLE Mesh, and Wi-Fi Direct based on real-time network conditions, ensuring optimal message delivery.

### How DORS Works

DORS evaluates each available transport using a multi-factor scoring system:

1. **Signal Strength** (RSSI for wireless transports)
2. **Proximity** (hop count - lower is better)
3. **Bandwidth** (throughput capability)
4. **Congestion** (queue depth and failure rate)
5. **Energy Efficiency** (battery impact)
6. **Reliability** (recent delivery success ratio)
7. **Available Capacity** (current queue pressure / load)

Each factor is scored 0-100, then weighted according to the transport type's characteristics. DORS prevents rapid transport switching ("flapping") using:
- **Hysteresis**: New transport must score significantly higher (default: 15 points)
- **Cooldown**: Wait period after switching (default: 20 seconds)
- **Stability Window**: New transport must be consistently better (default: 8 seconds)

### Transport-Specific Scoring

**BLE Transport:**
- Optimized for energy efficiency, signal quality, and available capacity
- Best for: Dense urban areas, low-power devices, short-range mesh
- Scoring weights: Signal (30%), Energy (30%), Congestion (15%), Proximity (15%), Reliability (5%), Load (5%)

**WiFi Direct Transport:**
- Optimized for high throughput and direct peer connections
- Best for: File transfers, video streaming, high-bandwidth needs
- Scoring weights: Bandwidth (35%), Proximity (20%), Congestion (20%), Reliability (15%), Load (10%)

**Internet Transport:**
- Prioritizes server connectivity while considering reliability and congestion
- Best for: Hybrid apps with server infrastructure
- Scoring: Baseline score (10 points, or 25 points if `preferOnline` is enabled), plus bandwidth (35%), reliability (30%), congestion (15%), energy (10%), load (10%)

### Escalation Logic

DORS can automatically escalate from BLE to WiFi Direct when:
- BLE retry failures reach threshold (default: 2 failures)
- RSSI stays below threshold for sustained period (default: 10s)
- Queue congestion persists (default: 50 messages for 10s)
- Messages approach TTL exhaustion (≤2 hops remaining)

For detailed configuration options, see the [DORS Configuration Guide](docs/dors-configuration.md).

## Mesh Network Architecture

The Offline Protocol SDK implements a cluster-based self-organizing mesh network that enables devices to communicate even when not directly connected, extending range through multi-hop routing across clusters.

### Cluster-Based Topology

The mesh organizes devices into clusters with two connection roles:

**Connection Roles:**
- **MEMBER**: Devices within the same cluster (intra-cluster connections)
- **BRIDGE**: Devices connecting different clusters (inter-cluster connections)

**Connection Budget:**
- Each device maintains up to 4 active BLE connections (configurable)
- When budget is full, the system evaluates whether to evict existing peers to make room for better connections

### How the Mesh Works

**1. Discovery and Advertisement**
- Devices continuously scan for BLE advertisements containing mesh metadata
- Advertisements include: node ID hash, free slot estimates, and peer metrics
- MeshController observes advertisements to build a view of network topology

**2. Connection Decision Process**
When a device discovers a peer, MeshController evaluates whether to connect:

```
1. Check available connection slots
   - If slots available → Connect (INTRA_CLUSTER or INTER_CLUSTER based on free slots)
   
2. If budget full, evaluate peer swap:
   - Calculate candidate score (RSSI, battery, uptime, stability, load)
   - Compare with worst existing peer
   - If candidate significantly better → Evict worst peer and connect
   - Otherwise → Reject connection
```

**3. Connection Intent and Role Assignment**
- **INTRA_CLUSTER**: Peer has free slots → MEMBER role (same cluster)
- **INTER_CLUSTER**: Peer has no free slots → BRIDGE role (different cluster)
- Role determines how messages are routed through the mesh

**4. Message Routing**
- Direct delivery when recipient is a connected peer (1 hop)
- Multi-hop routing through cluster members and bridges:
  ```
  Device A (Cluster 1) ──[MEMBER]──> Bridge Device ──[BRIDGE]──> Device B (Cluster 2)
           (1 hop)                      (1 hop)
           Total: 2 hops, TTL: 8 → 6
  ```
- Messages traverse clusters via bridge connections when needed
- Path selection considers cluster topology, peer quality, and remaining TTL

**5. Rebalancing**
- MeshController periodically evaluates network topology
- Can initiate new connections to optimize cluster connectivity
- Evicts underperforming peers to make room for better connections

### Peer Scoring

MeshController scores peers using weighted factors:
- **RSSI** (35%): Signal strength
- **Availability** (20%): Connection stability and uptime
- **Uptime** (15%): How long device has been active
- **Battery** (15%): Battery level and charging status
- **Stability** (10%): Connection reliability
- **Load** (5%): Current queue pressure

### Hop Count and TTL

- **TTL (Time To Live)**: Each message starts with a TTL (default: 8 hops)
- **Hop Count**: Increments with each device the message passes through
- **Expiration**: Messages expire if TTL reaches 0 before delivery
- **Path Selection**: The system prefers paths with sufficient remaining TTL and avoids overloaded clusters

### Network Topology Characteristics

The mesh network is:
- **Cluster-Based**: Devices organize into clusters with bridge connections between them
- **Self-Organizing**: No central coordinator required
- **Dynamic**: Topology adapts as devices join/leave and clusters merge/split
- **Quality-Aware**: Connection decisions based on peer scores, not just availability
- **Budget-Managed**: Connection limits prevent network overload
- **Fault-Tolerant**: Multiple paths through clusters provide redundancy

This mesh architecture enables communication in scenarios where:
- Devices are spread over a large area (stadium, campus, event)
- Internet connectivity is unavailable or unreliable
- Direct peer-to-peer connections are intermittent
- Network partitions occur (devices moving in/out of range)
- Devices need to form efficient clusters for optimal routing

For more details, see the [Architecture Documentation](docs/architecture.md) and [Transport Architecture Guide](docs/transport-architecture.md).

## Development

### Running Tests:
```bash
cargo test --workspace
```

### Building:
```bash
cargo build --workspace --release
```


---
