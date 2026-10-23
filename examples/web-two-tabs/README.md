# Mesh between tabs

Two or more browser tabs, each a full Offline Protocol node in a Web Worker, talking end-to-end encrypted with no server. The tabs find each other over a `BroadcastChannel`; the engine proves who is on the other end of each link, forms an MLS session, and encrypts every message.

```bash
cargo install wasm-bindgen-cli --version 0.2.105 --locked   # once; must match Cargo.lock
bash bindings/web/scripts/build.sh
python3 -m http.server 8000 --bind 127.0.0.1                 # from the repository root
```

Open <http://127.0.0.1:8000/examples/web-two-tabs/> in two tabs of the same browser. Each shows its own `off1…` address and, within a second or two, the other tab as a neighbour with an encrypted session. Pick it and send.

Identities are held in memory: reloading a tab makes it a new device with a new address. Durable storage and WebRTC between devices are the next steps in [docs/bridges/web.md](../../docs/bridges/web.md).
