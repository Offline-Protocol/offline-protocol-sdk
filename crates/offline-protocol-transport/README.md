# offline-protocol-transport

Transport abstraction layer for the [Offline Protocol SDK](https://github.com/Offline-Protocol/offline-protocol-sdk).

Provides:

- The `Transport` trait that all transports implement, plus `TransportMetrics` for the scoring signals DORS routing consumes
- Queue-based implementations for BLE, WiFi Direct, Internet, Reticulum, and Nostr
- `MockTransport` for tests

**These transports are I/O-free protocol engines** — they queue outbound frames and accept inbound bytes, but never open a socket or touch a radio. A *platform bridge* does the actual I/O: it drains each transport's outbound queue, performs the send, reports the outcome, and injects inbound bytes. The bridge contract is documented in this crate's API docs. The SDK's [React Native](https://www.npmjs.com/package/@offline-protocol/mesh-sdk) and [Python](https://github.com/Offline-Protocol/offline-protocol-sdk/blob/main/bindings/python/README.md) bindings ship ready-made bridges; direct Rust consumers write their own.

This crate is an internal layer of the SDK. Most Rust consumers want the main [`offline-protocol`](https://crates.io/crates/offline-protocol) crate instead.

## License

Copyright © 2025-2026 Offline Protocol, Inc.

Dual-licensed: [AGPL-3.0-only](https://github.com/Offline-Protocol/offline-protocol-sdk/blob/main/LICENSE) for open-source use, or a [commercial license](https://github.com/Offline-Protocol/offline-protocol-sdk/blob/main/LICENSE-COMMERCIAL.md) for proprietary use.
