# Changelog

All notable changes to the Offline Protocol SDK are documented in this file. This changelog covers everything after the **v0.7.1** release.

## [Unreleased] — since v0.7.1 (2026-03-03)

### Breaking Changes (React Native Bindings)

If you are building an app with the React Native bindings, the following changes require updates to your code:

#### `mls_add_group_member()` return type changed

The return type of `mlsAddGroupMember()` has changed from a single `MlsWelcomeMessage` to a new `MlsAddMemberResult` containing both a Welcome and a Commit message.

**Before:**
```typescript
const welcome = await protocol.mlsAddGroupMember(groupId, keyPackage);
// Send welcome to the invitee
```

**After:**
```typescript
const result = await protocol.mlsAddGroupMember(groupId, keyPackage);
// result.welcome → send to the invitee
// result.commit  → sent to existing group members automatically
```

#### New error variants: `UserBlocked` and `LockPoisoned`

Two new error variants have been added to `ProtocolError`:

- **`UserBlocked`** — Thrown when attempting to send a message, control signal, or establish a connection with a blocked user. If your app catches `ProtocolError` exhaustively, you must now handle this case.
- **`LockPoisoned`** — Thrown when an internal mutex is in a poisoned state (indicates a prior panic inside the SDK). This replaces the previous behavior where 129 `lock().unwrap()` calls would panic the process. Apps should treat this as a fatal internal error and consider restarting the protocol engine.

#### New `ProtocolConfig` fields

Three new fields have been added to `ProtocolConfig`. They have defaults so existing code will compile, but you should review them:

| Field | Type | Default | Description |
|-------|------|---------|-------------|
| `max_group_members` | `u32` | `256` | Maximum members allowed in a single MLS group |
| `group_relay_enabled` | `boolean` | `true` | Whether relay broadcasting is used for group fan-out |
| `require_transport_identity` | `boolean` | `false` | Enables Ed25519 sender identity binding at the transport layer |

#### New event types in React Native bindings

The following event types have been added to the React Native TypeScript types since v0.7.1:

- **`service_discovered`** — A peer advertised a mesh service in response to a discovery query.
- **`service_request_received`** — An incoming service request from another peer.
- **`service_response_received`** — A response to a service request you sent.
- **`presence_updated`** — A peer sent a presence update (Online, Away, Offline).
- **`typing_indicator_received`** — A peer started or stopped typing.
- **`read_receipt_received`** — A peer read one or more of your messages.
- **`message_relayed`** — A message was relayed through this node in the mesh.
- **`message_deferred`** — A message was queued for later delivery because no transport was available.

#### Rust-level events (not yet bridged to React Native)

The following events are emitted at the Rust/UniFFI layer but are not yet exposed in the React Native TypeScript types. If you are consuming the SDK directly through UniFFI (Swift/Kotlin), you should handle these:

- **`UserBlocked`** / **`UserUnblocked`** — A user was blocked or unblocked locally.
- **`MessageDecryptionFailed`** — An incoming message could not be decrypted.
- **`GroupEpochForkDetected`** / **`GroupEpochForkResolved`** — MLS epoch fork lifecycle events.
- **`SecurityWarning`** — A control message failed authentication or replay checks.
- **`TofuReset`** — TOFU trust state was reset for a peer.

#### `ForwardInfo` added to message events

`MessageReceivedEvent`, `MessageSentEvent`, and `GroupMessageReceivedEvent` now include an optional `forward_info` field. If your app displays messages, you should check for this field to show forwarding attribution:

```typescript
interface ForwardInfo {
  original_sender: string;
  original_message_id: string;
  original_timestamp: number;
  forward_count: number;
}
```

#### Native bridge expansion (iOS & Android)

The native modules (`OfflineProtocolModule.swift` and `OfflineProtocolModule.kt`) have been significantly expanded. If you have custom native module extensions or overrides, you will need to add implementations for the new methods: `blockUser`, `unblockUser`, `getBlockedUsers`, `isUserBlocked`, `forwardMessage`, `meshForwardMessageToGroup`, `sendPresenceUpdate`, `sendTypingIndicator`, `sendReadReceipt`, and the full `MeshServices` API surface (`registerService`, `unregisterService`, `discoverServices`, `sendServiceRequest`, `respondToServiceRequest`).

---

### Features

- **Service discovery and request/response** ([#45](https://github.com/Offline-Protocol/sdk/pull/45))
  Added a new `MeshServices` subsystem that enables peer-to-peer service discovery and typed request/response over the mesh network. Services are advertised via gossip broadcast and discovered without a central registry. Includes auto `not_found` responses for unknown services, known_peers tracking independent of MLS encryption state, configurable max-hops gossip limit to control broadcast radius, payload size limits and capacity bounds to prevent resource exhaustion, sender-based response routing for multi-hop meshes, and the new `OutboundMessage` struct replacing raw tuples throughout the send path. Full UniFFI bindings are included for iOS and Android.

- **Transport-agnostic MLS group messaging** ([#46](https://github.com/Offline-Protocol/sdk/pull/46))
  Wired MLS group encryption into the protocol engine, enabling encrypted group conversations that work seamlessly across BLE, WiFi Direct, and Internet transports. Messages are encrypted once and fan-out to all group members via DORS-selected transports. Adds configurable `max_group_members` (default 256), relay broadcast optimization for large groups, pending MLS commit buffering with TTL-based expiry, classification of MLS errors into permanent (e.g., bad state) vs retriable (e.g., out-of-order) categories, and extraction of `GroupMeshState` for cleaner state management. Includes 82 new tests in a dedicated test module.

- **Presence, typing indicators, and read receipts** ([#48](https://github.com/Offline-Protocol/sdk/pull/48))
  Added protocol-level support for three real-time communication signals: presence updates with a `PresenceStatus` enum (Online, Away, Offline), typing indicators with per-conversation granularity, and read receipts supporting batch message IDs. All three are implemented as lightweight internal control messages routed through DORS, meaning they work across any transport without a relay server. Input validation prevents empty recipient/conversation IDs and excessive message ID lists. Full UniFFI and React Native bindings for mobile with 25 tests covering edge cases.

- **User blocking with silent message filtering** ([#54](https://github.com/Offline-Protocol/sdk/pull/54))
  Added a complete user blocking system with a locally-persisted block list. Blocked users are filtered in the receive pipeline (after dedup but before ACK, so blocked senders never learn they are blocked), with guards on all outbound paths including send, control messages, and connection establishment. Blocking persists across restarts via `MlsStorage`. Unblocking a user cleans up stale MLS sessions. Includes a typed `Error::UserBlocked` variant, outbound presence leak prevention (so blocked users don't see your status), file transfer cleanup, a `MAX_BLOCKED_USERS` cap, and full UniFFI/React Native bindings.

- **User-level message forwarding with attribution** ([#61](https://github.com/Offline-Protocol/sdk/pull/61))
  Introduced first-class message forwarding as a protocol feature. Forwarded messages carry a `ForwardInfo` struct containing the original sender, original message ID, original timestamp, and a forward count. Both 1:1 (`forward_message()`) and group (`forward_message_to_group()`) forwarding are supported. The pending queue preserves forwarding attribution through retries, relay broadcast handles forwarded group messages, and a `MAX_FORWARD_COUNT=100` cap prevents infinite forwarding chains. Content type and media metadata are preserved through the forwarding path. Full React Native bindings included.

- **Demo app** — Added a simple demo app (`examples/demo-app/`) showcasing all SDK features including messaging, groups, presence (via `sendPresenceUpdate`), typing indicators, read receipts, service discovery, blocking, forwarding, and message relay/deferral tracking. Uses the production-recommended reliability config (10 retries, 10s ACK timeout).

### Performance

- **Reduce message latency from invitation to delivery** — Overhauled polling and timing across the stack to dramatically reduce the time from MLS invitation to first decryptable message. Replaced the 750ms × 8 fixed-interval MLS establishment polling with a 100ms exponential backoff helper that resolves faster in the common case. Aligned the Android process tick interval with iOS (500ms → 100ms) to eliminate a platform-specific latency gap. Reduced startup delay from 500ms to 100ms, and presence rebroadcast interval from 60s to 15s for faster peer discovery. Tightened reliability config in the example app to match production expectations.

### Bug Fixes

- **Harden mesh group robustness** ([#47](https://github.com/Offline-Protocol/sdk/pull/47))
  Fixed several issues that caused group messaging to degrade under real-world conditions. Stale relay caches now refresh from MLS membership on each fan-out. Added a leave election fallback with staggered re-election timeouts so groups can recover when the elected leader crashes. Implemented epoch fork detection using Lamport clock comparison and automatic resolution via leader-elected key-update commits. Added a circuit breaker on elections to prevent election storms, tuple-keyed leave elections to handle concurrent leaves, and per-attempt cooldown to prevent rapid-fire retries.

- **Harden control message authentication and sender verification** ([#49](https://github.com/Offline-Protocol/sdk/pull/49))
  Comprehensive security hardening of the control message path. Added transport-level sender identity binding so peers can verify who sent each control message. Implemented Ed25519 control message signing with TOFU (Trust On First Use) key pinning — the first time you communicate with a peer, their signing key is recorded, and all future control messages are verified against it. Added protections against internal prefix injection (where a malicious peer crafts payloads that look like control messages), LRU TOFU eviction for bounded memory, replay protection via nonce tracking, length-prefixed binary signing payloads with domain separators, and a `SecurityRejected` variant that suppresses ACKs for rejected messages so attackers don't get delivery confirmation.

- **Harden TOFU transport prefixes** ([#50](https://github.com/Offline-Protocol/sdk/pull/50))
  Hardened prefix handling in the TOFU transport layer to prevent prefix confusion attacks.

- **Harden TOFU storage and validate identity strings** ([#51](https://github.com/Offline-Protocol/sdk/pull/51))
  Added input validation throughout the identity system. `UserId` and `AppId` constructors now reject storage-hostile characters (path separators, null bytes) and all ASCII control characters to prevent key injection and filesystem traversal. TOFU restore keys are validated before use. TOFU peer restore is capped at `MAX_TOFU_PEERS` with a deterministic secondary sort for consistent truncation behavior.

- **Wire user blocking to native bridges** ([#55](https://github.com/Offline-Protocol/sdk/pull/55))
  Wired the Rust-level user blocking API through to the Android and iOS native bridge modules and fixed incorrect field mappings in service discovery event payloads.

- **Address 14 bugs from full codebase audit** ([#56](https://github.com/Offline-Protocol/sdk/pull/56))
  Fixed 14 bugs found during a systematic codebase audit: ACK piggyback overflow was silently dropped (messages lost), `FileChunk` had unbounded memory allocation (DoS vector), `finalize_file` skipped SHA256 checksum verification (integrity gap), `LamportClock` deserialization bypassed value clamping (could overflow), UniFFI storage error variant was mismapped (wrong errors surfaced to apps), `RetryEntry` had an Eq/Ord contract violation causing duplicate enqueues, routing table had a stale reverse index (phantom routes), DORS produced NaN scores when `ttl==0`, `MockTransport` used LIFO instead of FIFO ordering (tests didn't match real behavior), UniFFI event callback could deadlock under contention, `received_messages` used O(n) removal (degraded with message volume), `RetryQueueStats` was missing the Critical priority level, and added `#![deny(unsafe_code)]` to the MLS crate.

- **Production readiness improvements** ([#57](https://github.com/Offline-Protocol/sdk/pull/57))
  Replaced all 129 `lock().unwrap()` calls across the codebase with poison-recovering lock wrappers, so a panic in one thread no longer takes down the entire SDK. Added a CI pipeline with fmt, clippy, test, cargo-deny license/advisory checking, and code coverage. Added a TOFU reset API for apps that need to clear trust state. Added a 1MB max message size guard at the transport layer to prevent oversized payloads from crashing BLE stacks. Added cargo-deny config and SECURITY.md. Fixed `receive_message()` silently dropping messages when serialization failed (now returns a proper error).

- **Save BLE fragments on missing peripheral** ([#58](https://github.com/Offline-Protocol/sdk/pull/58))
  BLE fragments are now saved to a buffer when the peripheral connection is temporarily unavailable, instead of being silently dropped. This fixes a data loss issue where BLE messages were lost during brief connection interruptions.

- **Enforce FIFO ordering for BLE fragment queues** ([#59](https://github.com/Offline-Protocol/sdk/pull/59))
  Fixed BLE fragment queues to enforce strict FIFO ordering. Previously, fragments could be delivered out of order, causing message reassembly failures on the receiving side.

- **Mesh networking fixes** ([#60](https://github.com/Offline-Protocol/sdk/pull/60))
  A collection of mesh networking improvements: added unicast multi-hop relay forwarding with a `MessageRelayed` event so apps can track relay activity, switched dedup storage to LRU eviction for bounded memory, added composite-score route eviction so stale routes are pruned based on quality rather than age alone, enabled multi-hop service discovery responses with an originator field for correct return routing, reduced epoch fork false positives by tightening detection thresholds, adopted RFC 1982 serial number arithmetic for sequence numbers (handles wrapping correctly at 2^32), and extracted relay logic into a dedicated helper module.

- **Decouple transport retries from ACK retries** ([#62](https://github.com/Offline-Protocol/sdk/pull/62))
  Fixed a fundamental reliability issue where messages permanently died after just 3 transport send failures, even though the transport was only temporarily unavailable. The root cause was that the `max_retries` limit was applied at enqueue time rather than being purely a scheduling concern. Now, enqueue is always accepted and the retry queue handles scheduling with exponential backoff. Added `drain_all()` and `flush()` methods so messages are sent immediately when a transport becomes available. Fixed ghost re-sends from un-cleaned retry queue entries, double-sends from concurrent flush paths, and zombie entries that never expired. Returns `Ok` with a `MessageDeferred` event when no transport is available (instead of `Err`). Bumped defaults to 10 retries with 10s ACK timeout for better real-world reliability.

- **Fix message forwarding** — Fixed a bug where forwarded message JSON was never parseable because the serialization format didn't match the deserialization expectation.

- **Fix group messaging** ([#63](https://github.com/Offline-Protocol/sdk/pull/63))
  Six targeted fixes for group messaging: added missing dedup check in the relay group handler (duplicate messages were processed twice), prevented duplicate Welcome messages from overwriting valid MLS state (caused decryption failures for all subsequent messages), stopped raw ciphertext from leaking as `GroupMessageReceived` events on decrypt failure (apps received garbage), added retry logic for commit fan-out (commits to some members were silently lost), fixed `GroupManager` incorrectly defaulting the group display name to the internal group ID, and fixed the demo app not cleaning up local state when the current user is kicked from a group.

- **Add missing dedup to commit, Welcome, and leave handlers** ([#64](https://github.com/Offline-Protocol/sdk/pull/64))
  Added deduplication to `handle_group_mls_commit`, `handle_group_mls_welcome`, and `handle_group_mls_leave` — the three group control message handlers that had no dedup at all. Without this, duplicate network deliveries caused false epoch fork detection (from double-applied commits), wasted cryptographic operations (from reprocessing Welcomes), and election timer resets (from duplicate leave messages).

### Refactoring

- **Split protocol.rs monolith** ([#52](https://github.com/Offline-Protocol/sdk/pull/52))
  Split the ~3000-line `protocol.rs` file into focused sub-modules (messaging, groups, security, presence, services, etc.) for better maintainability and faster compilation. No behavioral changes.

- **Extract pending queue module** ([#53](https://github.com/Offline-Protocol/sdk/pull/53))
  Extracted the pending message queue into its own module with explicit imports, reducing coupling between the queue logic and the main protocol engine.

### Documentation

- **Clean up documentation** — Removed duplicate documentation files (including `bindings/react-native/MESH.md`), fixed outdated information across READMEs and inline docs, and added a documentation index for easier navigation. If you had external references to `MESH.md`, note that it has been removed — the relevant information is now covered in inline documentation and the main README.
