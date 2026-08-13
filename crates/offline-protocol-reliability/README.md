# offline-protocol-reliability

Reliability layer for the [Offline Protocol SDK](https://github.com/Offline-Protocol/offline-protocol-sdk): delivery acknowledgements, retries, and deduplication.

Provides:

- `AckManager` — tracks per-message delivery acknowledgements
- `RetryQueue` — retransmission with exponential backoff
- `Deduplicator` — drops duplicate frames arriving over multiple paths or retries
- `AckOptimizer` — batches and prunes ACK traffic

This crate is an internal layer of the SDK. Most Rust consumers want the main [`offline-protocol`](https://crates.io/crates/offline-protocol) crate instead, which wires these pieces into its delivery pipeline — see the [message delivery guide](https://github.com/Offline-Protocol/offline-protocol-sdk/blob/main/docs/message-delivery.md).

## License

Copyright © 2025-2026 Offline Protocol, Inc.

Dual-licensed: [AGPL-3.0-only](https://github.com/Offline-Protocol/offline-protocol-sdk/blob/main/LICENSE) for open-source use, or a [commercial license](https://github.com/Offline-Protocol/offline-protocol-sdk/blob/main/LICENSE-COMMERCIAL.md) for proprietary use.
