# CLAUDE.md

This file provides guidance to Claude Code (claude.ai/code) when working with code in this repository.

## Project Overview

Offline Protocol SDK: an offline-first messaging protocol in Rust with multi-transport switching (BLE, WiFi Direct, Internet), mesh networking, and automatic MLS end-to-end encryption (RFC 9420). Exposed to iOS/Android/React Native via UniFFI bindings.

## Common Commands

```bash
# Verify loop (lint subsumes typecheck; don't run a separate `cargo build` first —
# it only adds a third artifact set, including the expensive uniffi cdylib link)
cargo clippy --workspace -- -D warnings
cargo test --workspace --lib                    # all unit tests; skips the empty per-crate doctest passes

# Test
cargo test --workspace                          # full run incl. doctests (what CI runs)
cargo test --package offline-protocol-core      # single crate
cargo test test_message_creation                # single test
cargo test -- --nocapture                       # with stdout

# Build (only when you need the compiled artifacts, e.g. the uniffi cdylib)
cargo build --workspace
cargo build --workspace --release

# Format (must pass before commits; fmt takes --all, not --workspace)
cargo fmt --all
cargo fmt --all -- --check                      # check only

# Docs
cargo doc --workspace --no-deps

# Benchmarks (Criterion)
cargo bench --package offline-protocol-bench
```

### UniFFI / Mobile Builds

```bash
cd bindings/react-native
npm run build:uniffi:all          # all platforms
npm run build:uniffi:ios          # iOS only
npm run build:uniffi:android      # Android only
npm run generate:bindings         # regenerate after UDL changes
```

Prerequisites: `cargo install uniffi --version 0.30.0 --features cli --locked` (must match the workspace `uniffi = "0.30"` pin), Android NDK (`ANDROID_NDK_HOME`), Xcode.

## Architecture

### Dependency Graph (bottom-up)

```
offline-protocol-core          ← Foundation: Message, UserId, AppId, TTL, HopCount, timestamps
    ↓
offline-protocol-transport     ← Transport trait + BLE/WiFi Direct/Internet impls, TransportMetrics
offline-protocol-reliability   ← AckManager, RetryQueue (exp backoff), Deduplicator, AckOptimizer
offline-protocol-mls           ← MlsManager, MlsStorage trait, session & group encryption (OpenMLS)
offline-protocol-services      ← MeshServices: service registry, discovery (gossip), request/response
    ↓
offline-protocol-router        ← DORS transport selector, RelayManager, PathSelector, gossip routing
    ↓
offline-protocol               ← Main engine: OfflineProtocol, ProtocolConfig, TransportManager, events
    ↓
offline-protocol-uniffi        ← UniFFI bindings for Swift/Kotlin (cdylib + staticlib)
offline-protocol-bench         ← Criterion benchmarks
```

### Key Architectural Patterns

- **`Transport` trait** (`crates/offline-protocol-transport/src/traits.rs`): all transports implement this; uses `as_any()` for safe downcasting. `MockTransport` available for tests.
- **`MlsStorage` trait** (`crates/offline-protocol-mls/src/storage.rs`): platform-agnostic secure storage interface — apps implement this for iOS Keychain, Android Keystore, etc.
- **DORS** (`crates/offline-protocol-router/src/dors.rs`): multi-factor scoring (RSSI, congestion, bandwidth, battery, reliability, capacity) with hysteresis, cooldown, and stability window to prevent transport flapping.
- **Wire format**: `Message` serializes as JSON by default (single chokepoint in `crates/offline-protocol-transport/src/common.rs`), with an additive compact binary codec — wire v1, `crates/offline-protocol-core/src/wire.rs` — selected per peer. Receivers auto-detect via the first byte (`0xF5` = binary, `{` = JSON); senders emit binary only to peers that advertise support (`wire_versions` in the signed key package), gated by `TransportConfig::binary_wire_enabled` (default on). JSON stays the permanent floor and the sole format for persistence and the internet relay. The frozen `WireMessageV1` DTO must never change field order/types — additive changes use its `ext` TLV section (tag registry on `EXT_TAG_B64_TAIL` in `wire.rs`; tag 1 carries base64 content tails as raw bytes); a breaking change bumps the magic byte (`0xF6` = v2) and negotiates.
- **MLS envelope (end-to-end, distinct from the hop-local wire codec)**: `__MLS_ENC__` payloads are legacy JSON or, for recipients advertising `env_versions` in their key package, base64 of `EncryptedMessage::to_bytes` (compact, ~2.7× smaller). Sealed per-recipient in `protocol/send.rs::seal_encrypted_content`, sniffed by the byte after the prefix in `protocol/message_dispatch.rs::parse_encrypted_payload` (`{` = JSON); parsing accepts all historical forms unconditionally. Gated by `EncryptionConfig::compact_envelope_enabled` (default on), independent of `binary_wire_enabled`. Size ground truth: `wire_size_and_fragment_report_for_encrypted_dms` test (run with `--nocapture`).
- **Deferred-ACK atom (queue-path silent-loss fix)**: an encrypted DM or media chunk received *before* the receiver's MLS session/group is established is queued for delayed decryption but is **not** delivery-ACKed and **not** left dedup-marked. The `DecryptResult::SessionNotReady` path returns `InternalMessageResult::Deferred` (media: `ChunkOutcome::Deferred` from `handle_incoming_file_chunk_via`, itself driven by `MediaChunkDecrypt::Deferred`), and the receive loop responds by `unmark_seen` + skip-ACK — so the sender keeps retransmitting and each resend re-enters processing instead of hitting the duplicate re-ACK path. Six interdependent pieces, correct only together (see the `InternalMessageResult::Deferred` and `ChunkOutcome` doc comments in `protocol/types.rs`): (1) `Deferred` outcome; (2) idempotent enqueue by message id (`PendingDecryptionQueue::enqueue` — resends don't stack, TTL measured from first receipt); (3) drain on *any* successful decrypt (`confirm_session_from_successful_decrypt` → `process_pending_decryption`, fixing the both-create **owner** path that never receives a Welcome); (4) re-mark the id when the drain surfaces the message; (5) pending TTL 2 min → 30 min (`DEFAULT_PENDING_TTL_MS`, FFI-mirrored); (6) **ACK on drain**: the arrival transport is recorded on the pending entry (`PendingDecryptMessage::received_via`, plumbed via the `_via` variants — `enqueue_pending_decryption_via`, `handle_incoming_file_chunk_via`, `handle_encrypted_message(.., arrival_transport)`), and the drain sends the deferred delivery ACK directly on it (`process_pending_decryption` → `ack_drained_message`). **ACK-latency semantic (important):** the drain surfaces the message *locally* **and** ACKs it on the recorded transport — so the common window is closed. The ACK still degrades gracefully: if `received_via` is `None` (transport-less enqueue, defensive re-enqueue) or that transport is gone, the ACK falls back to DORS and ultimately to the sender's-next-resend re-ACK path, so a late/absent delivery-ACK during the not-yet-confirmed window is **not** loss (the receiver may already hold the message; a sender that exhausts its retry budget before both the session confirms *and* an ACK lands may still mark the message undeliverable though it was delivered locally — strictly better than the old silent drop, and app teams must not read a missing ACK as non-delivery). Plaintext media chunks rejected by the encryption policy return `ChunkOutcome::Rejected` (no ACK, id unmarked), matching the plaintext-text `SecurityRejected` path. An evicted encrypted media chunk surfaces `MessageDecryptionFailed`/`PendingQueueDropped`, but that signal is **advisory** (transfer *stalled*, recoverable on resend) — the terminal media signal remains `FileReceiveFailed`. Coverage: `test_deferred_encrypted_dm_defers_ack_then_recovers_without_loss_or_dup`, `test_deferred_dm_is_acked_on_drain_without_a_resend`, `test_evicted_pending_message_recovers_on_resend_after_session_ready`, `test_pending_queue_drains_on_decrypt_confirmation_owner_path`, `test_deferred_media_chunk_defers_ack_then_recovers`, `test_plaintext_media_chunk_rejected_withholds_ack_and_unmarks`. **Mesh-group path (same atom, group-specific shape)**: the mesh group handler `handle_group_mls_msg_via` (arrival transport plumbed from the `process_internal_message_via` dispatch) returns `InternalMessageResult::Deferred` when a message is buffered because local group state lags (`GroupDecryptOutcome::Retriable` → `buffer_pending_group_message`, `PendingGroupMessage::received_via`), reusing the same receive-loop `Deferred` arm (skip-ACK + `unmark_seen`). `drain_pending_group_messages` sends the deferred ACK on the recorded transport via `ack_drained_group_message` → `send_group_delivery_ack`. Two group-specific differences from the DM path: (a) there are **two** dedup layers — the group-level `message_dedup` stays marked across the whole pending lifetime (replay-amplification defense + authoritative double-delivery guard) while the receive loop unmarks only the transport `deduplicator`, so the drain does **not** re-mark the transport dedup; (b) a duplicate of a *still-pending* message returns `Deferred` (checked via `is_group_message_pending`, before decrypt) rather than re-ACKing, and only a duplicate of an already-delivered id returns `Consumed`. The existing `release_replay_protection` (clears both dedup layers on eviction/TTL-drop) is the un-ACKed sender's recovery path. The **relay** path (`handle_relay_group_message_with_mls`) is deliberately exempt: it sends no delivery ACK and the relay sender is not ACK-gated (`try_relay_broadcast`), so its buffered entries carry `received_via: None` and the drain ACK is a correct no-op. Coverage: `test_deferred_group_msg_defers_ack_then_recovers_without_loss_or_dup`, `test_deferred_group_msg_is_acked_on_drain_without_a_resend`, `test_group_dup_while_pending_defers_not_reacks`, `test_group_dup_after_delivery_reacks_and_not_redelivered`, `test_evicted_pending_group_msg_recovers_on_resend_after_state_ready`.
- **Crypto-failure recovery (1:1 desync heal, distinct from the not-ready defer above)**: an *established* 1:1 MLS session that has forked (the two sides disagree on the MLS epoch) yields a `WrongEpoch`/`NoPastEpochData` decrypt failure. This is classified as the dedicated recoverable `MlsError::SessionDesync` (in `group.rs::process_message`, shared by 1:1 and group decrypt) → `SessionStateError::SessionDesync` → `DecryptResult::SessionDesync`, kept strictly separate from `Decryption`: AEAD/corrupt/forged and ratchet-generation failures stay `Decryption` and fail closed, because re-keying on them would be a re-key-storm vector (coverage: `test_corrupt_ciphertext_is_not_classified_as_session_desync`). **Tier 1 (honest failure + heal):** on the 1:1 receive path (text: `handle_encrypted_message`; media: `receive.rs`) a desync withholds the delivery ACK and `unmark_seen`s the id (reuses the Deferred atom arm) but does **not** enqueue — the ciphertext is sealed to the dead epoch and can never drain — and triggers `schedule_session_rekey`: tear down our own stale session **and** advertise a `session_reset` key package (the peer drops its stale session, rebuilds from our key package, and Welcomes us back, which we join session-less). Deleting the local session is what makes convergence symmetric for both user-id orderings (the returning Welcome is *joined*, not gated by the greater-id-adopts tiebreaker). Rate-limited to one re-key per peer per `REKEY_INTERVAL_SECS` (30s) via `rekey_due_at`, stamped before send, cleared on any decrypt success. **SECURITY:** `schedule_session_rekey`'s `peer_id` is the *wire-claimed* sender (the `SenderIdentityMismatch` gate only runs on successful decrypt, not on a `WrongEpoch` failure) — an outsider cannot forge a qualifying frame, but a network attacker **replaying a genuine peer's captured old-epoch ciphertext** can force one rate-limited teardown+re-establishment of that session per window; strictly better than the old silent drop, and the rate limit is the mitigation. **Tier 2 (true no-loss):** the sender keeps per-outbox-entry re-seal provenance (`OutboxReseal`, memory-only via `#[serde(skip)]` — holds plaintext, never persisted) so each resend re-seals against the peer's *current* session (`reseal_for_resend_in_place` at the two resend transmit points; `reseal_resend_content` gated on `confirmed_sessions`, preserves `Message.id` for dedup/ACK). Media has no Tier 2 (chunks are re-encoded, not replayed): it recovers via the descriptor-based `MediaResendRequired` path. Staging (`pending_reseal`) is strictly transient — `take_staged_reseal` always removes, and `remove_outbox_entry` clears it belt-and-braces, so a staged-but-dropped send never strands plaintext. Whole thing gated by `EncryptionConfig::crypto_recovery_enabled` (default on, FFI/RN-mirrored); disabled → legacy drop-and-ACK fall-through. Coverage: `test_desync_dm_withholds_ack_and_triggers_rekey`, `test_desync_rekey_is_rate_limited`, `test_desync_dm_heals_end_to_end_when_detector_id_is_{greater,smaller}`, `test_reseal_on_resend_recovers_after_recipient_rekeys_to_new_epoch`, `test_crypto_recovery_disabled_falls_back_to_drop_and_ack`.
- **Sealed rich payload (inside the MLS plaintext)**: rich message extras (reply_context, rich media_metadata incl. key/iv secrets, forward_info) only ever travel as a `__RICH_V1__` + JSON body wrapped around the text *before* encryption, negotiated via `rich_versions` in the key package (`RICH_PAYLOAD_V1`). Sealed in `protocol/send.rs::prepare_outbound_content` (the single chokepoint for fresh sends and pending flushes; `PendingMessage.rich` preserves option-borne provenance through the queue), restored in `protocol/receive.rs::apply_decrypted_content` right after the outer-field strip — sealed body is authoritative, outer copies are wiped. The body also carries a sealed copy of the outer `content_type` hint (additive field; absent → outer stands, `FileChunk` refused on restore like at the send boundary); fresh sends with a non-Text hint seal a hint-only body even without extras. Forwards (`forward_message`) seal their attribution + the original `media_metadata` as extras toward capable recipients — the only way forwarded cloud media keeps its `encryption_key`/`iv` — with the cleartext outer copies kept as the legacy fallback (secrets stripped at the wire chokepoint; sealed restore overwrites them wholesale). Non-capable recipients: extras silently dropped, never cleartext. **Groups**: the same `__RICH_V1__` body seals into the group MLS plaintext (`group_mesh.rs::send_group_message_inner`; hint-only non-Text sends seal too — the group payload has no outer content_type carrier, so an unsealed hint would be lost, not just unprotected; parse shared via `RichPayloadV1::parse_sealed`, applied on all three inbound paths — mesh, buffered drain, relay), but gated on *every* other member being known rich-capable: directly in `peer_rich_payload`, or inviter-attested in `peer_rich_attested` — the Add commit carries `affected_member_rich` (to existing members) and the Welcome a `member_rich` map (to the joiner; entries bounded to the joined MLS roster, admin-gated on the commit like `role`), so members added by someone else stay sealable; attestation chains across successive adds, direct exchange always overrides it, and it feeds *only* the group gate (never DM sealing or envelope selection). A genuinely unknown member still fails the gate closed and extras drop, surfaced via `GroupRichExtrasDropped` (now with `unknown_members`), and the drop path backfills by key-packaging the unknown members once (their auto-exchange reply reopens the gate — the heal for pre-attestation groups). Apps can pre-check via `group_rich_readiness(group_id)`. When the body seals, the hop-visible payload `forward_info` copy is omitted (every member reads the sealed attribution; a payload copy would only expose the original sender to relays). Surfaced via additive `media_metadata`/`content_type` fields on `GroupMessageReceived` (telemetry scrubber redacts the secrets); public surface `send_group_message_with`/`GroupSendOptions` + `forward_message_to_group` (core; not yet exposed over UniFFI beyond the pre-existing forward API). Per-peer capability sets (`env_versions`/`rich_versions`) persist across restarts as `PeerCapabilities` (`peer_capabilities` storage key — separate from the key-package cache, which is deleted on session creation) and are restored in `initialize_mls` before `start()` flushes pending sends; `wire_versions` stays in-memory by design (hop-local, re-exchanged on connect). Gated by `EncryptionConfig::rich_payload_enabled` (default on), independent of the other two kill switches; inbound parsing unconditional (parse failure → raw text + warning). Public surface: `send_message_with`/`SendMessageOptions` (core), `send_message_rich` (UniFFI), rich params on RN `sendMessage`. Boundary validation in `send_message_with`: rejects `ContentType::FileChunk` and rich extras >32 KiB serialized (`MAX_RICH_EXTRAS_BYTES`; capped at the boundary, not seal time, so a flush re-seal can never fail the cap and re-queue forever).
- **Protocol control messages**: internal prefix convention (`__MLS_KEY_PKG__`, `__MLS_WELCOME__`, `__MLS_ENC__`, etc.) in `crates/offline-protocol/src/protocol.rs`. Service messages use `__SVC_DISC_Q__`, `__SVC_DISC_R__`, `__SVC_REQ__`, `__SVC_RESP__` prefixes in `crates/offline-protocol-services/src/payloads.rs`.
- **Event-driven**: `OfflineProtocol` emits events (MessageReceived, PeerDiscovered, TransportChanged, etc.) via `EventCallback`.
- **Runtime telemetry**: apps install a `TelemetrySink` via `OfflineProtocol::install_telemetry_sink(sink, config)`; `TelemetryConfig::mls_verbosity` (`Off` | `Lifecycle` (default) | `Diagnostic`) gates MLS lifecycle emission at runtime. Replaces the retired `mls-observability` Cargo feature. Identifier scrubbing is on by default via `TelemetryConfig::scrub_ids`.

### Safety Rules

- Core crates enforce `#![deny(unsafe_code)]` — zero unsafe allowed.
- FFI crate (`offline-protocol-uniffi`) allows unsafe for UniFFI scaffolding only.

### Build Profiles

- `dev`: debug, no optimization
- `release`: opt-level 3, LTO, stripped
- `minisize`: inherits release + opt-level "z", panic abort (for mobile binary size)

## Commit Convention

Conventional Commits: `<type>(<scope>): <subject>`

Types: `feat`, `fix`, `docs`, `test`, `refactor`, `perf`, `chore`

Scopes: `core`, `transport`, `router`, `reliability`, `services`, `protocol`, `uniffi`, `bindings`

## Code Style (Rust)

- `thiserror` for library errors, `Result<T, E>` everywhere (no `unwrap()` in library code)
- Prefer zero-copy (`&str` over `&String`, `bytes::Bytes` for byte handling)
- Avoid allocation when possible — no unnecessary `String`/`Vec` creation
- `tracing` for structured logging
- `serde` for all serialization
- `tokio` for async (though most core logic is synchronous)
- `pub(crate)` for internal APIs, `pub` only for truly public APIs
