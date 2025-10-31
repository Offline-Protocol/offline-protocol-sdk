# Architecture Deep Dive

This document provides a deep dive into the Offline Protocol SDK architecture.

## Design Philosophy

1. **Safety First**: 95% safe Rust, unsafe isolated to FFI
2. **Write Once**: Core logic shared across all platforms
3. **Zero-Copy**: Minimize allocations and copying
4. **Modular**: Each crate has a single responsibility
5. **Testable**: 110 tests covering all critical paths

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

### 5. offline-protocol

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

### 6. offline-protocol-ffi

**Purpose**: C FFI bindings for cross-platform use.

**Key Functions**:
- `offline_protocol_create/destroy` - Memory management
- `offline_protocol_start/stop` - Lifecycle
- `offline_protocol_send_message` - Messaging

**Safety Patterns**:
- Pointer validation (null checks)
- Panic catching (`catch_unwind`)
- SAFETY comments on every `unsafe` block
- Error codes instead of exceptions

**Safety**: ⚠️ Contains `unsafe` code (~5% of codebase)
- This is the **ONLY** crate with unsafe code
- All unsafe blocks documented
- Defensive programming at language boundaries

**Dependencies**: `offline-protocol`

## DORS Algorithm

### Transport Selection

**Input**:
- Message to send
- Available transports with metrics
- Current transport state

**Process**:
1. Calculate score for each transport
2. Check hysteresis threshold
3. Check cooldown period
4. Check stability window
5. Select best transport

**Scoring**:
```
BLE Score = (signal * 0.3) + (energy * 0.3) + (congestion * 0.2) + (proximity * 0.2)
WiFi Score = (bandwidth * 0.4) + (proximity * 0.3) + (congestion * 0.3)
Internet Score = 100 (if prefer_online) or 0
```

### Escalation Triggers

**BLE → Wi-Fi Direct**:
- Retry failures ≥ 2
- RSSI < -85 dBm for 10+ seconds
- Queue depth > 50 messages
- TTL exhaustion (message dying)
- Hop count increasing without delivery

**Wi-Fi Direct → BLE**:
- BLE recovered (RSSI > -70 dBm for 15+ seconds)
- Wi-Fi setup time > 8 seconds
- Battery < 20%
- Last 3 messages via BLE successful

## Relay System

### Promotion Logic

**Conditions**:
```
should_promote = 
    connections >= threshold AND
    battery >= min_battery AND
    (battery >= 15% OR charging) AND
    relay_priority != Never
```

**Scoring**:
```
relay_score = 
    (connections/10 * 30) +      // Connection factor (0-30)
    (battery/100 * 20) +         // Battery factor (0-20)
    (charging ? 20 : 0) +        // Charging bonus (20)
    (link_quality/100 * 20) +    // Link quality (0-20)
    -(congestion * 15) +         // Congestion penalty (0-15)
    -(queue_depth/50 * 15)       // Queue penalty (0-15)
```

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

### FFI Boundary

**Pattern**:
```
Rust → C:
    Box::into_raw(Box::new(object))  // Transfer ownership

C → Rust:
    &mut *(handle as *mut T)         // Borrow

C cleanup:
    Box::from_raw(handle)            // Reclaim ownership
```

**String Handling**:
```
Rust → C:
    CString::into_raw()              // Allocate

C → Rust:
    CStr::from_ptr().to_str()        // Borrow

Cleanup:
    CString::from_raw()              // Free
```

## Performance Optimizations

1. **Zero-Cost Abstractions**: Iterators, generics compile to same code as manual loops
2. **Inline Functions**: `#[inline]` on hot paths
3. **Link-Time Optimization**: Enabled in release profile
4. **Minimal Allocations**: Reuse buffers, use references
5. **BinaryHeap**: O(log n) priority queue operations
6. **HashMap**: O(1) lookups for ACK/dedup

## Testing Strategy

### Unit Tests (110 total)

- **Core types**: 12 tests
- **Transport**: 3 tests  
- **Router/DORS**: 20 tests
- **Reliability**: 22 tests
- **Protocol**: 41 tests
- **FFI**: 12 tests

### Integration Tests

*(To be added)*

Planned scenarios:
- Multi-device message flow
- Transport switching
- Network partitions
- Congestion handling
- File transfers

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

### Future Enhancements

- Message encryption (E2E)
- Authentication (message signatures)
- Rate limiting
- DoS protection

