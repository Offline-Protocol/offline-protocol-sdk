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
- **Sealed rich payload (inside the MLS plaintext)**: rich message extras (reply_context, rich media_metadata incl. key/iv secrets, forward_info) only ever travel as a `__RICH_V1__` + JSON body wrapped around the text *before* encryption, negotiated via `rich_versions` in the key package (`RICH_PAYLOAD_V1`). Sealed in `protocol/send.rs::prepare_outbound_content` (the single chokepoint for fresh sends and pending flushes; `PendingMessage.rich` preserves option-borne provenance through the queue), restored in `protocol/receive.rs::apply_decrypted_content` right after the outer-field strip — sealed body is authoritative, outer copies are wiped. The body also carries a sealed copy of the outer `content_type` hint (additive field; absent → outer stands, `FileChunk` refused on restore like at the send boundary); fresh sends with a non-Text hint seal a hint-only body even without extras. Forwards (`forward_message`) seal their attribution + the original `media_metadata` as extras toward capable recipients — the only way forwarded cloud media keeps its `encryption_key`/`iv` — with the cleartext outer copies kept as the legacy fallback (secrets stripped at the wire chokepoint; sealed restore overwrites them wholesale). Non-capable recipients: extras silently dropped, never cleartext. **Groups**: the same `__RICH_V1__` body seals into the group MLS plaintext (`group_mesh.rs::send_group_message_inner`; parse shared via `RichPayloadV1::parse_sealed`, applied on all three inbound paths — mesh, buffered drain, relay), but gated on *every* other member being in `peer_rich_payload` (one ciphertext serves the whole group; an unknown-capability member — e.g. joined via someone else's Welcome — fails the gate closed and extras drop). Surfaced via additive `media_metadata`/`content_type` fields on `GroupMessageReceived` (telemetry scrubber redacts the secrets); public surface `send_group_message_with`/`GroupSendOptions` + `forward_message_to_group` (core; not yet exposed over UniFFI beyond the pre-existing forward API). Per-peer capability sets (`env_versions`/`rich_versions`) persist across restarts as `PeerCapabilities` (`peer_capabilities` storage key — separate from the key-package cache, which is deleted on session creation) and are restored in `initialize_mls` before `start()` flushes pending sends; `wire_versions` stays in-memory by design (hop-local, re-exchanged on connect). Gated by `EncryptionConfig::rich_payload_enabled` (default on), independent of the other two kill switches; inbound parsing unconditional (parse failure → raw text + warning). Public surface: `send_message_with`/`SendMessageOptions` (core), `send_message_rich` (UniFFI), rich params on RN `sendMessage`. Boundary validation in `send_message_with`: rejects `ContentType::FileChunk` and rich extras >32 KiB serialized (`MAX_RICH_EXTRAS_BYTES`; capped at the boundary, not seal time, so a flush re-seal can never fail the cap and re-queue forever).
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
