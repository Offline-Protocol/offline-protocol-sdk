# offline-protocol

Main protocol engine of the [Offline Protocol SDK](https://github.com/Offline-Protocol/offline-protocol-sdk): an offline-first messaging protocol with intelligent multi-transport switching, mesh networking, and automatic end-to-end encryption.

This is the crate to depend on if you are consuming the SDK from Rust — it re-exports and orchestrates the lower layers (core types, transports, routing, reliability, MLS encryption, mesh services) behind one engine.

## Features

- **Multi-transport**: automatically switches between BLE, WiFi Direct, Internet, Reticulum, and Nostr relays via DORS (Dynamic Offline Relay Switch)
- **Mesh networking**: peer discovery, cluster formation, and bounded multi-hop message relay
- **End-to-end encryption**: automatic MLS (RFC 9420) for direct messages and groups, fail-closed by default
- **Reliability**: delivery ACKs, retries with exponential backoff, and deduplication built in
- **Event-driven**: the engine surfaces `MessageReceived`, `PeerDiscovered`, `TransportChanged`, and the rest of its lifecycle through an event callback

## What you must implement

The Rust crates are I/O-free protocol engines — they queue, route, encrypt, and select transports, but never open a socket or touch a radio. A *platform bridge* does the actual I/O: it drains each transport's outbound queue, performs the send, reports the outcome, and injects inbound bytes. If you consume this crate directly, you write that bridge yourself; the contract is documented in the [`offline-protocol-transport`](https://crates.io/crates/offline-protocol-transport) crate docs.

Ready-made bridges ship with the higher-level bindings instead:

- **React Native** (iOS/Android): [`@offline-protocol/mesh-sdk`](https://www.npmjs.com/package/@offline-protocol/mesh-sdk)
- **Python** (macOS/Linux/Windows): [Python binding guide](https://github.com/Offline-Protocol/offline-protocol-sdk/blob/main/bindings/python/README.md)

## Documentation

- [Architecture deep dive](https://github.com/Offline-Protocol/offline-protocol-sdk/blob/main/docs/architecture.md)
- [Configuration guide](https://github.com/Offline-Protocol/offline-protocol-sdk/blob/main/docs/configuration.md)
- [Message delivery](https://github.com/Offline-Protocol/offline-protocol-sdk/blob/main/docs/message-delivery.md) — ACK ladder, retries, offline park/push, group delivery reports
- [MLS encryption integration](https://github.com/Offline-Protocol/offline-protocol-sdk/blob/main/docs/mls-integration.md)
- [Full documentation index](https://github.com/Offline-Protocol/offline-protocol-sdk#documentation)

## License

Copyright © 2025-2026 Offline Protocol, Inc.

Dual-licensed: [AGPL-3.0-only](https://github.com/Offline-Protocol/offline-protocol-sdk/blob/main/LICENSE) for open-source use, or a [commercial license](https://github.com/Offline-Protocol/offline-protocol-sdk/blob/main/LICENSE-COMMERCIAL.md) for proprietary use. You may use the SDK under either license; you do not need both.
