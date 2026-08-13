# offline-protocol-router

Routing layer for the [Offline Protocol SDK](https://github.com/Offline-Protocol/offline-protocol-sdk): transport selection, relay management, and mesh routing.

Provides:

- **DORS** (Dynamic Offline Relay Switch) — multi-factor transport scoring (signal strength, congestion, bandwidth, battery, reliability, capacity) with hysteresis, cooldown, and a stability window to prevent transport flapping
- `RelayManager` and `PathSelector` — relay coordination and path choice for multi-hop delivery
- Gossip-based routing state exchange

This crate is an internal layer of the SDK. Most Rust consumers want the main [`offline-protocol`](https://crates.io/crates/offline-protocol) crate instead — see the [DORS deep dive](https://github.com/Offline-Protocol/offline-protocol-sdk/blob/main/docs/dors.md) and [DORS configuration guide](https://github.com/Offline-Protocol/offline-protocol-sdk/blob/main/docs/dors-configuration.md).

## License

Copyright © 2025-2026 Offline Protocol, Inc.

Dual-licensed: [AGPL-3.0-only](https://github.com/Offline-Protocol/offline-protocol-sdk/blob/main/LICENSE) for open-source use, or a [commercial license](https://github.com/Offline-Protocol/offline-protocol-sdk/blob/main/LICENSE-COMMERCIAL.md) for proprietary use.
