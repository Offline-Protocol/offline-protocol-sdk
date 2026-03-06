# CLAUDE.md

This file provides guidance to Claude Code (claude.ai/code) when working with code in this repository.

## Project Overview

Offline Protocol SDK: an offline-first messaging protocol in Rust with multi-transport switching (BLE, WiFi Direct, Internet), mesh networking, and automatic MLS end-to-end encryption (RFC 9420). Exposed to iOS/Android/React Native via UniFFI bindings.

## Common Commands

```bash
# Build
cargo build --workspace
cargo build --workspace --release

# Test
cargo test --workspace                          # all tests
cargo test --package offline-protocol-core      # single crate
cargo test test_message_creation                # single test
cargo test -- --nocapture                       # with stdout

# Lint & format (must pass before commits)
cargo clippy --workspace -- -D warnings
cargo fmt --workspace
cargo fmt --workspace -- --check                # check only

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

Prerequisites: `cargo install uniffi --features="cli"`, `cargo install cargo-ndk`, Android NDK, Xcode.

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
- **Protocol control messages**: internal prefix convention (`__MLS_KEY_PKG__`, `__MLS_WELCOME__`, `__MLS_ENC__`, etc.) in `crates/offline-protocol/src/protocol.rs`. Service messages use `__SVC_DISC_Q__`, `__SVC_DISC_R__`, `__SVC_REQ__`, `__SVC_RESP__` prefixes in `crates/offline-protocol-services/src/payloads.rs`.
- **Event-driven**: `OfflineProtocol` emits events (MessageReceived, PeerDiscovered, TransportChanged, etc.) via `EventCallback`.
- **Feature flag**: `mls-observability` in `offline-protocol` crate enables detailed MLS lifecycle events.

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
