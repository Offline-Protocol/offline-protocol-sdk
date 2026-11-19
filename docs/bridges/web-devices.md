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
codes, an MLS group, and game state ten times a second. A page dialling a
native app from one code (W16) has its own checks,
[below](#a-page-dials-a-native-app).

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

## Bluetooth through a phone

Two browsers on separate devices, each linked over Web Bluetooth only to
one phone running the SDK as the peripheral, must reach each other through
it ([W14](web.md#w14-bluetooth-is-a-central-the-person-points-at-a-device)).
The lobby has no Bluetooth carrier, so this runs on a page that starts a
node with `ble: true`, links through `BluetoothCarrier.request()`, and
sends by address. It needs two Chromium browsers on separate devices: two
pages on one machine reach the phone through one radio, and the peripheral
attributes writes per central, by the hello it wrote.

1. Both pages link to the phone. Fails as: an empty chooser, or a link
   that rejects (a phone whose peripheral serves no Hello).
2. Each page sends a direct message to the other's address. Pass: it
   arrives `encrypted`, with the sender's address, and neither page lists
   the other as a neighbour.
3. One page creates a group and invites the other, which sends to it.
   Pass: the invite finds the member's key package and the message
   arrives.
4. The phone's app shows none of the plaintext.

| Date | Commit | Devices | Notes |
|------|--------|---------|-------|
| 2026-10-28 | e07b88be | iPhone 16, iOS 26.6.2, the SDK example app in the foreground (the peripheral); Samsung Galaxy M17 5G (SM-M176B), Android 16, Chrome 156.0.8078.47; MacBook, macOS 26.6.2, Chrome for Testing from Playwright's `chromium-1248`. Pages over `adb reverse` and `localhost`, driven over the DevTools protocol, the chooser answered with its `DeviceAccess` domain | 1: both linked to the iPhone's verified address; Chrome on Android listed it as an unknown device, Chromium on macOS by its name. 2: Mac to Samsung under 5 s, Samsung to Mac under 3 s, both encrypted; each page's only neighbour was the iPhone. 3: the invite found the key package on its first try, the Samsung joined within 3 s and its message reached the Mac within 3 s. 4: the app read "2 nearby • 0 unread". Chrome for Testing ships no `NSBluetoothAlwaysUsageDescription`, so macOS ends it at its first Bluetooth call: the run used a copy with the key added, re-signed ad hoc and started with `open`. Not run: an Android phone or a board as the peripheral, the app in the background, a link that drops and comes back |

## A page dials a native app

A page must reach the native app on the same Wi-Fi network, or on the
phone's own hotspot with no internet, from one code the app shows and
nothing else: no server, no relay, no second code
([W16](web.md#w16-a-page-dials-a-native-device-from-the-code-it-shows),
[ADR 0039](../adr/0039-a-browser-dials-a-native-device-from-a-code.md)).
CI dials the example listener and the Python host on one machine
(`direct.html`); what it cannot show is a real phone's listener, bound to
its own Wi-Fi or hotspot interface, answering a real phone's browser. This
runs on [`examples/web-direct`](../../examples/web-direct), which reads the
code from its URL fragment and at once removes it from the address bar with
`history.replaceState`, or takes it pasted. The native app opens its
listener (`openWebrtcDirect()` in React Native, `open_webrtc_direct(host)`
in Python) and shows the code.

| Pair | Why |
|------|-----|
| iPhone Safari dials the Android app | Real Safari, with its mDNS candidates, against the Android listener bound to the Wi-Fi network |
| Laptop Chrome, Firefox and Safari dial the iPhone app | Every engine against the iOS listener |
| iPhone Safari dials the Android app on the Android phone's own hotspot | The case the feature is for: no router and no internet |

Before starting, know what goes wrong for someone repeating it:

- **The page comes from HTTPS or the cache, never from the phone.** The node
  needs a secure context, so serve the page through a tunnel (see
  [serving the page](#serving-the-page)). A tunnel puts what it serves on the
  internet, so serve a directory holding only the page, `bindings/web/src`
  and `bindings/web/pkg`, not the repository root. The demo has no service
  worker: for the hotspot, load it while online and do not reload it after
  (an application's own page would be cached whole, W15).
- **The code is a credential.** It carries the listener's ICE password and
  admits whoever holds it for as long as the listener is open
  ([R35](../security/threat-model.md#r35-a-dial-code-admits-whoever-holds-it-to-an-open-listener)).
  Never paste it into a log, an issue, a chat or a shared note, and record
  the listener's host and port only. Close the listener when the run is
  done.
- **On iOS the listener lives only while the app is in the foreground.**
  Switching away to read or copy the code closes it, and the code dies with
  it: bring the app back and open the listener again, which makes a new
  code. The first open asks for the local network permission, which the
  listener needs: allow it.

1. **Open.** The app opens its listener and shows a code. Record the host
   and port it is bound to: the phone's Wi-Fi address, or on the hotspot
   its hotspot interface's. Open the page with the code behind `#`, or
   paste it. The address bar must no longer show the code.
2. **Link.** The page reads `linked`. Record the dial time. Fails as:
   `failed: timeout`, which is a closed listener, a stale code, or a
   network that isolates its clients.
3. **Proved, and a session.** The page shows the app's address as proved
   and the session as ready, and the app's first message (its presence)
   arrives encrypted. Fails as: linked and proved, and a session that never
   forms.
4. **Page to phone.** Send a text, then **Send 256 KiB**. Each shows a
   delivery receipt; record both latencies. The app shows the text once.
5. **Phone to page.** The app sends a direct message to the page's address
   (shown under "This page"). It arrives on the page encrypted, and the app
   shows it delivered.
6. **Dial again.** Press **Dial again** a few times. Each links, proves and
   forms a session; record the dial times. A message sent after the last is
   delivered, and the app shows it once.
7. **The phone's hotspot.** The Android phone shares its hotspot with no
   SIM and cellular data off, and is on no Wi-Fi network; the other device
   joins it, with the page already loaded (above). Open the listener after
   the hotspot is up, get the code to the page by typing it into the paste
   field, and repeat steps 2 to 6. Fails as: the listener refuses to open
   ("not on a Wi-Fi network and sharing no hotspot"), or a link that proves
   the address and never forms a session.

## What this does not show

- **Safari's storage eviction.** Safari may delete a site's script-writable
  storage, OPFS included, after a period with no visit. The identity goes
  with it, and no session-length run can see that.
- **Links across networks.** The lobby configures no ICE server, so both
  devices must be on one network. Crossing networks needs the application's
  own STUN or TURN servers (W9), and no page tests one yet.
- **Firefox for Android, Samsung Internet, and the other engines on iOS.** On
  iOS every browser is WebKit, so Chrome on an iPhone is Safari's engine.
- **An iPhone listening on its own hotspot.** It is not built: the iOS
  listener binds the Wi-Fi interface only
  ([S10](swift.md#s10-a-stream-a-page-dialled-is-a-row-in-the-one-stream-table-in-the-foreground-only)),
  and the iPhone in these runs has no cellular service to share. Only the
  Android phone's hotspot has been dialled.
- **Other diallers.** Chrome and Firefox on an Android phone have not dialled
  a listener, and neither has any browser on Windows. The laptop runs used
  Playwright's Chromium, Firefox and WebKit builds on a Mac, not Google
  Chrome or Safari.

## Results

Add a row per run: the date, the commit, both devices with their OS and
browser versions, and one line per check that did not go as written above.

| Date | Commit | Devices | Notes |
|------|--------|---------|-------|
| 2026-10-23 | 6bca9e37 | MacBook, macOS 26.5.1, Chrome 155.0.8059.86 (host); Samsung Galaxy M36 (SM-M366B), Android 16, Chrome 152.0.7977.82 (player). Page over `adb reverse`, both on one 5 GHz Wi-Fi network. Laptop hosted only; the phone-hosts direction is not run yet. Both pages driven over the DevTools protocol, with real touch input on the phone | 1: address on the phone in about 0.45 s, fresh or durable. 2: link 0.24 s after Connect, a host-candidate pair to the phone's Wi-Fi address. 4: 10 states/s each way; host 40 store flushes/s, player about 20/s. 5: the backlog the host sent while the link was down arrived in the first second (30 states). 6: background and screen lock both drop the link within 3 s, and the host shows "the other player disconnected" after about 17 s. The phone's page shows nothing: it still reads "Joined the game hosted by…" over a frozen court, because the lobby reports a lost peer only when it hosts. Two new codes restored play after backgrounding. After the lock, recovery was not checked because the phone has a secure lock screen |
| 2026-10-23 | e9c9878e | Same two devices as the row above. Both directions: the laptop hosted, then the phone hosted. Driven the same way | 2: link 40 to 110 ms after Connect, state flowing within 0.8 s. 4: 10 states/s each way; host about 22 store flushes/s with ephemeral sends. 5: the player and then the host reloaded; each came back as the same address, the host still hosting the same game, and state flowed again after two new codes with no second invite. 6: in both directions and for both background and lock, both pages showed "connection unstable" 11 to 13 s after the phone left the screen and the disconnect, with what to press, 10 s after that; the host sent nothing while apart, and after two new codes play resumed at 10 states/s with no backlog. After the lock the phone was relinked behind its secure lock screen; unlocking it by hand was not run |
| 2026-10-24 | fa09b4a8 | Same two devices as the rows above, same browser versions. Both directions, a fresh profile per pair. Driven the same way, with each page's status, link and node events logged against its own clock | 2: link 30 to 153 ms after Connect, state flowing within 1 s. 4: 10 states/s each way; host 20 to 23 store flushes/s. 5: in both directions the player and then the host reloaded, each the same address; the host came back hosting the same game and the player was let back in on its link ("is back") with no second invite. With the phone hosting, a laptop already in session with it was invited 0.1 s after the link. 6: home screen and screen lock, in both directions: both pages showed "connection unstable" 11.6 to 13.3 s after the phone left the screen and the disconnect, with what to press, 10 s after that; the host sent nothing while apart; two new codes resumed play at 10 states/s with no backlog. After the first lock the phone was unlocked by hand: its page still read "the host disconnected", and two new codes resumed play. 7: the second tab stayed at "starting…" with "another node already runs this account in this browser". Paths new at this commit, with the phone hosting: 55 invites made to fail (patched from the page) showed "still trying to add" at 10.1 s and the player joined at 15.2 s, never saved as a member while its invites failed; a resumed game the node does not hold showed "Group not found" and End this game at 10.1 s for a new device and 3.2 s after the link for a saved member. One of 20 fresh pairs, the first, never joined: the host's invite succeeded and it sent state, and the phone never reported the member added and showed no game, with nothing on either screen. On a later relink the host delivered a held message that the phone decrypted, and the phone still did not join. That run had no event log; 19 logged fresh pairs, three of them reloaded before linking, all joined, so the cause is not known |
| 2026-10-27 | 3a6e27ce | Samsung Galaxy M17 5G (SM-M176B), Android 16, Chrome 154.0.8037.92 (host); iPhone 16, iOS 26.6.2, Safari (player). Phone to phone, no laptop browser. Page over an ngrok tunnel from `test/serve.py` (every response `no-store`), both phones on one 5 GHz Wi-Fi network. Chrome driven over the DevTools protocol, Safari through WebDriverAgent with real touches, codes typed in. Phone-hosts direction only | 1: an address on both. Chrome's cache held the 17 files under the build id, the page the lobby's own (ngrok's browser warning was clicked through once per phone and never cached). 2: link 46 to 173 ms after Connect, over seven links. 3: the host's players list names the iPhone's address. 4: host 10 states/s at 22 to 24 store flushes/s, player 10/s received at 22/s; the iPhone's finger moved its paddle on the host's canvas (8%, 93%, 51% down). 5: both reloaded as the same address; the host came back hosting the same game and let the player back in ("is back") with no second invite. 6: iPhone to the home screen: host "connection unstable" 9.3 s later and "the other player disconnected" 9.7 s after that. Samsung to the home screen, and locked: the iPhone said unstable after about 14 s and disconnected 8 to 9 s after; after the lock, Chrome had reloaded the tab (a `back_forward` navigation), back as the same address hosting the same game. Two new codes resumed play each time. Locking the iPhone not run: WebDriverAgent cannot lock iOS 26.6. 7: a second tab stayed at "starting…" with "another node already runs this account in this browser". 8: not run, no relays or STUN server chosen. 9: server stopped, both phones on the Samsung's hotspot with no internet: each reloaded from its service worker as the same address. They did not link: Chrome on the phone sharing the hotspot gathered no ICE candidate (Safari's answer had its mDNS one), so the offline link is not run yet and step 9 now asks for a router. Installed: Chrome minted a WebAPK with the Pong icon, which opened standalone with no network as the tab's address, still hosting. Safari added the Pong icon and name, but the installed page's first offline launch showed "Safari can't open the page because your iPhone is not connected to the internet": it has its own service worker and cache. Opened once online it was a new address, and offline after that it opened as that address |

### WebRTC Direct

One row per run of [a page dials a native app](#a-page-dials-a-native-app):
the date, the commit, the dialler and the listener with their OS and
browser versions, and the numbers each check asks for. Never the code.

| Date | Commit | Devices | Notes |
|------|--------|---------|-------|
| 2026-11-19 | d04ed2dd | iPhone 16, iOS 27.2, Safari (dialler); Samsung Galaxy M17 5G (SM-M176B), Android 16, the React Native example app with a local panel that calls `openWebrtcDirect()` (listener). One home Wi-Fi network. Page over an HTTPS tunnel serving only the page, `bindings/web/src` and `pkg`; the code read off the app's screen and opened in Safari as the fragment | 1: listener on the phone's Wi-Fi address, bound to the Wi-Fi network. 2: linked in 209 ms. 3: proved, session ready, the app's presence arrived encrypted. 4: text delivered in 604 ms, 256 KiB in 1663 ms. 5: arrived encrypted, the app showed it delivered. 6: three dials again linked in 104, 56 and 125 ms; a message after them was delivered in 704 ms and shown once. Earlier the same day Chromium 156 on a Mac dialled the same app: linked in 473 ms, a session, a text in 368 ms, 256 KiB in 1918 ms, a dial again in 179 ms, and the app's message arrived. 7: run in the rows below |
| 2026-11-19 | 9ee92f85 | MacBook, Playwright's Chromium 156, Firefox 157 and WebKit 27.2 (diallers; no Google Chrome was installed, so Chromium stood in for it); iPhone 16, iOS 27.2, the example app built Debug with its JavaScript bundled (listener). One home Wi-Fi network | 1: listener on en0's address. 2 to 6, as Chromium, Firefox, WebKit: linked in 221, 167, 326 ms; a session in all three; text delivered in 557, 510, 300 ms; 256 KiB in 849, 744, 1115 ms; dial again linked in 210, 135, 329 ms and a message after it delivered in 553, 652, 725 ms. 5: in Chromium only, linked in 169 ms: the app's message arrived encrypted, from the app's address. 7: not built for an iPhone |
| 2026-11-19 | dfba700f | iPhone 16, iOS 27.2, Safari (dialler); the Samsung above, sharing its hotspot with no SIM and no internet, on no Wi-Fi network (listener). The page loaded online first, then the tunnel closed and the iPhone joined the hotspot; the code typed into the paste field | 7: the listener opened on the hotspot interface (`swlan0`), which before this commit it refused ("not on a Wi-Fi network"). Linked in 180 ms and proved, but no session formed: the Android manager reported the peer-stream layer up only for Wi-Fi P2P or the LAN, neither is up on a phone sharing its hotspot, so the engine held the link unusable and no key package went out. Fixed in 095008ab: an open listener holds the layer up |
| 2026-11-19 | 095008ab | Same devices, hotspot and page as the row above | 7: listener on `swlan0`. 2: linked in 68 ms. 3: proved, session ready. 4: text delivered in 665 ms, 256 KiB in 1821 ms. 5: arrived encrypted, and so did the app's presence. 6: dial again linked in 110 ms, proved, session ready |
