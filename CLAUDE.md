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
| `offline-protocol-sealed`: the envelope codec, `derive_address`, canonical signing payloads, ratchet constants, the 1:1 control-frame prefixes, `KeyPackagePayload`, the identity assertion codec | ADR [0022](docs/adr/0022-one-sealed-layer-shared-with-the-leaf.md) (also dual std/no_std, and the one home for each) |
| `offline-protocol-leaf`: anything a device does at pairing, on a frame, or with its store | ADR [0021](docs/adr/0021-a-leaf-node-speaks-mls.md) and [docs/spec/leaf-provisioning.md](docs/spec/leaf-provisioning.md) (a time source, real entropy, durable-before-emit and authorization are obligations, not suggestions) |
| Replicated documents: the store, sync frames, attachments | [docs/spec/data-sync.md](docs/spec/data-sync.md), [the replication state machine](docs/state-machines/data-replication.md), ADR [0018](docs/adr/0018-data-layer-engine-and-storage-seams.md) and [0019](docs/adr/0019-remote-document-imports-are-contained-not-trusted.md) |
| Any binding: Swift, Kotlin, Python, TypeScript | [docs/bridges/](docs/bridges/README.md) |
| The Swift package or the Android library: what is in them, how they are built | ADR [0025](docs/adr/0025-native-packages-are-assembled-in-place.md), [bindings/swift](bindings/swift/README.md), [bindings/kotlin](bindings/kotlin/README.md) |
| The telemetry pipe: what it sends, when, the queue, the uploader | [docs/telemetry.md](docs/telemetry.md), the [network egress](docs/security/threat-model.md#network-egress) section of the threat model, and [docs/privacy.md](docs/privacy.md) |

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

# Bare metal. `offline-protocol-core`, `offline-protocol-sealed` and
# `offline-protocol-leaf` are dual std/no_std and CI gates all three no_std
# halves; nothing else in the workspace compiles without `std`, so a stray
# `use std::` in one of them only fails here. So does an mls-rs error
# formatted with `{}`: mls-rs implements Display only under std.
# CI gates four bare-metal targets: the Cortex-M33, RISC-V with atomics
# (ESP32-C6/H2), and two with no compare-and-swap at all (ESP32-C3/C2 and
# Cortex-M0), plus WASI for the leaf (below).
# CI also runs `cargo build` on each, because clippy never reaches the atomics
# fallbacks' inline assembly; swap `clippy` for `build` to match it exactly.
TARGETS=(thumbv8m.main-none-eabihf riscv32imac-unknown-none-elf
         riscv32imc-unknown-none-elf thumbv6m-none-eabi)
rustup target add "${TARGETS[@]}"
for target in "${TARGETS[@]}"; do
    for crate in offline-protocol-core offline-protocol-sealed; do
        cargo clippy -p "$crate" --no-default-features \
            --target "$target" -- -D warnings
    done
    # The leaf crate needs a getrandom backend selected; the firmware registers one.
    cargo clippy -p offline-protocol-leaf --no-default-features \
        --features bare-metal-rng --target "$target" -- -D warnings
done
# WASI: the leaf's no_std half with NO bare-metal-rng (the target selects
# getrandom's `random_get` backend ahead of any feature) and its std half
# through the host shim example. wasm32-unknown-unknown is not claimed: there
# mls-rs enables getrandom's `js` feature, which shadows a host-registered
# backend, so a green build proves nothing about a non-browser host.
rustup target add wasm32-wasip1
cargo build -p offline-protocol-leaf --no-default-features --target wasm32-wasip1
cargo clippy -p offline-protocol-leaf --no-default-features \
    --target wasm32-wasip1 -- -D warnings
cargo build -p offline-protocol-leaf --example wasi_host_shim --target wasm32-wasip1
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

### Native packages

```bash
# The Swift package: build the Rust library for the simulator, assemble the
# package from the bridge sources, run every suite, then build an application
# against it. `swift test` cannot do this: it builds for the Mac, where the
# bridge does not compile. Under build/, not target/, which CI caches badly.
IPHONEOS_DEPLOYMENT_TARGET="$(bash scripts/ios-deployment-target.sh)" \
  cargo build --release --target aarch64-apple-ios-sim --package offline-protocol-uniffi
bash scripts/package-swiftpm-xcframework.sh build/swiftpm \
  target/aarch64-apple-ios-sim/release/liboffline_protocol_uniffi.a
rm -rf build/offline-protocol-swift
bash scripts/assemble-swift-package.sh --output build/offline-protocol-swift \
  --version 0.0.0 --bridge-tests \
  --xcframework-path build/swiftpm/offline_protocolFFI.xcframework
(cd build/offline-protocol-swift && xcodebuild test -scheme OfflineProtocolSDK \
  -destination "platform=iOS Simulator,id=$(bash ../../scripts/pick-ios-simulator.sh)")
(cd bindings/swift/consumer-check && xcodebuild test -scheme ConsumerCheck-Package \
  -destination "platform=iOS Simulator,id=$(bash ../../../scripts/pick-ios-simulator.sh)")

# The Android library (JDK 17, Gradle 8.9, no wrapper in the repository).
cd bindings/kotlin
gradle -PVERSION_NAME=0.0.0 :offline-protocol-android:testDebugUnitTest \
  :offline-protocol-android:publishAllPublicationsToStagingRepository
gradle -p consumer-check -PVERSION_NAME=0.0.0 :app:assembleRelease
```

**`cargo test` proves nothing about the bridges.** Each binding has its own
tests; see [docs/bridges/](docs/bridges/README.md#c9-bridge-behaviour-is-not-covered-by-the-rust-test-suite).

## Architecture

### Dependency graph (bottom up)

```
offline-protocol-core          Message, UserId, Address, AppId, TTL, HopCount, wire codec
    |
offline-protocol-sealed        EncryptedMessage + compact codec, derive_address, canonical
                               signing payloads, ratchet constants, 1:1 control-frame
                               prefixes, KeyPackagePayload, identity assertion codec
                               (dual std/no_std)
    |
offline-protocol-transport     Transport trait + BLE/WiFi Direct/Internet impls, metrics
offline-protocol-reliability   AckManager, RetryQueue, Deduplicator, AckOptimizer
offline-protocol-mls           MlsManager, MlsStorage trait, session & group encryption
offline-protocol-services      MeshServices: registry, discovery (gossip), request/response
offline-protocol-data          DataDoc: replicated documents (CRDT), caps, compaction
offline-protocol-telemetry-wire  Batch envelope + typed event payloads shared with the
                               telemetry ingest (a copy of the ingest's own wire crate)
    |
offline-protocol-router        DORS transport selector, relay role vocabulary
    |
offline-protocol               Engine: OfflineProtocol, ProtocolConfig, TransportManager, events,
                               and telemetry/pipe: the one background thread and the one
                               socket in the workspace (feature `telemetry-pipe`, default on)
    |
offline-protocol-uniffi        UniFFI bindings (cdylib + staticlib)
offline-protocol-bench         Criterion benchmarks

offline-protocol-leaf          A constrained device as a never-committing MLS member.
                               Sits on core + sealed only, never on the engine
                               (dual std/no_std)
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
- **Telemetry** is not an extension point. `OfflineProtocol::enable_telemetry`
  starts the pipe in `crates/offline-protocol/src/telemetry/pipe/`, which is
  the one production sink; `install_telemetry_sink` is crate-private. The
  emit path never allocates for an event the pipe drops (`classify.rs`), and
  the uploader never holds a lock across I/O. `TelemetryConfig::mls_verbosity`
  gates MLS lifecycle emission at runtime. `scrub_ids` (default on) hashes the
  peer and group ids on MLS lifecycle records, which the pipe reads only to
  pair a handshake's start with its end; it never changes what `on_event`
  delivers or what is uploaded.
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
  (`offline_protocol::storage_conformance::run` for protocol state,
  `offline_protocol::mls_storage_conformance::run` for MLS material).
  Swappable storage is the data layer's ranked-first property, and an adapter
  that returns `Ok` from every method can still lose overwrites, merge
  categories or tear a concurrent overwrite
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
- **`offline-protocol-leaf` reaches `alloc::sync` in `src/shared.rs` only.**
  Every other file takes `Arc` from `crate::shared`. A target with no
  compare-and-swap (ESP32-C3, Cortex-M0) has no `alloc::sync`, and a host
  always does, so a direct import compiles and tests green everywhere a
  developer works. `only_one_file_names_the_counted_pointer` refuses it on the
  host and the `embedded-core` job on the targets
  ([ADR 0021](docs/adr/0021-a-leaf-node-speaks-mls.md#the-store-handle-follows-the-target)).
- **Never write a second envelope codec, address derivation, canonical signing
  payload, ratchet constant, 1:1 control-frame prefix literal, key package
  payload or identity assertion split.** Each exists once, in `offline-protocol-sealed`;
  everything else re-exports or delegates. The tempting route is a harness or
  fixture writing three lines rather than taking the dependency, which is
  exactly what `tools/mls-interop` did before this crate existed, and what
  `the_interop_harness_uses_this_crate_rather_than_its_own_copies` now refuses
  ([ADR 0022](docs/adr/0022-one-sealed-layer-shared-with-the-leaf.md)).
- **The rollup and session-summary wire types exist only inside the telemetry
  pipe.** The free event API is per-event and unaggregated by design; a name
  that is both a wire aggregate and an `Event` would hand the product's
  differentiator to `on_event`. `aggregate_wire_types_never_reach_the_free_event_api`
  pins it, and greps `src/` outside the pipe for the two literals.
- **Every binding fills the telemetry platform fields itself and owns the
  lifecycle** ([C12](docs/bridges/README.md#c12-telemetry-is-callback-free-and-the-binding-fills-the-platform));
  `every_bridge_reads_the_telemetry_config_section` reads the UDL record and
  the three parsers.
- **Never move or copy a bridge source, and never write down in a native
  package a version, an SDK level, a deployment target or a permission the
  React Native package declares.** The Swift package and the Android library
  are built from the files under `bindings/react-native/{ios,android}`, where
  about fifty Rust guards read them by path, and four of those guards skip a
  file that is missing instead of failing. The packages read those values
  from the module, and the Android library is built by the toolchain the
  module's tests run on. A shared source that starts to need React breaks
  both packages, and the `Swift Package` and `Android Library` jobs are what
  say so
  ([ADR 0025](docs/adr/0025-native-packages-are-assembled-in-place.md),
  [C13](docs/bridges/README.md#c13-a-shared-bridge-source-compiles-without-react)).
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

Scopes: `core`, `sealed`, `leaf`, `transport`, `router`, `reliability`,
`services`, `protocol`, `uniffi`, `bindings`

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
