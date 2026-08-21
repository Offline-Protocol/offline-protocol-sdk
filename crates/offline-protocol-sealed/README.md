# offline-protocol-sealed

The pieces both halves of a sealed conversation must agree on, in a form a bare-metal leaf node can link: the encrypted-envelope wire codec, address derivation, the canonical signing-payload construction, and the control frames a pair exchanges.

Provides:

- `EncryptedMessage` and its compact binary codec, the wire form of every sealed payload in the protocol
- `derive_address`, the SDK's one derivation of an address from an identity key
- `canonical_payload`, the domain-separated, length-prefixed construction every signature in the protocol is taken over
- The sender-ratchet and leaf key-package constants that a phone and a leaf must configure identically
- The six control-frame prefixes a 1:1 sealed conversation is carried on, and `KeyPackagePayload`, the body that advertises what a peer can parse

Like [`offline-protocol-core`](https://crates.io/crates/offline-protocol-core), this crate compiles for bare-metal targets with `--no-default-features`.

This crate is an internal layer of the SDK. Most Rust consumers want the main [`offline-protocol`](https://crates.io/crates/offline-protocol) crate instead, and app developers want the [React Native](https://www.npmjs.com/package/@offline-protocol/mesh-sdk) or [Python](https://github.com/Offline-Protocol/offline-protocol-sdk/blob/main/bindings/python/README.md) bindings.

## License

Copyright © 2025-2026 Offline Protocol, Inc.

Dual-licensed: [AGPL-3.0-only](https://github.com/Offline-Protocol/offline-protocol-sdk/blob/main/LICENSE) for open-source use, or a [commercial license](https://github.com/Offline-Protocol/offline-protocol-sdk/blob/main/LICENSE-COMMERCIAL.md) for proprietary use.
