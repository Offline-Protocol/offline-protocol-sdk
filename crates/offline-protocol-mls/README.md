# offline-protocol-mls

MLS (Message Layer Security, [RFC 9420](https://www.rfc-editor.org/rfc/rfc9420)) integration for end-to-end encryption in the [Offline Protocol SDK](https://github.com/Offline-Protocol/offline-protocol-sdk), built on [OpenMLS](https://github.com/openmls/openmls).

Provides:

- `MlsManager` — key package lifecycle, 1:1 session establishment, and group encryption state
- The `MlsStorage` trait — a platform-agnostic secure storage interface that apps back with iOS Keychain, Android Keystore, or an equivalent
- Session and group encrypt/decrypt used by the protocol engine's automatic end-to-end encryption

This crate is an internal layer of the SDK. Most Rust consumers want the main [`offline-protocol`](https://crates.io/crates/offline-protocol) crate instead, which drives key exchange and encryption automatically — see the [MLS integration guide](https://github.com/Offline-Protocol/offline-protocol-sdk/blob/main/docs/mls-integration.md).

## License

Copyright © 2025-2026 Offline Protocol, Inc.

Dual-licensed: [AGPL-3.0-only](https://github.com/Offline-Protocol/offline-protocol-sdk/blob/main/LICENSE) for open-source use, or a [commercial license](https://github.com/Offline-Protocol/offline-protocol-sdk/blob/main/LICENSE-COMMERCIAL.md) for proprietary use.
