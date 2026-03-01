# API Reference

Complete API documentation for the Offline Protocol SDK.

## Core Types

### MessagePriority

Message priority levels affecting delivery order and retry behavior.

```rust
pub enum MessagePriority {
    Low,      // Can be delayed or dropped under congestion
    Medium,   // Default priority
    High,     // Delivered quickly
    Critical, // Emergency messages, highest guarantee
}
```

### TransportType

Available transport mechanisms.

```rust
pub enum TransportType {
    Internet,    // Online connectivity (unlimited range)
    BLE,         // Bluetooth Low Energy mesh (50-100m)
    WiFiDirect,  // Wi-Fi Direct (100-200m, Android only)
}
```

### Message

Core message structure.

```rust
pub struct Message {
    pub id: MessageId,              // Unique identifier
    pub sender: UserId,             // Sender's user ID
    pub recipient: UserId,          // Recipient's user ID
    pub app_id: AppId,              // Application identifier
    pub priority: MessagePriority,  // Message priority
    pub ttl: TTL,                   // Time-to-live (hops remaining)
    pub hop_count: HopCount,        // Hops traversed
    pub timestamp: Timestamp,       // When message was created
    pub content: String,            // Message content
    pub metadata: HashMap<String, String>,  // App-specific data
    pub requires_ack: bool,         // Whether ACK is required
}
```

## Configuration

### ProtocolConfig

Main configuration structure.

```rust
pub struct ProtocolConfig {
    pub app_id: String,             // Application ID (required)
    pub user_id: String,            // User ID (required)
    pub transport: TransportConfig, // Transport settings
    pub encryption: EncryptionConfig, // Encryption settings (NEW!)
    pub dors: DorsConfig,          // DORS settings
    pub relay: RelayConfig,        // Relay settings
    pub path: PathConfig,          // Path selection settings
    pub reliability: ReliabilityConfig, // Reliability settings
    pub initial_ttl: u8,           // Initial TTL (default: 8)
}
```

**Builder API**:
```rust
let config = ProtocolConfig::builder("my-app", "user123")
    .ble_enabled(true)
    .wifi_direct_enabled(true)
    .online_first(false)
    .initial_ttl(10)
    .encryption_enabled(true)      // NEW: Auto-encryption
    .auto_key_exchange(true)       // NEW: Auto key exchange
    .store_pending_messages(true)  // NEW: Queue pending messages
    .require_encryption(false)     // NEW: Strict encryption policy
    .build()?;
```

### EncryptionConfig

Configuration for automatic MLS end-to-end encryption.

```rust
pub struct EncryptionConfig {
    /// Enable automatic encryption/decryption (default: true)
    pub enabled: bool,
    
    /// Auto-exchange key packages on peer discovery (default: true)
    pub auto_key_exchange: bool,
    
    /// Store pending messages when no session exists (default: true)
    pub store_pending: bool,

    /// Require encryption for outbound sends (default: false)
    pub require_encryption: bool,

    /// Bounds and eviction policy for encrypted messages received
    /// before session readiness.
    pub pending_queue: PendingQueueConfig,
}
```

**TypeScript**:
```typescript
interface EncryptionConfig {
  enabled?: boolean;        // Default: true
  autoKeyExchange?: boolean; // Default: true
  storePending?: boolean;    // Default: true
  requireEncryption?: boolean; // Default: false
  pendingQueue?: {
    maxPendingPerPeer?: number; // Default: 64
    maxPendingGlobal?: number;  // Default: 4096
    pendingTtlMs?: number;      // Default: 120000
    overflowPolicy?: 'drop_oldest' | 'drop_newest'; // Default: drop_oldest
  };
}
```

### DorsConfig

DORS (transport selection) configuration.

```rust
pub struct DorsConfig {
    pub switch_hysteresis: f32,        // Min score improvement to switch (default: 15.0)
    pub switch_cooldown_secs: u64,     // Wait after switching (default: 20)
    pub ble_to_wifi_retry_threshold: u32,  // Retries before escalation (default: 2)
    pub rssi_switch_threshold: i16,    // RSSI for switching (default: -85 dBm)
    pub congestion_queue_threshold: usize, // Queue depth limit (default: 50)
    pub stability_window_secs: u64,    // Stability check window (default: 8)
    pub poor_signal_duration_secs: u64, // Seconds RSSI must remain poor before escalating (default: 10)
    pub ttl_escalation_threshold: u8,   // TTL value considered near exhaustion (default: 2)
    pub congestion_duration_secs: u64,  // Seconds congestion must persist before escalating (default: 10)
    pub ttl_escalation_hold_secs: u64,  // Seconds to keep TTL alarm active (default: 20)
    pub history_window_size: usize,     // Samples to retain for scoring history (default: 10)
    pub queue_recovery_ratio: f32,      // Queue ratio that clears congestion flag (default: 0.5)
    pub prefer_online: bool,           // Online-first mode (default: false)
}
```

### RelayConfig

Relay management configuration.

```rust
pub struct RelayConfig {
    pub relay_threshold: usize,        // Min connections to be relay (default: 3)
    pub min_battery_for_relay: u8,     // Min battery % (default: 30)
    pub allow_relay: bool,             // Allow relay role (default: true)
    pub relay_priority: RelayPriority, // Auto/Always/Never (default: Auto)
}
```

### ReliabilityConfig

Reliability layer configuration.

```rust
pub struct ReliabilityConfig {
    pub ack: AckConfig,      // ACK timeout: 5000ms, max pending: 1000
    pub retry: RetryConfig,  // Max retries: 3, backoff: 2.0x
    pub dedup: DeduplicatorConfig, // Max tracked: 10000, retention: 1 hour
}
```

## Main API

### OfflineProtocol

Main protocol class.

#### Constructor

```rust
pub fn new(config: ProtocolConfig) -> Result<Self>
```

Creates a new protocol instance. Validates configuration.

#### Lifecycle Methods

```rust
pub fn start(&mut self) -> Result<()>
```

Starts the protocol. Initializes transports and begins operation.

```rust
pub fn stop(&mut self) -> Result<()>
```

Stops the protocol gracefully. Shuts down all transports.

```rust
pub fn pause(&mut self) -> Result<()>
```

Pauses the protocol (for background mode). Reduces battery usage.

```rust
pub fn resume(&mut self) -> Result<()>
```

Resumes from paused state.

#### Messaging

```rust
pub fn send_message(
    &mut self,
    recipient: impl Into<String>,
    content: impl Into<String>,
    priority: Option<MessagePriority>,
    reply_to_msg: Option<impl Into<String>>,
) -> Result<MessageId>
```

Sends a message. Returns `Ok(message_id)` when the message is accepted for send
or queued for retry/pending-session delivery, or `Err` for policy and setup failures.

**Example**:
```rust
match protocol.send_message("user456", "Hello!", Some(MessagePriority::High), None::<String>) {
    Ok(msg_id) => println!("Sent: {msg_id}"),
    Err(e) => println!("Deferred for retry: {e}"),
}
```

```rust
pub fn receive_message(&mut self) -> Option<Message>
```

Polls for the next received message (non-blocking).

#### Event Handling

```rust
pub fn on_event<F>(&mut self, handler: F)
where F: Fn(Event) + Send + Sync + 'static
```

Registers an event handler.

**Example**:
```rust
protocol.on_event(|event| {
    match event {
        Event::MessageDelivered { message_id, latency_ms, .. } => {
            println!("Delivered {} in {}ms", message_id, latency_ms);
        }
        _ => {}
    }
});
```

#### Encryption (Auto-Encryption)

```rust
pub fn initialize_mls(&mut self, storage: Arc<dyn MlsStorage>) -> Result<()>
```

Initializes MLS encryption with the provided storage backend. Required before encryption can be used.

```rust
pub fn is_mls_initialized(&self) -> bool
```

Returns whether MLS encryption is initialized.

```rust
pub fn on_neighbor_discovered(&mut self, peer_id: &str)
```

Called when a neighbor is discovered. When `auto_key_exchange` is enabled, automatically sends our key package to the new peer.

```rust
pub fn on_neighbor_lost(&mut self, peer_id: &str)
```

Called when a neighbor is lost. Cleans up tracking state.

**Note**: When `encryption.enabled` is `true` (default), `send_message` automatically:
1. Encrypts content if MLS is initialized and a session exists
2. Creates a session and sends Welcome if we have the recipient's key package
3. Queues the message if `store_pending` is `true` and no session/key package exists

With default settings, encryption is best-effort. Set `encryption.require_encryption = true`
to guarantee encrypted delivery or fail with a typed error (`SessionNotReady`
or `EncryptFailed`).

In strict mode, send failures are fail-fast and do not transmit transport payloads.
Connection-control APIs that require plaintext bootstrap messages are rejected while
`encryption.require_encryption = true`.

Rust migration note:
- `offline_protocol::Error` includes strict-mode variants (`SessionNotReady`, `EncryptFailed`).
- `SessionNotReady` includes establishment progress (`NoKeyPackage`, `HaveKeyPackage`, `SessionPending`, `SessionConfirmed`).
- exhaustive `match` blocks over `Error` should add these variants before upgrading.

Similarly, `receive_message` automatically:
1. Handles incoming key packages (stores them for session creation)
2. Handles incoming Welcome messages (joins the session)
3. Decrypts encrypted messages

**TypeScript Example**:
```typescript
// Initialize encryption (required once after start)
await protocol.initializeMlsWithSecureStorage();

// Messages are automatically encrypted/decrypted!
await protocol.sendMessage({ recipient: 'bob', content: 'Hello!' });

protocol.on('message_received', (event) => {
  console.log(event.content);    // Already decrypted
  console.log(event.encrypted);  // true if was encrypted
});
```

#### Background Processing

```rust
pub fn process(&mut self) -> Result<()>
```

Processes pending operations (retries, timeouts). Call periodically.

## Events

### Event Types

All events are serializable to JSON for cross-language use.

#### MessageSent

```rust
MessageSent {
    message_id: String,
    timestamp: i64,
}
```

Emitted when a message is queued for sending.

#### MessageReceived

```rust
MessageReceived {
    message_id: String,
    sender: String,
    recipient: String,
    content: String,
    hop_count: u8,
    transport: String,
    timestamp: i64,
}
```

Emitted when a message is received.

#### MessageDelivered

```rust
MessageDelivered {
    message_id: String,
    latency_ms: u64,
    hop_count: u8,
    transport: String,
}
```

Emitted when ACK is received (successful delivery).

#### MessageFailed

```rust
MessageFailed {
    message_id: String,
    reason: String,
    retry_count: u32,
}
```

Emitted when max retries exceeded.

#### TransportSwitched

```rust
TransportSwitched {
    from: Option<String>,
    to: String,
    reason: String,
}
```

Emitted when DORS switches transport.

#### RelayPromoted / RelayDemoted

```rust
RelayPromoted {
    connection_count: usize,
    battery_level: u8,
}

RelayDemoted {
    reason: String,
}
```

Emitted when relay status changes.

#### NetworkMetrics

```rust
NetworkMetrics {
    neighbor_count: usize,
    relay_count: usize,
    delivery_ratio: f32,  // 0.0-1.0
    avg_latency_ms: u64,
}
```

Periodic network statistics.

## File Transfer

### FileTransferManager

Handles file chunking and reassembly.

```rust
pub fn chunk_file(
    &self,
    file_id: String,
    file_name: String,
    file_data: Vec<u8>,
) -> Result<Vec<FileChunk>>
```

Chunks a file for sending (default 32KB chunks).

```rust
pub fn process_chunk(&mut self, chunk: FileChunk) -> Option<FileProgress>
```

Processes a received chunk. Returns progress.

```rust
pub fn finalize_file(&mut self, file_id: &str) -> Option<Vec<u8>>
```

Returns complete file data when all chunks received.

### FileChunk

```rust
pub struct FileChunk {
    pub file_id: String,
    pub file_name: String,
    pub file_size: u64,
    pub total_chunks: u32,
    pub chunk_index: u32,
    pub chunk_data: Vec<u8>,
    pub file_checksum: String,
}
```

### FileProgress

```rust
pub struct FileProgress {
    pub file_id: String,
    pub file_name: String,
    pub file_size: u64,
    pub chunks_completed: u32,
    pub total_chunks: u32,
    pub percentage: u8,  // 0-100
}
```

## FFI API (C)

For native platform integration.

### Functions

```c
// Lifecycle
OfflineProtocol* offline_protocol_create(const char* config_json);
void offline_protocol_destroy(OfflineProtocol* handle);
int32_t offline_protocol_start(OfflineProtocol* handle);
int32_t offline_protocol_stop(OfflineProtocol* handle);

// Messaging
int32_t offline_protocol_send_message(
    OfflineProtocol* handle,
    const char* recipient,
    const char* content,
    int32_t priority,
    char* out_message_id,
    uintptr_t out_len
);

// Events
int32_t offline_protocol_poll_event(
    OfflineProtocol* handle,
    char* out_event_json,
    uintptr_t out_len
);

// Memory
void offline_protocol_free_string(char* s);
```

### Error Codes

```c
#define SUCCESS 0
#define ERROR_NULL_POINTER -1
#define ERROR_INVALID_UTF8 -2
#define ERROR_NOT_STARTED -3
#define ERROR_ALREADY_STARTED -4
#define ERROR_SEND_FAILED -5
#define ERROR_PANIC -99
#define ERROR_OTHER -100
```

## Platform-Specific APIs

See platform-specific documentation:
- [React Native API](../bindings/react-native/README.md)
- [Android Integration](android-integration.md)
- [iOS Integration](ios-integration.md)
- [Web/WASM API](../bindings/web/README.md)

