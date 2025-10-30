# Offline Protocol SDK Implementation Plan

## Project Structure

Rust workspace with core crates:

- `offline-protocol-core`: Message types, routing, core protocol logic
- `offline-protocol-transport`: Transport trait and implementations (BLE, Wi-Fi Direct)
- `offline-protocol-router`: DORS engine, relay management, path selection
- `offline-protocol-reliability`: ACK handling, retry queues, deduplication  
- `offline-protocol-ffi`: C-compatible FFI layer for platform bindings
- `offline-protocol`: Main library crate tying everything together

Bindings structure:

- `bindings/typescript`: React Native npm package (@offlineprotocol/sdk)
- `bindings/ios`: CocoaPods package with Swift wrappers
- `bindings/android`: Gradle package with Kotlin/Java wrappers

## Phase 1: Core Protocol Foundation

### 1.1 Message Types and Serialization

- Define message structures: `TextMessage`, `FileMessage`, `ControlMessage`
- Message envelope with: `messageId`, `senderId`, `recipientId`, `ttl`, `hopCount`, `priority`, `timestamp`, `metadata`
- Use `serde` with MessagePack for efficient serialization
- Support priority levels: `low`, `medium`, `high`

### 1.2 Transport Abstraction

- `Transport` trait with async methods:
- `start()`, `stop()`, `pause()`, `resume()`
- `send(message)`, `receive()` → stream
- `get_neighbors()`, `get_link_quality(neighbor)`
- `TransportType` enum: `BLE`, `WiFiDirect`  
- `TransportMetrics`: RSSI, latency, delivery ratio, available bandwidth

### 1.3 Neighbor Discovery

- Neighbor table: device ID, username, role (peer/relay), link quality, last seen
- Periodic beacon broadcasting (every 5s)
- Neighbor timeout (30s without beacon)
- Link quality estimation based on RSSI and packet loss

## Phase 2: Transport Implementations

### 2.1 BLE Mesh Transport

- Use `btleplug` crate for cross-platform BLE
- Custom GATT service UUID for offline protocol
- Characteristics: message TX, message RX, beacon
- BLE advertising with service UUID and device info
- Scanning for neighbors, automatic connection management
- Handle BLE MTU negotiation (aim for 512 bytes)
- Mesh topology: multiple simultaneous connections

### 2.2 Wi-Fi Direct Transport

- Platform-specific native implementations
- Android: JNI wrapper for Wi-Fi P2P APIs
- Group owner negotiation (prefer device with more battery/connections)
- Socket-based communication (TCP for reliability)
- Service discovery for finding other devices
- Handle connection state changes and reconnection

### 2.3 Mock Transport

- In-memory transport for testing
- Configurable latency, packet loss, bandwidth
- Simulated network topology
- Multi-node simulation capability

## Phase 3: Routing Layer (DORS)

### 3.1 Path Selection Engine (BLE → Wi-Fi Direct Escalation)

- **Primary**: Always try BLE first
- **Escalation triggers**:
- BLE retry count exceeds `bleToWiFiRetryThreshold` (default: 2)
- RSSI below `rssiSwitchThreshold` (default: -85 dBm)
- BLE delivery ratio < 50% over last 10 messages
- **Hysteresis**: Wait `switchHysteresis` seconds before switching back
- **Cooldown**: `switchCooldown` seconds after each switch
- Track per-neighbor transport performance
- Emit `transport:switched` events

### 3.2 Relay Manager

- **Promotion criteria**:
- Connection count ≥ `relayThreshold` (default: 3)
- Battery ≥ `minBatteryForRelay` (default: 30%)
- `allowActAsRelay` enabled
- `relayPriority` is 'auto' or 'always'
- **Demotion triggers**:
- Connection count drops below threshold
- Battery below minimum
- User disables relay
- Advertise relay capability in beacons
- Message forwarding logic for relays
- Emit `relay:promoted` and `relay:demoted` events

### 3.3 Multi-hop Routing

- Flooding-based routing with TTL
- Duplicate detection using LRU cache of message IDs (capacity: 1000)
- Decrease TTL on each hop, drop at TTL=0
- Track hop count for metrics
- Prefer shorter paths when multiple routes available

## Phase 4: Reliability Layer

### 4.1 ACK Manager

- Track pending messages waiting for ACK
- ACK timeout (default: 10s, configurable)
- Trigger retry on timeout
- Emit `message:delivered` on ACK receipt
- Include hop count and latency in delivery event

### 4.2 Retry Queue  

- Exponential backoff: 1s, 2s, 4s, 8s, 16s...
- Max retries (default: 3, configurable)
- Priority queue (high → medium → low)
- Persistent outbox using SQLite or file storage
- Messages survive app restarts
- Max lifetime (default: 1 hour, configurable)
- Emit `message:failed` after max retries

### 4.3 Deduplicator

- LRU cache of recently seen message IDs
- Time-based expiry (5 minutes)
- Drop duplicate messages silently
- Prevent forwarding loops

## Phase 5: File Transfer and Fragmentation

### 5.1 File Chunking

- Automatic fragmentation for files > MTU
- Chunk sizes: BLE (512 bytes), Wi-Fi Direct (8KB)
- Chunk metadata: file ID, total chunks, chunk index, checksum
- Base64 encoding for binary data

### 5.2 File Reassembly

- Track incoming chunks per file ID
- Request missing chunks (selective retransmission)
- Verify checksums per chunk and overall file
- Assemble complete file and emit `file:received` event

### 5.3 Progress Tracking

- Progress callback: `{ percentage, bytesSent, totalBytes, currentChunk, totalChunks }`
- Update after each chunk ACK
- Cancellation support

## Phase 6: SDK Public API

### 6.1 Main OfflineProtocol Interface

Methods:

- `new(config)`: Initialize with configuration object
- `start()`: Start all transports and routing engine
- `stop()`: Gracefully shutdown
- `pause()`: Reduce operations for background (stop scanning, reduce beacons)
- `resume()`: Resume full operations
- `cleanup()`: Final cleanup and deallocation
- `sendMessage({ recipient, text, priority, metadata })`: Send text message
- `sendFile({ recipient, file, priority, onProgress })`: Send file with progress
- `setTransportSelector(callback)`: Custom transport selection logic
- `checkPermissions()`: Returns `{ bluetooth, location, wifiDirect, notifications }`
- `requestPermission(type)`: Request specific permission
- `on(event, handler)`: Register event listener

### 6.2 Configuration Object

Structure:

- `appId`: String (application identifier)
- `username`: String (user identifier)
- `transports.ble.enabled`: Boolean (default: true)
- `transports.ble.scanInterval`: Number (default: 5000ms)
- `transports.wifiDirect.enabled`: Boolean (default: true)
- `transports.wifiDirect.autoSwitch`: Boolean (default: true)
- `network.relayThreshold`: Number (default: 3)
- `network.initialTTL`: Number (default: 8)
- `network.enableDORS`: Boolean (default: true)
- `dors.autoSwitch`: Boolean (default: true)
- `dors.switchHysteresis`: Number seconds (default: 15)
- `dors.switchCooldown`: Number seconds (default: 20)
- `dors.bleToWiFiRetryThreshold`: Number (default: 2)
- `dors.rssiSwitchThreshold`: Number dBm (default: -85)
- `relay.allowActAsRelay`: Boolean (default: true)
- `relay.relayPriority`: 'auto' | 'always' | 'never' (default: 'auto')
- `relay.minBatteryForRelay`: Number percent (default: 30)
- `reliability.maxRetries`: Number (default: 3)
- `reliability.ackTimeout`: Number ms (default: 10000)
- `reliability.outboxMaxLifetime`: Number ms (default: 3600000)

### 6.3 Events

- `message:received`: `{ messageId, senderUsername, text, metadata, timestamp }`
- `message:delivered`: `{ messageId, hopCount, latency, transport }`
- `message:failed`: `{ messageId, reason }`
- `file:received`: `{ messageId, senderUsername, file: { name, size, mimeType, data }, timestamp }`
- `relay:promoted`: `{ connectionCount, timestamp }`
- `relay:demoted`: `{ reason, timestamp }`
- `transport:switched`: `{ from, to, reason, timestamp }`
- `neighbor:discovered`: `{ username, deviceId, role, linkQuality, rssi }`
- `neighbor:lost`: `{ username, deviceId }`
- `network:metrics`: `{ neighborCount, relayCount, deliveryRatio, avgLatency }`

### 6.4 Custom Transport Selector

Callback signature: `(message, availableTransports, metrics) => TransportType | null`

- Return `null` to use DORS default logic
- Return specific transport to override
- Access to message priority, type, size, metadata
- Access to current metrics for each transport

### 6.5 Permission Management

- `checkPermissions()`: Check current status of all required permissions
- `requestPermission(type)`: Trigger native permission dialog
- Types: 'bluetooth', 'location', 'wifiDirect', 'notifications'
- Platform-specific handling (iOS vs Android)
- Automatic monitoring of permission changes

## Phase 7: FFI Layer

### 7.1 C-Compatible API

- Use `#[no_mangle]` and `extern "C"` for functions
- Opaque pointers for Rust objects (`*mut OfflineProtocol`)
- C-string conversion (`CString`, `CStr`)
- Error codes instead of Result types
- Catch panics at FFI boundary using `std::panic::catch_unwind`
- Generate C headers with `cbindgen`

### 7.2 Callback Bridge

- C function pointers for callbacks
- Store callbacks in Rust, invoke from event loop
- Thread-safe callback invocation using channels
- Convert Rust events to C-compatible structs

### 7.3 Memory Management

- `protocol_new(config)` → `*mut OfflineProtocol`
- `protocol_free(protocol)` → deallocate
- String ownership: caller must free returned strings
- Clear documentation of ownership semantics

## Phase 8: Platform Bindings

### 8.1 TypeScript/React Native Bindings

- Use `napi-rs` for Node.js native module
- TypeScript class wrapping Rust FFI
- Extend `EventEmitter` for event handling
- Promise-based async methods
- Generate `.d.ts` type definitions
- npm package: `@offlineprotocol/sdk`
- Pre-built binaries for major platforms
- Example: Matches the API you provided exactly

### 8.2 iOS Bindings

- Build universal static library (arm64 for device, x86_64 for simulator)
- Swift class wrapping C FFI
- Protocol/delegate pattern for events
- CocoaPods podspec: `OfflineProtocolSDK.podspec`
- Handle iOS permissions: Bluetooth, Local Network
- Background mode configuration guidance

### 8.3 Android Bindings

- JNI bindings using `jni` crate
- Kotlin wrapper class
- Listener interfaces for events
- Build AAR package
- Gradle: `com.offlineprotocol:sdk:1.0.0`
- Handle Android permissions: Bluetooth, Location, Nearby Devices (API 31+)
- Service for background operation

## Phase 9: Testing

### 9.1 Unit Tests

- Test message serialization/deserialization
- Test ACK manager, retry queue, deduplicator in isolation
- Test neighbor table operations
- Test DORS switching logic with mocked metrics
- Test relay promotion/demotion logic

### 9.2 Integration Tests

- Multi-node simulation with mock transport
- Test message delivery across 1-5 hops
- Test file transfer with fragmentation
- Test transport failover (BLE → Wi-Fi Direct)
- Test relay behavior under load
- Test permission handling

### 9.3 Example Applications

- React Native chat app demonstrating all features
- iOS Swift example app
- Android Kotlin example app
- Emergency responder example (offline-only, aggressive relay)

## Phase 10: Documentation and Deployment

### 10.1 Documentation

- README with installation, quick start, features
- Rustdoc for all Rust crates
- TSDoc comments for TypeScript bindings
- Platform-specific guides (iOS, Android, React Native)
- Architecture document explaining DORS algorithm
- Troubleshooting guide (permissions, BLE issues, etc)

### 10.2 CI/CD

- GitHub Actions workflows:
- Rust: `cargo test`, `cargo clippy`, `cargo fmt --check`
- Build binaries for Linux, macOS, Windows
- Build iOS framework and Android AAR
- Run integration tests
- Publish npm package
- Publish to CocoaPods trunk
- Publish to Maven Central

### 10.3 Packaging

- Cargo: Publish crates to crates.io
- npm: Publish to npm registry
- CocoaPods: `pod trunk push`
- Maven: Publish to Maven Central or JitPack

## Key Technologies

- **Rust**: Edition 2021, async with `tokio`
- **BLE**: `btleplug` for cross-platform support
- **Serialization**: `serde` + `rmp-serde` (MessagePack)
- **FFI**: `cbindgen` for headers
- **Node**: `napi-rs` for TypeScript bindings
- **Android**: `jni` crate for JNI bindings
- **iOS**: C FFI with Swift wrappers
- **Storage**: `sled` or SQLite for persistent outbox
- **Testing**: `tokio-test`, `criterion` for benchmarks

## Critical Considerations

1. **BLE Limitations**: 100-200 Kbps throughput, 10-50m range, 512 byte MTU typical
2. **Wi-Fi Direct**: Android-only, complex group owner negotiation, requires location permission
3. **Battery Impact**: Continuous BLE scanning drains battery, implement pause/resume carefully
4. **Permissions**: Runtime permissions on mobile, handle denials gracefully
5. **Testing**: Requires physical devices for realistic BLE/Wi-Fi testing
6. **Thread Safety**: All public APIs must be thread-safe (`Send + Sync`)
7. **Error Handling**: Never panic in public API, use `Result` types, catch panics at FFI boundary