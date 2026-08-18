# Architecture Deep Dive

This document provides a deep dive into the Offline Protocol SDK architecture.

## Design Philosophy

1. **Safety First**: 95% safe Rust, unsafe isolated to FFI
2. **Write Once**: Core logic shared across all platforms
3. **Zero-Copy**: Minimize allocations and copying
4. **Modular**: Each crate has a single responsibility
5. **Testable**: ~1,700 tests covering all critical paths

## Crate Organization

### 1. offline-protocol-core

**Purpose**: Fundamental types with zero dependencies.

**Key Types**:
- `Message`, `MessageId`, `MessagePriority`
- `Address`, `AddressError` — the self-certifying `off1…` identity (bech32m
  over a 20-byte hash of the Ed25519 identity key). A peer's address is
  verified by re-deriving it from the key its owner presents, which is what
  makes identity unforgeable without a trust store.
- `UserId`, `AppId`, `TTL`, `HopCount`, `Timestamp` — note `UserId` is the
  transport-level wrapper that *carries* an address on the wire; it validates
  charset and length only, so it is not itself proof of identity.
- `Error` type with `thiserror`

**Safety**: `#![deny(unsafe_code)]` - 100% safe Rust

**Dependencies**: `serde`, `uuid`, `chrono`

### 2. offline-protocol-transport

**Purpose**: Transport abstraction layer.

**Key Components**:
- `Transport` trait (send, receive, status, metrics)
- `TransportType` enum (BLE, WiFi Direct, Internet, Reticulum, Nostr)
- `TransportMetrics` for monitoring
- `MockTransport` for consumers that explicitly enable the `test-utils` feature
- Shared `common` module for cross-transport helper functions

**Safety**: `#![deny(unsafe_code)]` - 100% safe Rust

**Dependencies**: `offline-protocol-core`

### 3. offline-protocol-router

**Purpose**: DORS engine and routing logic.

**Key Components**:
- `TransportSelector` - DORS algorithm with multi-factor scoring
- `RelayConfig` / `RelayRole` - Whether this device forwards for others; the
  standing is decided by the engine's forwarding governor from traffic actually
  carried, not predicted from thresholds

**Algorithms**:
- Transport scoring: Signal + Proximity + Bandwidth + Congestion + Energy + Reliability + Load
- Per-transport scoring profiles (BLE, WiFi Direct, Internet, Reticulum, Nostr)
- Hysteresis prevents flapping (15-point threshold)
- Cooldown timer (20 seconds)
- Stability window (8 seconds)

**Safety**: `#![deny(unsafe_code)]` - 100% safe Rust

**Dependencies**: `offline-protocol-core`, `offline-protocol-transport`

### 4. offline-protocol-reliability

**Purpose**: Reliable delivery guarantees.

**Key Components**:
- `AckManager` - Timeout tracking (default 10s)
- `RetryQueue` - Exponential backoff (1s → 2s → 4s …, capped at 5 min; 10 retries)
- `Deduplicator` - Message ID tracking (1,000 IDs, 1-hour retention)

**Data Structures**:
- `BinaryHeap` for priority queue (retry queue)
- `HashMap` for O(1) lookups (ACK manager, deduplicator)
- Time-based expiration for memory bounds

**Safety**: `#![deny(unsafe_code)]` - 100% safe Rust

**Dependencies**: `offline-protocol-core`

### 5. offline-protocol-mls

**Purpose**: End-to-end encryption for one-to-one and group messaging using MLS (RFC 9420).

**Key Components**:
- `MlsManager` - Session, group, key-package, encryption, and decryption lifecycle
- `MlsStorage` - Storage-agnostic interface for platform **secure** storage (credential store)
- `GroupId`, `GroupInfo`, `EncryptedMessage`, and `WelcomeMessage`

**Safety**: `#![deny(unsafe_code)]` - 100% safe Rust

**Dependencies**: `offline-protocol-core`, OpenMLS

### 6. offline-protocol-services

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

### 7. offline-protocol

**Purpose**: Main SDK API integrating all components.

**Key Components**:
- `ProtocolConfig` - Unified configuration
- `Event` - All event types
- `OfflineProtocol` - Main entry point
- `ProtocolStateStorage` - App-container storage for restartable delivery state (outbox, pending messages, session/Welcome lifecycles, peer snapshots, media descriptors, block list, Lamport clock)
- `FileTransferManager` - File chunking/reassembly

**Storage domains**: key material and restartable protocol state are two
different contracts with two different lifecycles. `MlsStorage` (in the MLS
crate) is credential-backed and may outlive an app container;
`ProtocolStateStorage` must live *in* the container and be removed on app
deletion. Sensitive protocol-state record values are sealed with a per-install
AEAD key held in secure storage before they reach the state provider. See
[MLS Integration](mls-integration.md#custom-storage-advanced).

**Thread Safety**:
- `Arc<Mutex<SharedState>>` for shared mutable state
- Event callbacks: `Arc<dyn Fn(Event) + Send + Sync>`

**Safety**: `#![deny(unsafe_code)]` - 100% safe Rust

**Dependencies**: All other crates

### 8. offline-protocol-uniffi

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

**Summary**: DORS evaluates each transport (BLE, WiFi Direct, Internet, Reticulum, Nostr) using seven weighted factors (signal, proximity, bandwidth, congestion, energy, reliability, capacity), applies hysteresis + cooldown + stability checks to prevent flapping, and supports automatic escalation from BLE to WiFi Direct when performance degrades. Reticulum and Nostr are scored as fallbacks with the lowest tie-break priorities (Reticulum for off-grid resilience, Nostr for censorship-resistant routing).

## Relay System

### Promotion Logic

A device is promoted to relay when it has sufficient connections, battery, and relay priority — and only if its configuration allows relaying at all (`allowRelay` must be true and the relay priority must not be `never`; a device whose config forbids relaying is also demoted if it somehow holds the role). Relay scoring considers connection count, battery level, charging state, link quality, congestion, and queue depth.

### Load Balancing

Distributes messages across top K relays (default 3):
1. Filter out overloaded relays (congestion above the configured `maxCongestionLevel`, default 0.7)
2. Score all remaining relays
3. Select top K by score
4. Forward to all selected relays

## Reliability Layer

### ACK Workflow

```
1. Message sent → Register pending ACK
2. Start timeout timer (default 10s)
3. If ACK received → Emit MessageDelivered event
4. If timeout → Add to retry queue
5. If max retries → Emit MessageFailed event
```

### Retry Backoff

```
Retry 0: Wait 1s
Retry 1: Wait 2s   (1s * 2.0)
Retry 2: Wait 4s   (2s * 2.0)
Retry 3: Wait 8s   (4s * 2.0)
Retry 4: Wait 16s  (8s * 2.0)
Retry 5: Wait 32s  (16s * 2.0)
Retry 6: Wait 64s  (32s * 2.0)
Retry 7: Wait 128s (64s * 2.0)
Retry 8: Wait 256s (128s * 2.0)
Retry 9: Wait 300s (maximum-delay clamp, 5 min)
```

See [Message Delivery](message-delivery.md#exponential-backoff) for the
clamping semantics and how to tune the ladder.

### Deduplication

**Method**: Hash-based tracking
**Capacity**: 1,000 message IDs
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

### Test Coverage (~1,700 tests)

Tests are distributed across all crates and cover:
- Core types and message construction
- Transport abstraction and metrics
- DORS scoring, hysteresis, escalation
- Reliability (ACK, retry, deduplication)
- MLS encryption lifecycle, crypto-desync recovery
- Storage split: restore walks, delete budgets, legacy adoption, record sealing
- Service discovery and request/response
- Protocol integration and event handling
- UniFFI bindings

Several are **drift guards** rather than behavioural tests — they read another
file and assert it still agrees with the Rust source (React Native event types
against the `Event` enum, the built-in storage providers' transfer ceilings
against the Rust constant, the RN bridges' config fallbacks against the
reliability defaults). Those are the tests that fail when documentation-adjacent
code goes stale.

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
