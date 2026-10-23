# Pong lobby

A two-player Pong game between browsers, with no server. Each browser runs a full Offline Protocol node in a Web Worker with durable storage. The host makes an MLS group and invites every device it forms a session with; the host's game state goes to the group ten times a second, end-to-end encrypted, and the other player sends its paddle back as a direct message.

```bash
cargo install wasm-bindgen-cli --version 0.2.105 --locked   # once; must match Cargo.lock
bash bindings/web/scripts/build.sh
python3 -m http.server 8000 --bind 0.0.0.0                   # from the repository root
```

**Two devices on one network.** Open `http://<this-machine>:8000/examples/web-lobby/` on both. A browser gives WebRTC and OPFS only to a secure context, and a LAN address over plain HTTP is not one, so serve the page over HTTPS, or on a phone use a tunnel to `localhost`. On one device press **Host a game**, then **Connect a device: make a code**, and get the code to the other device (paste it, or show it as a QR code). The other device presses **I have a code**, pastes it, and sends its answer back the same way. The devices connect on the local network: no ICE server is configured, so nothing leaves it.

**Two tabs of one browser.** Open <http://127.0.0.1:8000/examples/web-lobby/> and <http://127.0.0.1:8000/examples/web-lobby/?profile=b>. A durable node is one per account per origin, so the second tab needs its own profile. The tabs find each other over a `BroadcastChannel` with no codes.

The identity survives a reload: the same profile is the same `off1…` address, and still a member of the group.

The stats line shows what the game costs, including the durable store's flushes per second. Every state update is an ordinary acknowledged, retried MLS send, and each one writes the sender's ratchet state and the delivery state before it returns. That is the cost an unacknowledged send for game state would remove. See [docs/bridges/web.md](../../docs/bridges/web.md).
