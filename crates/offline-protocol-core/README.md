# offline-protocol-core

Core types and data structures for the [Offline Protocol SDK](https://github.com/Offline-Protocol/offline-protocol-sdk) — the foundation layer every other crate in the workspace builds on.

Provides:

- The `Message` envelope and its identifiers (`UserId`, `AppId`), TTL and hop-count bookkeeping, and timestamps
- The compact binary wire codec (wire v1), negotiated per peer with JSON as the permanent fallback format

This crate is an internal layer of the SDK. Most Rust consumers want the main [`offline-protocol`](https://crates.io/crates/offline-protocol) crate instead, and app developers want the [React Native](https://www.npmjs.com/package/@offline-protocol/mesh-sdk) or [Python](https://github.com/Offline-Protocol/offline-protocol-sdk/blob/main/bindings/python/README.md) bindings.

## License

Copyright © 2025-2026 Offline Protocol, Inc.

Dual-licensed: [AGPL-3.0-only](https://github.com/Offline-Protocol/offline-protocol-sdk/blob/main/LICENSE) for open-source use, or a [commercial license](https://github.com/Offline-Protocol/offline-protocol-sdk/blob/main/LICENSE-COMMERCIAL.md) for proprietary use.
