# CLAUDE.md

Guidance for Claude Code (claude.ai/code) working in this repository.

## Project overview

Offline Protocol SDK: an offline-first messaging protocol in Rust with
multi-transport switching (BLE, Wi-Fi Direct, Reticulum, Nostr, internet relay),
mesh networking, and automatic MLS end-to-end encryption (RFC 9420). Exposed to
iOS, Android, React Native and Python via UniFFI bindings.

## Where the knowledge lives

This file is repository instructions. The design knowledge lives in documents
that are versioned, reviewable, and readable by people who are not an agent.
**Read the relevant one before changing behaviour in its area.**

| If you are touching | Read first |
|---------------------|------------|
| Wire encoding, envelopes, control frames, negotiation | [docs/spec/](docs/spec/README.md) |
| Anything security-relevant | [docs/security/threat-model.md](docs/security/threat-model.md) |
| Acknowledgement, retry, session, group or transport behaviour | [docs/state-machines/](docs/state-machines/README.md) |
| A decision that looks odd or over-engineered | [docs/adr/](docs/adr/README.md) |
| `offline-protocol-core`: adding an import, a dependency, or a constructor | ADR [0020](docs/adr/0020-core-compiles-without-std.md) (it is dual std/no_std) |
| `offline-protocol-sealed`: the envelope codec, `derive_address`, canonical signing payloads, ratchet constants, the 1:1 control-frame prefixes, `KeyPackagePayload` | ADR [0022](docs/adr/0022-one-sealed-layer-shared-with-the-leaf.md) (also dual std/no_std, and the one home for each) |
| Replicated documents: the store, sync frames, attachments | [docs/spec/data-sync.md](docs/spec/data-sync.md), [the replication state machine](docs/state-machines/data-replication.md), ADR [0018](docs/adr/0018-data-layer-engine-and-storage-seams.md) and [0019](docs/adr/0019-remote-document-imports-are-contained-not-trusted.md) |
| Any binding: Swift, Kotlin, Python, TypeScript | [docs/bridges/](docs/bridges/README.md) |

The ADR index is the fastest route to "why is this like this". If you are about
to simplify something that looks redundant, check there first: several shapes in
this codebase are correct only as a whole, and the ADR names the failure that
partial versions cause.

## Common commands

```bash
# Verify loop (lint subsumes typecheck; don't run a separate `cargo build` first,
# it only adds a third artifact set including the expensive uniffi cdylib link)
cargo clippy --workspace -- -D warnings
cargo test --workspace --lib                    # all unit tests; skips empty per-crate doctest passes

# Test
cargo test --workspace                          # full run incl. doctests (what CI runs)
cargo test --package offline-protocol-core      # single crate
cargo test test_message_creation                # single test
cargo test -- --nocapture                       # with stdout

# Build (only when you need compiled artifacts, e.g. the uniffi cdylib)
cargo build --workspace
cargo build --workspace --release

# Format (must pass before commits; fmt takes --all, not --workspace)
cargo fmt --all
cargo fmt --all -- --check

# Docs (CI gates this under -D warnings; without the flag broken intra-doc links
# only print and still exit 0, so the plain command passes what CI fails)
RUSTDOCFLAGS="-D warnings" cargo doc --workspace --no-deps

# Benchmarks (Criterion)
cargo bench --package offline-protocol-bench

# Bare metal. `offline-protocol-core` and `offline-protocol-sealed` are dual
# std/no_std and CI gates both no_std halves; nothing else in the workspace
# compiles either without `std`, so a stray `use std::` in one of them only
# fails here.
rustup target add thumbv8m.main-none-eabihf
for crate in offline-protocol-core offline-protocol-sealed; do
    cargo clippy -p "$crate" --no-default-features \
        --target thumbv8m.main-none-eabihf -- -D warnings
done
./tools/embedded-footprint/measure.sh    # flash/RAM cost of the protocol layer
```

### UniFFI and mobile builds

```bash
# Regenerate EVERY binding after a UDL change: Swift, Kotlin and Python together.
# The three are one artifact set from one UDL, so there is one script. A partial
# regeneration fails at runtime, not at build time.
./scripts/generate-bindings.sh

cd bindings/react-native
npm run build:uniffi:all          # all platforms
npm run build:uniffi:ios          # iOS only
npm run build:uniffi:android      # Android only
npm run generate:bindings         # wrapper for ../../scripts/generate-bindings.sh
```

Prerequisites: `cargo install uniffi --version 0.30.0 --features cli --locked`
(must match the workspace `uniffi = "0.30"` pin), Android NDK
(`ANDROID_NDK_HOME`), Xcode.

**`cargo test` proves nothing about the bridges.** Each binding has its own
tests; see [docs/bridges/](docs/bridges/README.md#c9-bridge-behaviour-is-not-covered-by-the-rust-test-suite).

## Architecture

### Dependency graph (bottom up)

```
offline-protocol-core          Message, UserId, Address, AppId, TTL, HopCount, wire codec
    |
offline-protocol-sealed        EncryptedMessage + compact codec, derive_address, canonical
                               signing payloads, ratchet constants, 1:1 control-frame
                               prefixes, KeyPackagePayload (dual std/no_std)
    |
offline-protocol-transport     Transport trait + BLE/WiFi Direct/Internet impls, metrics
offline-protocol-reliability   AckManager, RetryQueue, Deduplicator, AckOptimizer
offline-protocol-mls           MlsManager, MlsStorage trait, session & group encryption
offline-protocol-services      MeshServices: registry, discovery (gossip), request/response
offline-protocol-data          DataDoc: replicated documents (CRDT), caps, compaction
    |
offline-protocol-router        DORS transport selector, relay role vocabulary
    |
offline-protocol               Engine: OfflineProtocol, ProtocolConfig, TransportManager, events
    |
offline-protocol-uniffi        UniFFI bindings (cdylib + staticlib)
offline-protocol-bench         Criterion benchmarks
```

### Key extension points

- **`Transport` trait** (`crates/offline-protocol-transport/src/traits.rs`): all
  transports implement it; uses `as_any()` for safe downcasting. `MockTransport`
  is available for tests.
- **`MlsStorage` trait** (`crates/offline-protocol-mls/src/storage.rs`):
  platform-agnostic secure storage. Apps implement it for iOS Keychain, Android
  Keystore, and so on.
- **`DataStore`** (`crates/offline-protocol/src/protocol/data.rs`): replicated
  documents over the storage seam. The backend is swappable at runtime via
  `DataConfig::storage` (one line, no rebuild), sealing sits above the
  adapter, and the CRDT engine stays inside `offline-protocol-data`.
- **`TelemetrySink`**: installed via
  `OfflineProtocol::install_telemetry_sink(sink, config)`.
  `TelemetryConfig::mls_verbosity` gates MLS lifecycle emission at runtime;
  identifier scrubbing is on by default.
- **`EventCallback`**: the engine emits events (MessageReceived,
  NeighborDiscovered, TransportSwitched, and so on). Events cross UniFFI as
  opaque JSON.

### Rules that are enforced by tests, not by the compiler

These fail silently if broken. Each is documented in full where it is linked.

- **Regenerate all bindings together** after a UDL change
  ([C1](docs/bridges/README.md#c1-regenerate-every-binding-together)).
- **The FFI error enum is append-only**; discriminants are positional
  ([C2](docs/bridges/README.md#c2-the-error-enum-is-append-only)).
- **Hand-mirrored constants** (relay-answer prefixes, one-shot event tags, the
  mesh wake task key, the protocol-state record ceiling) exist in up to four
  places across three languages, pinned either by per-language literal tests or
  by a Rust guard that reads the binding sources
  ([C5](docs/bridges/README.md#c5-hand-mirrored-constants-must-be-pinned-in-every-language)).
- **Adding a control-message prefix** means adding it to the registry that
  drives injection prevention
  ([spec](docs/spec/control-messages.md#reserved-prefix-registry)).
- **A storage adapter must pass the conformance suite**
  (`offline_protocol::storage_conformance::run`). Swappable storage is the
  data layer's ranked-first property, and an adapter that returns `Ok` from
  every method can still lose overwrites or merge categories
  ([C11](docs/bridges/README.md#c11-a-storage-adapter-is-a-supported-extension-point-and-is-verified)).
- **Never hand the CRDT engine bytes that did not come out of a sealed
  record.** Malformed imports can panic upstream, and `minisize` is
  `panic = "abort"`, so the AEAD tag is the real containment
  ([ADR 0018](docs/adr/0018-data-layer-engine-and-storage-seams.md)).
- **`offline-protocol-core` must keep compiling without `std`.** Its seven
  locally-declared dependencies cannot go back to `{ workspace = true }`
  (cargo silently ignores `default-features = false` on an inherited
  dependency), and a new `use std::` needs a `#[cfg(feature = "std")]`. The
  `embedded-core` CI job is the only thing that catches either
  ([ADR 0020](docs/adr/0020-core-compiles-without-std.md)).
- **`offline-protocol-sealed` must keep compiling without `std` too**, under
  the same two traps as core (local dependency declarations, no bare
  `use std::`), gated by the same `embedded-core` CI job
  ([ADR 0022](docs/adr/0022-one-sealed-layer-shared-with-the-leaf.md)).
- **Never write a second envelope codec, address derivation, canonical signing
  payload, ratchet constant, 1:1 control-frame prefix literal or key package
  payload.** Each exists once, in `offline-protocol-sealed`;
  everything else re-exports or delegates. The tempting route is a harness or
  fixture writing three lines rather than taking the dependency, which is
  exactly what `tools/mls-interop` did before this crate existed, and what
  `the_interop_harness_uses_this_crate_rather_than_its_own_copies` now refuses
  ([ADR 0022](docs/adr/0022-one-sealed-layer-shared-with-the-leaf.md)).
- **Never add a catch-all arm to a telemetry reason classifier that matches on
  an enum** (a classifier over an open wire string may, if the fallback returns
  a fixed token and never the input)
  ([ADR 0013](docs/adr/0013-exhaustive-privacy-classifier.md)).

### Safety rules

- Core crates enforce `#![deny(unsafe_code)]`. Zero unsafe allowed.
- The FFI crate (`offline-protocol-uniffi`) allows unsafe for UniFFI scaffolding
  only.

### Build profiles

- `dev`: debug, no optimization
- `release`: opt-level 3, LTO, stripped
- `minisize`: inherits release + opt-level "z", panic abort (mobile binary size)

## Commit convention

Conventional Commits: `<type>(<scope>): <subject>`

Types: `feat`, `fix`, `docs`, `test`, `refactor`, `perf`, `chore`

Scopes: `core`, `sealed`, `transport`, `router`, `reliability`, `services`,
`protocol`, `uniffi`, `bindings`

## Code style (Rust)

- `thiserror` for library errors, `Result<T, E>` everywhere, no `unwrap()` in
  library code
- Prefer zero-copy (`&str` over `&String`, `bytes::Bytes` for byte handling)
- Avoid allocation when possible
- `tracing` for structured logging
- `serde` for all serialization
- `tokio` for async, though most core logic is synchronous
- `pub(crate)` for internal APIs, `pub` only for truly public APIs

## Documentation style

- **No em dashes.** Use a comma, a colon, parentheses, or two sentences.
- Documents state invariants before mechanisms. The invariant survives a
  refactor; the function name does not.
- When a shape exists to prevent a specific failure, name the failure. Otherwise
  someone will simplify it back.
- Release notes go in `CHANGELOG.md` for the current release only; older
  releases are archived by series in [docs/changelog/](docs/changelog/README.md).
