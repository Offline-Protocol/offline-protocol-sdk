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

The lobby is cached under a build id (W15). After pulling a change that
needs no Rust build, run `bash bindings/web/scripts/build-id.sh` before
serving it, or every device that opened the page keeps the old one.

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
   the player receive about as many. Both are ephemeral sends (W10), so the
   host's flushes should be about two per state.
5. **Reload keeps the identity.** Reload B. It shows the same `off1…`
   address as before. The link is gone (a WebRTC connection does not survive
   a reload), so exchange two new codes. Record whether B receives game state
   again without being invited a second time: it is still a member of the
   group, so it should. Then reload A: it comes back hosting the same game,
   and after two new codes B receives state again and A's players list names
   B, still with no second invite.
6. **Background and return.** On the phone, switch to another app for
   thirty seconds, then come back. Record what happened: the game resumed on
   the same link; both pages said the other device disconnected and two new
   codes restored play; or the phone's page showed "the node ended" and
   needed a reload. Repeat with the phone's screen locked for thirty seconds.
   On Chrome, A should show "connection unstable" within about 15 seconds
   of the phone leaving the screen (Android keeps the page's network for a
   few seconds first) and "the other player disconnected" 10 seconds after
   that, and the phone the same about the other device once it is back. None
   of these is a failure on its own; what must not happen is a page that
   looks connected and never moves again, or a reload that comes back as a
   new address.
7. **Two tabs, one profile.** On the laptop, open the lobby in a second tab
   with the same URL. The second tab must not start a second engine over the
   same account (W8): it stays at "starting…", and its console shows
   "another node already runs this account in this browser".
8. **Nearby, with no codes.** Reload both pages. On each, fill in the same
   Nostr relays and a STUN server you choose, a name, and press **Look for
   devices nearby** (W13). Each lists the other by its name within a few
   seconds; press **Connect** beside A on B, and A asks whether to let B in.
   **Allow** links them ("1 device connected" on both), and **Deny** leaves B
   waiting until its request times out. Then put the phone on cellular data:
   it is behind another address now, and the two must no longer list each
   other. Fails as: an empty list on one network (the STUN server unreachable,
   or the two leaving by different routes, which the page cannot tell), or a
   link that forms without A being asked.
9. **Offline, and installed** (W15). With both pages loaded once online,
   stop the laptop's server and put both devices on a router with no
   internet (one unplugged from the wall). A phone's own hotspot does not
   do: Chrome on the phone sharing it gathers no ICE candidate, as the
   hotspot is no network Android reports, so that phone cannot link. Reload
   each: it shows the same `off1…` address it had. Link them with two codes
   and play. Then install the page (Chrome: **Install**; Safari: **Add to
   Home Screen**). On Android, close the lobby's tab first (the installed
   app is the same account, W8) and open the app from the app drawer, still
   offline: it opens, with the Pong icon, as the same address as the tab.
   On iOS the installed page has storage apart from Safari's, its service
   worker and cache included, so open it once online, where it is a new
   address, then offline, where it must open as that address. Fails as: the
   browser's offline page, "starting…" that never ends, a different address
   after an offline reload, or a generic icon.

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
| 2026-10-23 | 6bca9e37 | MacBook, macOS 26.5.1, Chrome 155.0.8059.86 (host); Samsung Galaxy M36 (SM-M366B), Android 16, Chrome 152.0.7977.82 (player). Page over `adb reverse`, both on one 5 GHz Wi-Fi network. Laptop hosted only; the phone-hosts direction is not run yet. Both pages driven over the DevTools protocol, with real touch input on the phone | 1: address on the phone in about 0.45 s, fresh or durable. 2: link 0.24 s after Connect, a host-candidate pair to the phone's Wi-Fi address. 4: 10 states/s each way; host 40 store flushes/s, player about 20/s. 5: the backlog the host sent while the link was down arrived in the first second (30 states). 6: background and screen lock both drop the link within 3 s, and the host shows "the other player disconnected" after about 17 s. The phone's page shows nothing: it still reads "Joined the game hosted by…" over a frozen court, because the lobby reports a lost peer only when it hosts. Two new codes restored play after backgrounding. After the lock, recovery was not checked because the phone has a secure lock screen |
| 2026-10-23 | e9c9878e | Same two devices as the row above. Both directions: the laptop hosted, then the phone hosted. Driven the same way | 2: link 40 to 110 ms after Connect, state flowing within 0.8 s. 4: 10 states/s each way; host about 22 store flushes/s with ephemeral sends. 5: the player and then the host reloaded; each came back as the same address, the host still hosting the same game, and state flowed again after two new codes with no second invite. 6: in both directions and for both background and lock, both pages showed "connection unstable" 11 to 13 s after the phone left the screen and the disconnect, with what to press, 10 s after that; the host sent nothing while apart, and after two new codes play resumed at 10 states/s with no backlog. After the lock the phone was relinked behind its secure lock screen; unlocking it by hand was not run |
| 2026-10-24 | fa09b4a8 | Same two devices as the rows above, same browser versions. Both directions, a fresh profile per pair. Driven the same way, with each page's status, link and node events logged against its own clock | 2: link 30 to 153 ms after Connect, state flowing within 1 s. 4: 10 states/s each way; host 20 to 23 store flushes/s. 5: in both directions the player and then the host reloaded, each the same address; the host came back hosting the same game and the player was let back in on its link ("is back") with no second invite. With the phone hosting, a laptop already in session with it was invited 0.1 s after the link. 6: home screen and screen lock, in both directions: both pages showed "connection unstable" 11.6 to 13.3 s after the phone left the screen and the disconnect, with what to press, 10 s after that; the host sent nothing while apart; two new codes resumed play at 10 states/s with no backlog. After the first lock the phone was unlocked by hand: its page still read "the host disconnected", and two new codes resumed play. 7: the second tab stayed at "starting…" with "another node already runs this account in this browser". Paths new at this commit, with the phone hosting: 55 invites made to fail (patched from the page) showed "still trying to add" at 10.1 s and the player joined at 15.2 s, never saved as a member while its invites failed; a resumed game the node does not hold showed "Group not found" and End this game at 10.1 s for a new device and 3.2 s after the link for a saved member. One of 20 fresh pairs, the first, never joined: the host's invite succeeded and it sent state, and the phone never reported the member added and showed no game, with nothing on either screen. On a later relink the host delivered a held message that the phone decrypted, and the phone still did not join. That run had no event log; 19 logged fresh pairs, three of them reloaded before linking, all joined, so the cause is not known |
| 2026-10-27 | 3a6e27ce | Samsung Galaxy M17 5G (SM-M176B), Android 16, Chrome 154.0.8037.92 (host); iPhone 16, iOS 26.6.2, Safari (player). Phone to phone, no laptop browser. Page over an ngrok tunnel from `test/serve.py` (every response `no-store`), both phones on one 5 GHz Wi-Fi network. Chrome driven over the DevTools protocol, Safari through WebDriverAgent with real touches, codes typed in. Phone-hosts direction only | 1: an address on both. Chrome's cache held the 17 files under the build id, the page the lobby's own (ngrok's browser warning was clicked through once per phone and never cached). 2: link 46 to 173 ms after Connect, over seven links. 3: the host's players list names the iPhone's address. 4: host 10 states/s at 22 to 24 store flushes/s, player 10/s received at 22/s; the iPhone's finger moved its paddle on the host's canvas (8%, 93%, 51% down). 5: both reloaded as the same address; the host came back hosting the same game and let the player back in ("is back") with no second invite. 6: iPhone to the home screen: host "connection unstable" 9.3 s later and "the other player disconnected" 9.7 s after that. Samsung to the home screen, and locked: the iPhone said unstable after about 14 s and disconnected 8 to 9 s after; after the lock, Chrome had reloaded the tab (a `back_forward` navigation), back as the same address hosting the same game. Two new codes resumed play each time. Locking the iPhone not run: WebDriverAgent cannot lock iOS 26.6. 7: a second tab stayed at "starting…" with "another node already runs this account in this browser". 8: not run, no relays or STUN server chosen. 9: server stopped, both phones on the Samsung's hotspot with no internet: each reloaded from its service worker as the same address. They did not link: Chrome on the phone sharing the hotspot gathered no ICE candidate (Safari's answer had its mDNS one), so the offline link is not run yet and step 9 now asks for a router. Installed: Chrome minted a WebAPK with the Pong icon, which opened standalone with no network as the tab's address, still hosting. Safari added the Pong icon and name, but the installed page's first offline launch showed "Safari can't open the page because your iPhone is not connected to the internet": it has its own service worker and cache. Opened once online it was a new address, and offline after that it opened as that address |
