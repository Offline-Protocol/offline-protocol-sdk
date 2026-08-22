# offline-protocol-leaf

A constrained device that speaks the Offline Protocol as a real peer: a door lock, a sensor, a mains-powered relay. It parses frames, validates addressing, runs RFC 9420 MLS through [mls-rs](https://github.com/awslabs/mls-rs), and holds an end-to-end encrypted conversation with a phone under the same guarantees a phone gets. Same frames, same envelope, same trust gates, no second sealing path.

Provides:

- `LeafDevice`, a frame-level state machine: hand it an inbound message, get back the frames to send and what happened
- The never-committing member profile, where the phone creates the group and issues every commit and the device joins, opens, answers and persists
- `LeafStore`, one blob-storage seam a device implements over its secure key storage, with persist-before-emit enforced rather than documented
- Key package minting with the backdated `not_before` and supplied timestamp a device needs to pair at all

Four obligations this crate cannot discharge for you: a **time source** at pairing (every entry point takes `now_unix_secs`, because a device that lets an MLS library read a clock it does not have stamps 1970 and is refused as expired), **real entropy** (this crate registers no `getrandom` backend on purpose; wire the symbol to the part's hardware source), **durable storage** (`LeafStore` must be atomic per entry, because a ratchet state rolled back by a power cut reuses an AEAD nonce), and **authorization** (a session proves who a peer is and never that the owner meant them, since any address in radio range can complete a pairing, so firmware decides when the radio accepts one and what a given peer may actuate).

Like [`offline-protocol-core`](https://crates.io/crates/offline-protocol-core) and [`offline-protocol-sealed`](https://crates.io/crates/offline-protocol-sealed), this crate compiles for bare-metal targets with `--no-default-features` (add `--features bare-metal-rng`).

This crate is for firmware. Applications on a phone want the main [`offline-protocol`](https://crates.io/crates/offline-protocol) crate instead, or the [React Native](https://www.npmjs.com/package/@offline-protocol/mesh-sdk) or [Python](https://github.com/Offline-Protocol/offline-protocol-sdk/blob/main/bindings/python/README.md) bindings.

## License

Copyright © 2025-2026 Offline Protocol, Inc.

Dual-licensed: [AGPL-3.0-only](https://github.com/Offline-Protocol/offline-protocol-sdk/blob/main/LICENSE) for open-source use, or a [commercial license](https://github.com/Offline-Protocol/offline-protocol-sdk/blob/main/LICENSE-COMMERCIAL.md) for proprietary use.
