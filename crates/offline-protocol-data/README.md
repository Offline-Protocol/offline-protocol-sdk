# offline-protocol-data

Replicated documents for the [Offline Protocol SDK](https://github.com/Offline-Protocol/offline-protocol-sdk): offline-first state that any space member can edit while disconnected, merging deterministically on reconnect.

Provides:

- `DataDoc` — a document of `map`, `list`, `text` and `counter` collections
- Deterministic merge (CRDT), so duplicate and out-of-order deltas are absorbed by construction
- Opaque byte deltas and version tokens, sized for the SDK's transports
- `export_json()` and `export_raw()` escape hatches, so an application can always leave

The CRDT engine is an implementation detail of this crate: no engine type appears in its public API, and none crosses the SDK's FFI surface.

This crate is an internal layer of the SDK. Most Rust consumers want the main [`offline-protocol`](https://crates.io/crates/offline-protocol) crate instead.

## License

Copyright © 2025-2026 Offline Protocol, Inc.

Dual-licensed: [AGPL-3.0-only](https://github.com/Offline-Protocol/offline-protocol-sdk/blob/main/LICENSE) for open-source use, or a [commercial license](https://github.com/Offline-Protocol/offline-protocol-sdk/blob/main/LICENSE-COMMERCIAL.md) for proprietary use.
