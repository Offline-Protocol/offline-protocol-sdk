# offline-protocol-webrtc-direct

A WebRTC Direct listener: a native device that a web page on the same network dials from a code the device shows, with no internet and no signalling server. The listener is an ICE-lite responder with a self-signed DTLS certificate on one UDP port. The page builds the listener's session description from the code and never edits its own, and the data channel it opens carries an ordinary peer stream, proved by the same identity preamble and framed the same way as a TCP one.

Provides:

- `DialCode`, the `#op-direct=1&...` fragment a listener writes and a dialler reads, with every refusal rule the specification states
- `Listener`, sans-I/O: the platform owns the UDP socket and the clock, hands in datagrams and timer ticks, and gets back datagrams to send and three events (a stream proved an address, a body arrived, a proved stream ended)

`examples/listener` runs a listener in front of a full engine: the reference host loop the native bridges follow (one stream table, the newer dialled stream wins, the slot's caps), printing one JSON line per event. `cargo run -p offline-protocol-webrtc-direct --example listener -- <private-ip>`; its documentation lists the lines and the commands it reads.

The format is specified in [stream framing](https://github.com/Offline-Protocol/offline-protocol-sdk/blob/main/docs/spec/stream-framing.md#dialling-a-listener-from-a-browser), and the design in [ADR 0039](https://github.com/Offline-Protocol/offline-protocol-sdk/blob/main/docs/adr/0039-a-browser-dials-a-native-device-from-a-code.md).

This crate is an internal layer of the SDK, used by the native bindings. Most Rust consumers want the main [`offline-protocol`](https://crates.io/crates/offline-protocol) crate instead, and app developers want the [React Native](https://www.npmjs.com/package/@offline-protocol/mesh-sdk) or [Python](https://github.com/Offline-Protocol/offline-protocol-sdk/blob/main/bindings/python/README.md) bindings.

## License

Copyright © 2025-2026 Offline Protocol, Inc.

Dual-licensed: [AGPL-3.0-only](https://github.com/Offline-Protocol/offline-protocol-sdk/blob/main/LICENSE) for open-source use, or a [commercial license](https://github.com/Offline-Protocol/offline-protocol-sdk/blob/main/LICENSE-COMMERCIAL.md) for proprietary use.
