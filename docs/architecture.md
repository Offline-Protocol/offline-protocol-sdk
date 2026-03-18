# Architecture Deep Dive

This document provides a deep dive into the Offline Protocol SDK architecture.

## Design Philosophy

1. **Safety First**: 95% safe Rust, unsafe isolated to FFI
2. **Write Once**: Core logic shared across all platforms
3. **Zero-Copy**: Minimize allocations and copying
4. **Modular**: Each crate has a single responsibility
5. **Testable**: ~800 tests covering all critical paths

## Crate Organization

### 1. offline-protocol-core

**Purpose**: Fundamental types with zero dependencies.

**Key Types**:
- `Message`, `MessageId`, `MessagePriority`
- `UserId`, `AppId`, `TTL`, `HopCount`, `Timestamp`
- `Error` type with `thiserror`

**Safety**: `#![deny(unsafe_code)]` - 100% safe Rust

**Dependencies**: `serde`, `uuid`, `chrono`

### 2. offline-protocol-transport

**Purpose**: Transport abstraction layer.

**Key Components**:
- `Transport` trait (send, receive, status, metrics)
- `TransportType` enum
- `TransportMetrics` for monitoring
- `MockTransport` for testing

**Safety**: `#![deny(unsafe_code)]` - 100% safe Rust

**Dependencies**: `offline-protocol-core`

### 3. offline-protocol-router

**Purpose**: DORS engine and routing logic.

**Key Components**:
- `TransportSelector` - DORS algorithm with multi-factor scoring
- `RelayManager` - Promotion/demotion with battery awareness
- `PathSelector` - Optimal relay selection

**Algorithms**:
- Transport scoring: Signal + Proximity + Bandwidth + Congestion + Energy
- Hysteresis prevents flapping (15-point threshold)
- Cooldown timer (20 seconds)
- Stability window (8 seconds)

**Safety**: `#![deny(unsafe_code)]` - 100% safe Rust

**Dependencies**: `offline-protocol-core`, `offline-protocol-transport`

### 4. offline-protocol-reliability

**Purpose**: Reliable delivery guarantees.

**Key Components**:
- `AckManager` - Timeout tracking (default 5s)
- `RetryQueue` - Exponential backoff (1s → 2s → 4s, max 30s)
- `Deduplicator` - Message ID tracking (10,000 IDs, 1-hour retention)

**Data Structures**:
- `BinaryHeap` for priority queue (retry queue)
- `HashMap` for O(1) lookups (ACK manager, deduplicator)
- Time-based expiration for memory bounds

**Safety**: `#![deny(unsafe_code)]` - 100% safe Rust

**Dependencies**: `offline-protocol-core`

### 5. offline-protocol-services

**Purpose**: Standalone service discovery and request/response over the mesh.

**Key Components**:
- `MeshServices` - Service registry, discovery query generation/handling, request/response routing
- `ServiceEvent` - Events emitted by service operations (`ServiceDiscovered`, `ServiceRequestReceived`, `ServiceResponseReceived`)
- `ServiceAction` - Return type from message handling (either `NotHandled` or `Consumed` with messages to send and events to emit)

**Design**: All methods return **actions** (messages to send, events to emit) rather than performing I/O directly. Discovery uses gossip flooding with hop-limited forwarding and deduplication.

**Constants**:
- Discovery query dedup TTL: 60 seconds
- Default max hops: 10
- Max gossip fanout per hop: 5
- Max dedup entries: 10,000

**Safety**: `#![deny(unsafe_code)]` - 100% safe Rust

**Dependencies**: `offline-protocol-core`

### 6. offline-protocol

**Purpose**: Main SDK API integrating all components.

**Key Components**:
- `ProtocolConfig` - Unified configuration
- `Event` - All event types
- `OfflineProtocol` - Main entry point
- `FileTransferManager` - File chunking/reassembly

**Thread Safety**:
- `Arc<Mutex<SharedState>>` for shared mutable state
- Event callbacks: `Arc<dyn Fn(Event) + Send + Sync>`

**Safety**: `#![deny(unsafe_code)]` - 100% safe Rust

**Dependencies**: All other crates

### 7. offline-protocol-uniffi

**Purpose:** UniFFI bindings for cross-platform interoperability.

**Features:**
- Type-safe Swift and Kotlin bindings auto-generated from UDL
- Automatic memory management
- Native exception handling
- Complete API surface

**Key Interfaces:**
- `OfflineProtocol` - Main protocol instance (lifecycle, messaging, MLS, routing, transports)
- `MeshServices` - Standalone service discovery and request/response (takes `OfflineProtocol` reference)

**Safety**: Contains `unsafe` code limited to UniFFI scaffolding.
- This is the **ONLY** crate with unsafe code
- All unsafe blocks documented with SAFETY comments

**Dependencies**: `offline-protocol`

## DORS Algorithm

See [DORS Deep Dive](dors.md) for the full scoring system and [DORS Configuration Guide](dors-configuration.md) for tuning parameters.

**Summary**: DORS evaluates each transport using seven weighted factors (signal, proximity, bandwidth, congestion, energy, reliability, capacity), applies hysteresis + cooldown + stability checks to prevent flapping, and supports automatic escalation from BLE to WiFi Direct when performance degrades.

## Relay System

### Promotion Logic

A device is promoted to relay when it has sufficient connections, battery, and relay priority. Relay scoring considers connection count, battery level, charging state, link quality, congestion, and queue depth.

### Load Balancing

Distributes messages across top K relays (default 3):
1. Filter out overloaded relays (congestion > 0.7)
2. Score all remaining relays
3. Select top K by score
4. Forward to all selected relays

## Reliability Layer

### ACK Workflow

```
1. Message sent → Register pending ACK
2. Start timeout timer (default 5s)
3. If ACK received → Emit MessageDelivered event
4. If timeout → Add to retry queue
5. If max retries → Emit MessageFailed event
```

### Retry Backoff

```
Retry 0: Wait 1s
Retry 1: Wait 2s  (1s * 2.0)
Retry 2: Wait 4s  (2s * 2.0)
Retry 3: Wait 8s  (4s * 2.0)
Max: 30s
```

### Deduplication

**Method**: Hash-based tracking
**Capacity**: 10,000 message IDs
**Retention**: 1 hour
**Eviction**: FIFO when at capacity

**Check**:
```
1. On send → Check if duplicate, mark as seen
2. On receive → Check if duplicate, skip if seen
```

## File Transfer

### Chunking

**Process**:
```
1. Calculate chunks: ceil(file_size / chunk_size)
2. For each chunk:
   - Extract data slice
   - Create FileChunk with metadata
   - Include checksum
3. Return Vec<FileChunk>
```

**Default**: 32KB chunks (configurable)

### Reassembly

**Process**:
```
1. Receive chunk → Store by chunk_index
2. Track progress (chunks_received / total_chunks)
3. When complete → Reassemble in order
4. Validate checksum
5. Return complete file
```

**Features**:
- Out-of-order assembly
- Duplicate handling (idempotent)
- Multiple concurrent transfers

## Memory Management

### Rust Core

**Ownership**:
- Messages are moved, not copied (efficient)
- References used wherever possible
- Arc for shared ownership (event callbacks)
- Mutex for shared mutable state

**Lifetimes**:
- Static lifetimes for event callbacks
- Borrowed references in functions (zero-copy)

### FFI Boundary (UniFFI)

The SDK uses [UniFFI](https://mozilla.github.io/uniffi-rs/) to generate type-safe Swift and Kotlin bindings from a UDL (UniFFI Definition Language) file. UniFFI handles memory management, string conversion, and error propagation automatically.

**Pattern**: Define the interface in UDL, UniFFI generates the scaffolding. The generated bindings handle ownership transfer, null safety, and exception bridging.

## Performance Optimizations

1. **Zero-Cost Abstractions**: Iterators, generics compile to same code as manual loops
2. **Inline Functions**: `#[inline]` on hot paths
3. **Link-Time Optimization**: Enabled in release profile
4. **Minimal Allocations**: Reuse buffers, use references
5. **BinaryHeap**: O(log n) priority queue operations
6. **HashMap**: O(1) lookups for ACK/dedup

## Testing Strategy

### Test Coverage (~800 tests)

Tests are distributed across all crates and cover:
- Core types and message construction
- Transport abstraction and metrics
- DORS scoring, hysteresis, escalation
- Reliability (ACK, retry, deduplication)
- MLS encryption lifecycle
- Service discovery and request/response
- Protocol integration and event handling
- UniFFI bindings

Run all tests with `cargo test --workspace`.

## Security Considerations

### Memory Safety

**Guaranteed by Rust** (95% of code):
- No buffer overflows
- No null pointer dereferences
- No data races
- No use-after-free

**Manual Review Required** (5% of code):
- FFI boundary code
- All documented with SAFETY comments
- Defensive programming (null checks, panic catching)

### Input Validation

- All user inputs validated (IDs, TTL, config)
- All C strings checked for null and UTF-8 validity
- All buffer sizes validated
- All pointers validated before dereference

### Encryption

End-to-end encryption is provided via MLS (RFC 9420) with automatic key exchange and session management. See [MLS Integration Guide](mls-integration.md) for details.

