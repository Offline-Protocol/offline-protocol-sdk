# offline-protocol-services

Mesh services layer for the [Offline Protocol SDK](https://github.com/Offline-Protocol/offline-protocol-sdk): service registry, discovery, and request/response over the mesh.

Provides:

- `MeshServices` — register named services a device offers to the mesh
- Gossip-based service discovery across peers
- A request/response exchange on top of the mesh's store-and-forward delivery

This crate is an internal layer of the SDK. Most Rust consumers want the main [`offline-protocol`](https://crates.io/crates/offline-protocol) crate instead — see the [service discovery guide](https://github.com/Offline-Protocol/offline-protocol-sdk/blob/main/docs/service-discovery.md).

## License

Copyright © 2025-2026 Offline Protocol, Inc.

Dual-licensed: [AGPL-3.0-only](https://github.com/Offline-Protocol/offline-protocol-sdk/blob/main/LICENSE) for open-source use, or a [commercial license](https://github.com/Offline-Protocol/offline-protocol-sdk/blob/main/LICENSE-COMMERCIAL.md) for proprietary use.
