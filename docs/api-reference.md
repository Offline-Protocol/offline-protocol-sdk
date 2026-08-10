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
    Reticulum,   // Reticulum mesh (LoRa, TCP, UDP, serial, I2P)
    Nostr,       // Nostr relay (censorship-resistant WebSocket relays)
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
    pub timestamp: Timestamp,       // When message was created (wall-clock, display only)
    pub lamport_clock: LamportClock, // Logical clock for causal ordering across devices
    pub content_type: ContentType,  // Type of content carried (text, file chunk, etc.)
                                    // Unknown future types decode as `File` — see note below
    pub content: String,            // Message content
    pub binary_content: Option<Vec<u8>>,    // Raw payload for file-chunk data
    pub media_metadata: Option<MediaMetadata>, // Media details for non-text content
    pub metadata: HashMap<String, String>,  // App-specific data
    pub requires_ack: bool,         // Whether ACK is required
    pub reply_to_msg: Option<MessageId>,    // Message this is replying to (threading)
    pub forwarded_from: Option<ForwardInfo>, // Forwarding attribution, if forwarded
}
```

#### Unknown content types

`ContentType` decodes tolerantly on every path — string, JSON, and the binary
wire codec. A content type this build does not know (one added by a newer
sender) degrades to `ContentType::File` rather than failing the message it
rides in, so adding a variant is an additive wire change and never a silent
frame drop.

Apps should therefore read `File` as "a file **or** a type this build doesn't
know". A degraded value keeps `is_media()` `true` while carrying no
`media_metadata` and no associated transfer, so render that combination as a
generic unsupported-content placeholder — "this message needs a newer app
version" — rather than as a broken or empty attachment.

## Configuration

### ProtocolConfig

Main configuration structure.

```rust
pub struct ProtocolConfig {
    pub app_id: String,             // Application ID (required)
    pub user_id: String,            // User ID (required)
    pub transport: TransportConfig, // Transport settings
    pub dors: DorsConfig,          // DORS settings
    pub relay: RelayConfig,        // Relay settings
    pub path: PathConfig,          // Path selection settings
    pub reliability: ReliabilityConfig, // Reliability settings
    pub encryption: EncryptionConfig, // Auto-encryption (MLS) settings
    pub initial_ttl: u8,           // Initial TTL (default: 8)
    pub group: GroupConfig,        // Mesh group messaging settings
    pub security: SecurityConfig,  // Transport & control-message hardening
}
```

`user_id` is the device's canonical identity on every surface: it is
stamped as the sender on outbound messages, it is what peers see as
`NeighborDiscovered.peer_id` when they discover this device (on any
transport), and it is the `recipient` string others use to reach it.

**Builder API**:
```rust
let config = ProtocolConfig::builder("my-app", "user123")
    .ble_enabled(true)
    .wifi_direct_enabled(true)
    .online_first(false)
    .initial_ttl(10)
    .encryption_enabled(true)      // Auto-encryption
    .auto_key_exchange(true)       // Auto key exchange
    .store_pending_messages(true)  // Queue pending messages
    .require_encryption(true)      // Strict fail-closed policy (the default)
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

    /// Require encryption for outbound sends (default: true — fail-closed;
    /// set to false to explicitly opt in to plaintext operation)
    pub require_encryption: bool,

    /// Bounds and eviction policy for encrypted messages received
    /// before session readiness.
    pub pending_queue: PendingQueueConfig,

    /// Emit the compact MLS envelope toward recipients that advertise
    /// support (`env_versions` in their key package). Kill switch —
    /// inbound parsing is always on. (default: true)
    pub compact_envelope_enabled: bool,

    /// Seal the rich payload (reply context, rich media metadata, forward
    /// attribution) inside the MLS ciphertext toward recipients that
    /// advertise support (`rich_versions`). Kill switch — inbound parsing
    /// is always on; rich extras are never sent cleartext. (default: true)
    pub rich_payload_enabled: bool,

    /// Recover an undecryptable 1:1 message rather than dropping it and
    /// ACKing anyway: the delivery ACK is withheld so the sender keeps
    /// retrying, and each DM resend is re-sealed against the peer's current
    /// session. Only an epoch mismatch additionally triggers a rate-limited
    /// session re-key; other failures (AEAD/authentication, discarded
    /// ratchet generations, malformed frames) withhold the ACK but never
    /// re-key. message_decryption_failed is advisory and fires per failed
    /// attempt; message_failed stays terminal. The re-key trigger is
    /// unauthenticated by construction — safe because it is bounded, not
    /// because it is trusted; see Crypto-Failure Recovery. (default: true)
    pub crypto_recovery_enabled: bool,
}
```

**TypeScript**:
```typescript
interface EncryptionConfig {
  enabled?: boolean;        // Default: true
  autoKeyExchange?: boolean; // Default: true
  storePending?: boolean;    // Default: true
  requireEncryption?: boolean; // Default: true (fail-closed)
  pendingQueue?: {
    maxPendingPerPeer?: number; // Default: 64
    maxPendingGlobal?: number;  // Default: 4096
    pendingTtlMs?: number;      // Default: 1800000 (30 min)
    overflowPolicy?: 'drop_oldest' | 'drop_newest'; // Default: drop_oldest
  };
  compactEnvelopeEnabled?: boolean; // Default: true
  richPayloadEnabled?: boolean;     // Default: true
  cryptoRecoveryEnabled?: boolean;  // Default: true
}
```

`pendingQueue` bounds the **inbound** pending-decryption queue (messages that
arrived before the session was ready). The Rust struct additionally carries
`max_pending_bytes_per_peer` (4 MiB) and `max_pending_bytes_global` (32 MiB), which
are not on the FFI dictionary — binding callers get the defaults. The *outbound*
pre-session queue is separate and has its own bounds; see
[Configuration](configuration.md#reliability-configuration).

See [Wire Format Kill Switches](configuration.md#wire-format-kill-switches) for
what `compactEnvelopeEnabled` and `richPayloadEnabled` gate, and
[Crypto-Failure Recovery](configuration.md#crypto-failure-recovery) for
`cryptoRecoveryEnabled`.

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
    pub ack: AckConfig,      // ACK timeout: 10000ms, max pending: 1000
    pub retry: RetryConfig,  // Max retries: 10, backoff: 2.0x
    pub dedup: DeduplicatorConfig, // Max tracked: 1000, retention: 1 hour
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

Two boundary rules reject the call with `Error::InvalidArgument` before anything
is queued, clocked, or persisted:

- **`recipient` must be a well-formed `UserId`.** `UserId` rejects `:`, so
  namespaced placeholder forms (`unresolved:token`, `did:key:…`, `npub:…`) must
  be resolved before they reach the SDK.
- **`content` must be at most 256 KiB.** The cap is enforced here rather than at
  transmit time, because a message waiting on MLS session establishment is
  queued in memory and on disk long before it reaches the transport's own 1 MiB
  check. Use [`send_media_with`](#rich-messaging-sealed-extras) for anything
  larger — it chunks, and is not subject to this limit.

The pending-session queue behind this call is additionally bounded at 64
messages / 2 MiB per peer and 4096 messages / 16 MiB globally; at capacity the
oldest entry is settled with `message_failed` before the new one is admitted.
See [Configuration](configuration.md#reliability-configuration).

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

#### Rich Messaging (Sealed Extras)

```rust
pub fn send_message_with(
    &mut self,
    recipient: impl Into<String>,
    content: impl Into<String>,
    options: SendMessageOptions,
) -> Result<MessageId>

pub struct SendMessageOptions {
    pub priority: Option<MessagePriority>,
    pub reply_to_msg: Option<String>,
    pub content_type: Option<ContentType>,     // outer rendering hint; sealed copy is authoritative
    pub reply_context: Option<ReplyContext>,   // sealed-only
    pub media_metadata: Option<MediaMetadata>, // sealed-only (incl. encryption_key/iv secrets)
    pub forward_info: Option<ForwardInfo>,     // sealed-only
}
```

Like `send_message`, but carrying rich extras: quoted-reply context, rich media
metadata (cloud attachments and stickers, including their `encryption_key`/`iv`
secrets), and forward attribution. Rich extras only ever travel *inside* the
MLS ciphertext, toward recipients whose key package advertises support
(`rich_versions`); toward anyone else they are silently dropped — never sent
cleartext — and the message degrades to plain text with `reply_to_msg`
threading intact. Rejects `ContentType::FileChunk` (internal transport type)
and rich extras exceeding 32 KiB serialized, both as `InvalidArgument`.

UniFFI exposes this as `send_message_rich(recipient, content, options)`; React
Native routes `sendMessage` to it automatically when rich params are present
(see the [React Native guide](react-native-integration.md)).

```rust
pub fn send_media_with(
    &mut self,
    recipient: impl Into<String>,
    file_data: Vec<u8>,
    file_name: impl Into<String>,
    content_type: ContentType,
    options: MediaSendOptions,
) -> Result<String>  // file id

pub struct MediaSendOptions {
    pub media_metadata: Option<MediaMetadata>, // delivered with chunk 0
    pub caption: Option<String>,               // sealed-only
    pub reply_to_msg: Option<String>,          // sealed-only
    pub reply_context: Option<ReplyContext>,   // sealed-only
    pub forward_info: Option<ForwardInfo>,     // sealed-only
    pub file_id: Option<String>,               // caller-supplied id (resends)
}
```

Media-transfer counterpart (UniFFI: `send_media_rich`): the rich extras ride
sealed with the transfer's chunk 0. A caller-supplied `file_id` is how an app
answers `media_resend_required` after a restart.

```rust
pub fn send_group_message_with(
    &mut self,
    group_id: &str,
    content: &str,
    options: GroupSendOptions,
) -> Result<Vec<MessageId>>

pub struct GroupSendOptions {
    pub priority: Option<MessagePriority>,
    pub reply_to_msg: Option<String>,
    pub content_type: Option<ContentType>,     // sealed-only
    pub media_metadata: Option<MediaMetadata>, // sealed-only
}
```

Group counterpart. Rich extras seal into the group MLS plaintext only when
*every* other member is known rich-capable (directly or attested by their
inviter); otherwise the text still sends but the extras drop, surfaced via the
`GroupRichExtrasDropped` event — the SDK then probes the unknown members'
capability once so a later retry can succeed.

Both group send methods return **one `MessageId` per recipient**, not one per
send. By default each is a real frame with its own outbox entry, ACK, and retry
ladder, so an app can track them individually — see
[Group sends](message-delivery.md#group-sends).

```rust
pub fn group_rich_readiness(&self, group_id: &str) -> Result<GroupRichReadiness>

pub struct GroupRichReadiness {
    pub ready: bool,                  // a rich send right now would seal
    pub unknown_members: Vec<String>, // members holding the gate closed
}
```

Point-in-time, advisory pre-check so apps can warn before sending (e.g. gray
out the attachment button) instead of learning from `GroupRichExtrasDropped`
after the drop. `ready: false` with an empty `unknown_members` means only the
local `rich_payload_enabled` kill switch blocks sealing. Exposed over UniFFI
as `group_rich_readiness` and React Native as `meshGroupRichReadiness`.

```rust
pub fn forward_message(
    &mut self,
    original_message: &Message,
    new_recipient: impl Into<String>,
    priority: Option<MessagePriority>,
) -> Result<MessageId>

pub fn forward_message_to_group(
    &mut self,
    original_message: &Message,
    group_id: &str,
    priority: Option<MessagePriority>,
) -> Result<Vec<MessageId>>
```

Forwards carry attribution (`ForwardInfo`) and the original's media metadata.
Toward rich-capable recipients both are sealed — the only way forwarded cloud
media keeps its `encryption_key`/`iv` secrets, which are always stripped from
cleartext frames at the wire boundary.

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
pub fn initialize_mls(
    &mut self,
    secure_storage: Arc<dyn MlsStorage>,
    protocol_state_storage: Arc<dyn ProtocolStateStorage>,
) -> Result<()>
```

Initializes MLS encryption with two lifecycle-separated backends. `secure_storage`
holds cryptographic and install-secret material. `protocol_state_storage` holds
restartable message-plane state and must live inside the app container so app
deletion removes pending messages, outbox entries, and retry lifecycles.

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

Encryption is required by default (`encryption.require_encryption = true`):
encrypted delivery is guaranteed, or the send fails with a typed error
(`SessionNotReady` or `EncryptFailed`) / queues when `store_pending` is enabled.
Set `require_encryption = false` to explicitly opt in to plaintext operation;
each plaintext send then emits a `SecurityWarning` event with the
`PLAINTEXT_SEND` reason code (once per peer).

Inbound plaintext is gated by the same policy: with `require_encryption = true`,
plaintext text and legacy media from the mesh are rejected instead of being
surfaced (plaintext carries no sender authentication), emitting a
`SecurityWarning` with the `PLAINTEXT_RECEIVE_REJECTED` reason code (once per
peer). Even under the opt-out, inbound plaintext from a peer known to run MLS —
an MLS session exists with them, or they have signed a control message this
install verified — is
rejected as a downgrade/forgery attempt; a confirmed session is not required,
because an honest peer queues rather than downgrading while one is pending.
Peers that have shown no MLS signal remain readable. The `message_received`
event's `encrypted` field tells apps whether the content was MLS-decrypted
(`true`) or accepted as plaintext under the opt-out (`false`).

In strict mode, send failures are fail-fast and do not transmit transport payloads.
Connection-control APIs (connection request/accept/reject) are internal plaintext
bootstrap messages and are exempt from `require_encryption` — same as key
packages and service discovery.

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

Terminal failure: max ACK retries exceeded, or the outbox lifetime/capacity
dropped the message.

#### MessageDeferred

```rust
MessageDeferred {
    message_id: String,
    reason: String,
    retry_count: u32,
    next_retry_at: Option<i64>,
}
```

Emitted when a message is queued for retry because no transport could deliver
it right now. Non-terminal.

#### MessageRetrying

```rust
MessageRetrying {
    message_id: String,
    recipient: String,
    retry_count: u32,
    next_retry_at: i64,
}
```

Emitted each time the retry machinery re-schedules a message after a failed
attempt (transport send error or ACK timeout). Non-terminal.

#### MessageUndeliverable

```rust
MessageUndeliverable {
    message_id: String,
    recipient: String,
    reason: String,          // starts with "recipient_unreachable"
    file_id: Option<String>, // set when the message is a media chunk
}
```

The transport reported the recipient unreachable (e.g. the internet relay's
delivery verdict). Non-terminal: a plain DM is *parked* and re-driven on
reachability edges. See [Message Delivery](message-delivery.md#unreachable-recipients-parking).

#### MediaResendRequired

```rust
MediaResendRequired {
    file_id: String,
    recipient: String,
    file_name: String,
    file_size: u64,
}
```

Emitted at `start()` for each outbound media transfer that was in flight when
the previous process died. The app must re-supply the bytes via `send_media`
(or `send_media_with`) with the same `file_id`; they are checksum-validated
against the interrupted transfer.

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

#### NeighborDiscovered

```rust
NeighborDiscovered {
    peer_id: String,
    transport: String,
    rssi: Option<i16>,
}
```

Emitted when a peer becomes reachable. `peer_id` is the peer's canonical
address (`off1…`) — the value that peer derived from its own identity key,
on every transport — and is valid directly as the `recipient` of
`send_message` / `send_connection_request`.

#### Connection Request Events

```rust
ConnectionRequestReceived {
    sender: String,
    sender_name: String,
    timestamp: i64,
    key_package: Option<Vec<u8>>,
    initial_message: Option<String>,
}

ConnectionRequestUndeliverable {
    recipient: String,
    message_id: String,   // id returned by send_connection_request
    reason: String,       // starts with "recipient_unreachable", or "max_retries_exceeded"
}

ConnectionAccepted {
    accepted_by: String,
    accepted_by_name: String,
    timestamp: i64,
    key_package: Option<Vec<u8>>,
}

ConnectionRejected {
    rejected_by: String,
}

ConnectionRequestCancelled {
    cancelled_by: String,
}
```

Sender-side failure contract: recipient offline emits
`ConnectionRequestUndeliverable` (reason starts with
`recipient_unreachable`); retry exhaustion emits it with reason
`max_retries_exceeded` alongside the generic `MessageFailed`. Both
correlate with the message id returned by `send_connection_request`.
Answers (`ConnectionAccepted` / `ConnectionRejected`) correlate by peer
id, not message id. Undeliverable is a status signal, not proof of
permanent failure — the retried original may still arrive if the peer
comes back online.

#### GroupRichExtrasDropped

```rust
GroupRichExtrasDropped {
    group_id: String,
    unknown_members: Vec<String>,
}
```

Rich media metadata was dropped from an outbound group message because the
group is not fully rich-capable (or the local `rich_payload_enabled` kill
switch is off — then `unknown_members` is empty). The text was still sent.
The SDK probes the unknown members' capability automatically; apps can warn
the sender and retry later, or pre-check with `group_rich_readiness`.

#### GroupUnauthorizedMembershipChange

```rust
GroupUnauthorizedMembershipChange {
    group_id: String,
    committer: String,       // MLS-authenticated committer
    added: Vec<String>,      // sorted; empty for a pure removal
    removed: Vec<String>,    // sorted; empty for a pure addition
    reason: String,          // "sender_not_admin" | "affected_member_mismatch"
}
```

An MLS membership change was applied that its committer was not authorized
to make. The change **has been applied** — refusing it would mean refusing
the MLS merge, permanently forking this member from the group — so it is
reported instead; the corresponding `GroupMemberAdded` / `GroupMemberRemoved`
events still fire with `authorized: Some(false)`. The judgment runs against
the local, best-effort role replica, so it can false-positive; treat it as a
moderation signal for a human admin and never reverse automatically. Reports
are rate-limited per `(group, committer)`. Treat `reason` as an opaque
string — values may be added. See
[Group authorization model](./mls-integration.md#group-authorization-model).

#### GroupRoleChanged

```rust
GroupRoleChanged {
    group_id: String,
    user_id: String,
    new_role: String,    // "admin" or "member"
    changed_by: String,
}
```

Emitted when a member's role is changed in a group.

#### GroupRenamed

```rust
GroupRenamed {
    group_id: String,
    new_name: String,
    old_name: Option<String>,
    renamed_by: String,
}
```

Emitted when a group is renamed.

## Group Role Management

The high-level mesh group API includes role-based access control.

### GroupRole

```rust
pub enum GroupRole {
    Admin,   // Can invite/remove members and change roles
    Member,  // Default role, can send/receive messages
}
```

### Role Methods

```rust
/// Set a member's role (admin only).
pub fn set_member_role(
    &mut self,
    group_id: &str,
    user_id: &str,
    role: GroupRole,  // GroupRole::Admin or GroupRole::Member
) -> Result<()>
```

```rust
/// Get a member's role.
pub fn get_member_role(
    &self,
    group_id: &str,
    user_id: &str,
) -> Result<GroupRole>
```

```rust
/// Get all member roles in a group.
pub fn get_group_roles(
    &self,
    group_id: &str,
) -> Result<HashMap<String, GroupRole>>
```

### Group Info

```rust
/// Get information about a group.
pub fn get_group_info(
    &self,
    group_id: &str,
) -> Result<Option<MlsGroupInfo>>
```

Returns group metadata including members, epoch, and timestamps. Returns `None` if the group does not exist.

### Group Rename

```rust
/// Rename a group (admin only, broadcasts to all members).
pub fn rename_group(
    &mut self,
    group_id: &str,
    new_name: &str,
) -> Result<()>
```

Renames a group and broadcasts the change to all members. Only admins can rename groups.

### Input Validation

- `create_group` and `rename_group` trim whitespace and reject empty group names with an error.

### Security Invariants

- The group creator is automatically assigned `Admin`.
- Only admins can call `invite_to_group`, `remove_from_group`, `set_member_role`, and `rename_group`.
- Membership changes (invite/remove) are **not** enforced on receive — an unauthorized commit is applied and reported via `GroupUnauthorizedMembershipChange`. See [Group authorization model](./mls-integration.md#group-authorization-model).
- The last admin cannot be demoted, removed, or leave — returns `Error::LastAdmin`.
- If the last admin leaves unexpectedly, deterministic election promotes the lexicographically smallest member.

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

## UniFFI Bindings

Native platform integration uses UniFFI-generated type-safe bindings for Swift and Kotlin. The bindings are auto-generated from the UDL definition in `crates/offline-protocol-uniffi/`.

See [React Native Integration](react-native-integration.md) for the complete TypeScript API, [iOS Integration](ios-integration.md) for Swift usage, and [Android Integration](android-integration.md) for Kotlin usage.

## Platform-Specific APIs

See platform-specific documentation:
- [React Native Integration](react-native-integration.md)
- [iOS Integration](ios-integration.md)
- [Android Integration](android-integration.md)
