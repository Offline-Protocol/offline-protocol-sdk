# offline-protocol-uniffi

[UniFFI](https://mozilla.github.io/uniffi-rs/) bindings crate for the [Offline Protocol SDK](https://github.com/Offline-Protocol/offline-protocol-sdk). Builds the `cdylib`/`staticlib` and FFI scaffolding from which the SDK's Swift, Kotlin, and Python bindings are generated.

This crate is not meant to be consumed directly:

- **Rust** consumers want the main [`offline-protocol`](https://crates.io/crates/offline-protocol) crate
- **React Native** (iOS/Android) apps want [`@offline-protocol/mesh-sdk`](https://www.npmjs.com/package/@offline-protocol/mesh-sdk)
- **Python** (macOS/Linux/Windows) apps want the [Python binding](https://github.com/Offline-Protocol/offline-protocol-sdk/blob/main/bindings/python/README.md)

The generated bindings carry FFI checksums of the exact library they were generated against, so the library and all three language bindings are regenerated together from one script — see [the repository](https://github.com/Offline-Protocol/offline-protocol-sdk#regenerate-bindings-after-a-udl-change) for the build workflow.

## License

Copyright © 2025-2026 Offline Protocol, Inc.

Dual-licensed: [AGPL-3.0-only](https://github.com/Offline-Protocol/offline-protocol-sdk/blob/main/LICENSE) for open-source use, or a [commercial license](https://github.com/Offline-Protocol/offline-protocol-sdk/blob/main/LICENSE-COMMERCIAL.md) for proprietary use.
