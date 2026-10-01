# offline-protocol-leaf

A constrained device that speaks the Offline Protocol as a real peer: a door lock, a sensor, a mains-powered relay. It parses frames, validates addressing, runs RFC 9420 MLS through [mls-rs](https://github.com/awslabs/mls-rs), and holds an end-to-end encrypted conversation with a phone under the same guarantees a phone gets. Same frames, same envelope, same trust gates, no second sealing path.

Provides:

- `LeafDevice`, a frame-level state machine: hand it an inbound message, get back the frames to send and what happened
- The never-committing member profile, where the phone creates the group and issues every commit and the device joins, opens, answers and persists
- `LeafStore`, one blob-storage seam a device implements over its secure key storage, with persist-before-emit enforced rather than documented
- Key package minting with the backdated `not_before` and supplied timestamp a device needs to pair at all

Four obligations this crate cannot discharge for you: a **time source** at pairing (every entry point takes `now_unix_secs`, because a device that lets an MLS library read a clock it does not have stamps 1970 and is refused as expired), **real entropy** (this crate registers no `getrandom` backend on purpose; wire the symbol to the part's hardware source), **durable storage** (`LeafStore` must be atomic per entry, because a ratchet state rolled back by a power cut reuses an AEAD nonce), and **authorization** (a session proves who a peer is and never that the owner meant them, since any address in radio range can complete a pairing, so firmware decides when the radio accepts one and what a given peer may actuate).

Like [`offline-protocol-core`](https://crates.io/crates/offline-protocol-core) and [`offline-protocol-sealed`](https://crates.io/crates/offline-protocol-sealed), this crate compiles for bare-metal targets with `--no-default-features` (add `--features bare-metal-rng`). CI builds and lints it for the first four of these:

| Chip family | Rust target | This crate compiles | Inside an esp-hal 1.2.2 firmware |
|---|---|---|---|
| Cortex-M33 (for example Silicon Labs xG24) | `thumbv8m.main-none-eabihf` | Yes | Not applicable |
| ESP32-C6, ESP32-H2 | `riscv32imac-unknown-none-elf` | Yes | Type-checks (checked by hand) |
| ESP32-C3, ESP32-C2 | `riscv32imc-unknown-none-elf` | Yes | Refused: mls-rs and esp-hal select conflicting portable-atomic backends (checked by hand) |
| Cortex-M0 and M0+ (for example RP2040) | `thumbv6m-none-eabi` | Yes | Not applicable |
| ESP32, ESP32-S3 | Xtensa, outside rustup | Not tried | Not tried |
| ESP32-S2 | Xtensa, outside rustup | Not tried | Not tried; expected to hit the same refusal, since it has no compare-and-swap and esp-hal selects the same backend for it |

The ESP32-C3, ESP32-C2 and Cortex-M0 targets have no atomic compare-and-swap, so on them the firmware must link a [critical-section](https://crates.io/crates/critical-section) implementation. Build the store handle with `shared_store`, which works on every target.

The last column was checked once by hand, with a scratch firmware crate that depends on esp-hal 1.2.2 and on this crate, running `cargo check`. CI does not repeat it. The third column is what CI gates.

Compiling is all this table claims. Nothing here has been linked into a firmware image or run on a board, and a device still owes a radio, a flash driver behind `LeafStore`, a hardware entropy source behind `getrandom`, and a time source at pairing.

The crate also builds for a WebAssembly host, on the WASI target `wasm32-wasip1`, and CI gates both halves there: the `no_std` build with no `bare-metal-rng`, because on WASI `getrandom` reads the runtime's `random_get` and there is no symbol for the host to register, and the `std` build through [`examples/wasi_host_shim.rs`](examples/wasi_host_shim.rs), a shim that takes the time and the frames from the runtime over its standard streams. `wasm32-unknown-unknown` is not claimed: on that target the pinned mls-rs enables `getrandom`'s `js` feature, which is selected ahead of a host-registered backend, so a green build there says nothing about a non-browser host. Entropy on WASI is the runtime's, and every key is exactly as strong as the runtime's source.

This crate is for firmware. Applications on a phone want the main [`offline-protocol`](https://crates.io/crates/offline-protocol) crate instead, or the [React Native](https://www.npmjs.com/package/@offline-protocol/mesh-sdk) or [Python](https://github.com/Offline-Protocol/offline-protocol-sdk/blob/main/bindings/python/README.md) bindings.

## License

Copyright © 2025-2026 Offline Protocol, Inc.

Dual-licensed: [AGPL-3.0-only](https://github.com/Offline-Protocol/offline-protocol-sdk/blob/main/LICENSE) for open-source use, or a [commercial license](https://github.com/Offline-Protocol/offline-protocol-sdk/blob/main/LICENSE-COMMERCIAL.md) for proprietary use.
