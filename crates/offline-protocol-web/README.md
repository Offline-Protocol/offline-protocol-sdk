# offline-protocol-web

The [Offline Protocol SDK](https://github.com/Offline-Protocol/offline-protocol-sdk) engine for the browser: a full mesh node, MLS end-to-end encryption included, compiled to WebAssembly with `wasm-bindgen`.

The engine runs inside a dedicated Web Worker. The page owns the sockets (a `BroadcastChannel` between tabs, a WebRTC data channel between devices) and moves bytes; this crate owns everything the protocol says about them: the identity preamble that proves who is on the other end of a link, the frame boundary, one link per address, and every message after that.

This crate is experimental. Its storage is in memory, so an identity lasts as long as the worker. Most consumers want the JavaScript package under [`bindings/web`](https://github.com/Offline-Protocol/offline-protocol-sdk/tree/main/bindings/web) rather than this crate directly; see the [web bridge document](https://github.com/Offline-Protocol/offline-protocol-sdk/blob/main/docs/bridges/web.md).

## License

Copyright © 2025-2026 Offline Protocol, Inc.

Dual-licensed: [AGPL-3.0-only](https://github.com/Offline-Protocol/offline-protocol-sdk/blob/main/LICENSE) for open-source use, or a [commercial license](https://github.com/Offline-Protocol/offline-protocol-sdk/blob/main/LICENSE-COMMERCIAL.md) for proprietary use.
