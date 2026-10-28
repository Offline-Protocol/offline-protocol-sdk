# offline-protocol-web

The [Offline Protocol SDK](https://github.com/Offline-Protocol/offline-protocol-sdk) engine for the browser: a full mesh node, MLS end-to-end encryption included, compiled to WebAssembly with `wasm-bindgen`.

The engine runs inside a dedicated Web Worker. JavaScript owns the sockets and moves bytes: the worker holds the `BroadcastChannel` between tabs and the WebSockets to the application's Nostr relays, and the page holds the WebRTC data channels and the Web Bluetooth links between devices. This crate owns everything the protocol says about them: the identity preamble that proves who is on the other end of a link, the frame boundary, one link per address, and every message after that.

This crate is experimental. A node's identity and groups are durable: each storage category is a sealed append log over origin-private (OPFS) files, so an identity outlives the worker. Most consumers want the JavaScript package under [`bindings/web`](https://github.com/Offline-Protocol/offline-protocol-sdk/tree/main/bindings/web) rather than this crate directly; see the [web bridge document](https://github.com/Offline-Protocol/offline-protocol-sdk/blob/main/docs/bridges/web.md).

## License

Copyright © 2025-2026 Offline Protocol, Inc.

Dual-licensed: [AGPL-3.0-only](https://github.com/Offline-Protocol/offline-protocol-sdk/blob/main/LICENSE) for open-source use, or a [commercial license](https://github.com/Offline-Protocol/offline-protocol-sdk/blob/main/LICENSE-COMMERCIAL.md) for proprietary use.
