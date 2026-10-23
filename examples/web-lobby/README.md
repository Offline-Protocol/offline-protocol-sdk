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

Before a release, [docs/bridges/web-devices.md](../../docs/bridges/web-devices.md) is the checklist for running this page on a laptop and a phone.

**When a device goes away.** A phone that leaves the page, locks its screen or loses the network drops its link. Both pages say so: "connection unstable" while the browser still waits for an answer, then that the other device disconnected, with what to press to carry on. Two new codes bring it back with no second invite, as both devices are still in the group. A host that reloads comes back hosting the same game; **End this game** forgets it.

The stats line shows what the game costs, including the durable store's flushes per second. The game state and the paddle are ephemeral sends: each goes once over the live link, is never acknowledged or retried, and is never held for a link that is down, so an update that missed its moment does not arrive late. On the browser's store that is two flushes per state for the host instead of four. See [docs/bridges/web.md](../../docs/bridges/web.md), W9 and W10.
