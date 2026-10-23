# The browser binding on two devices

CI runs every test page in Chromium, Firefox and WebKit, but always on one
machine and over loopback. What that cannot show is what a person meets on two
real devices: a phone's browser, a local network between them, a secure
context that is not `localhost`, and a mobile browser that freezes a tab the
moment it leaves the screen. This checklist covers that. Run it before a
release that changes `bindings/web` or `crates/offline-protocol-web`, and
record the result at the bottom of this file.

It uses [`examples/web-lobby`](../../examples/web-lobby), because it is the
page a person would actually try: durable nodes, a WebRTC link made from two
codes, an MLS group, and game state ten times a second.

## The pairs

Run each pair in both directions (each device hosts once).

| Pair | Why |
|------|-----|
| Laptop Chrome and Android Chrome | The most common pair, and the engine CI runs most |
| Laptop Chrome and iPhone Safari | Safari, not Playwright's WebKit build: real OPFS quotas, real mDNS candidates, and iOS's tab suspension |

## Serving the page

A browser gives OPFS, Web Locks and WebRTC only to a secure context: HTTPS, or
`localhost`. A LAN address over plain HTTP is neither, and the node fails
to start there (the page stays at "starting…"). Build the module once, then pick one of these.

```bash
cargo install wasm-bindgen-cli --version 0.2.105 --locked   # once
bash bindings/web/scripts/build.sh
```

**Android, no certificate.** Serve on the laptop and forward the port over
USB, so the phone opens `localhost`, which is a secure context:

```bash
python3 -m http.server 8000 --bind 127.0.0.1    # from the repository root
adb reverse tcp:8000 tcp:8000
# on the phone: http://localhost:8000/examples/web-lobby/
```

The forwarded port carries only the page. The WebRTC link between the two
devices still crosses the Wi-Fi network, which is what is being tested.

**Any phone, a local certificate.** [mkcert](https://github.com/FiloSottile/mkcert)
makes a certificate for the laptop's LAN address, and a few lines of Python
serve over TLS:

```bash
mkcert -install
mkcert -cert-file lan.pem -key-file lan-key.pem "$(ipconfig getifaddr en0)"   # Linux: hostname -I
python3 - <<'EOF'                                   # from the repository root
import http.server, ssl
server = http.server.ThreadingHTTPServer(("0.0.0.0", 8443), http.server.SimpleHTTPRequestHandler)
context = ssl.SSLContext(ssl.PROTOCOL_TLS_SERVER)
context.load_cert_chain("lan.pem", "lan-key.pem")
server.socket = context.wrap_socket(server.socket, server_side=True)
server.serve_forever()
EOF
# on the phone: https://<laptop-address>:8443/examples/web-lobby/
```

The phone must trust mkcert's root. On iOS, send `rootCA.pem` from
`mkcert -CAROOT` to the phone, install the profile, then turn on full trust
for it in Settings > General > About > Certificate Trust Settings. On Android,
install it under Settings > Security > Encryption & credentials. Remove the
root from the phone when the run is done: it can vouch for any site.

A tunnel (an HTTPS URL that forwards to the laptop) also gives a secure
context with no certificate on the phone. It puts the page on the internet
for as long as it runs; the WebRTC link still stays on the local network,
because the lobby configures no ICE server.

## The checks

Open the page on both devices. Each step names what to record.

1. **Start.** Both pages show an `off1…` address under "This device" within a
   few seconds, and no "the node ended" status. Record the time to an
   address on the phone.
2. **Link.** On device A press **Host a game**, then **Connect a device:
   make a code**. Get the code to device B (paste it through a shared note,
   or turn it into a QR code), press **I have a code**, paste it, press
   **Make my answer**, and get that answer back to A, then **Connect**.
   Record whether the link formed, and how long it took after **Connect**.
   If it does not form, record whether the network isolates clients from
   each other (guest Wi-Fi often does), since a link with no ICE server
   needs the two devices to reach each other directly.
3. **Check the address.** The players list on A names B's address. It
   matches the address B shows. This is the check a person is asked to make
   before letting a device in (R27, R28), so confirm it is readable on the
   phone's screen.
4. **Play.** The ball moves on both screens, and B's paddle follows B's
   finger on A's screen. Record the stats line on both devices after ten
   seconds of play: states sent and received per second, store flushes per
   second, and log bytes. The host should send about ten states a second and
   the player receive about as many.
5. **Reload keeps the identity.** Reload B. It shows the same `off1…`
   address as before. The link is gone (a WebRTC connection does not survive
   a reload), so exchange two new codes. Record whether B receives game state
   again without being invited a second time: it is still a member of the
   group, so it should.
6. **Background and return.** On the phone, switch to another app for
   thirty seconds, then come back. Record what happened: the game resumed on
   the same link; A showed "the other player disconnected" and two new codes
   restored play; or the phone's page showed "the node ended" and needed a
   reload. Repeat with the phone's screen locked for thirty seconds. None of
   these is a failure on its own; what must not happen is a page that looks
   connected and never moves again, or a reload that comes back as a new
   address.
7. **Two tabs, one profile.** On the laptop, open the lobby in a second tab
   with the same URL. The second tab must not start a second engine over the
   same account (W8): it stays at "starting…", and its console shows
   "another node already runs this account in this browser".

## What this does not show

- **Safari's storage eviction.** Safari may delete a site's script-writable
  storage, OPFS included, after a period with no visit. The identity goes
  with it, and no session-length run can see that.
- **Links across networks.** The lobby configures no ICE server, so both
  devices must be on one network. Crossing networks needs the application's
  own STUN or TURN servers (W9), and no page tests one yet.
- **Firefox for Android, Samsung Internet, and the other engines on iOS.** On
  iOS every browser is WebKit, so Chrome on an iPhone is Safari's engine.

## Results

Add a row per run: the date, the commit, both devices with their OS and
browser versions, and one line per check that did not go as written above.

| Date | Commit | Devices | Notes |
|------|--------|---------|-------|
