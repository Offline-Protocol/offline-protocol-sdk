# Dial a native app from a page

A page that links to a native app on the same Wi-Fi network with no internet and no server. The app opens a WebRTC Direct listener and shows a dial code; the page reads the code, dials it, and the two talk end-to-end encrypted over the data channel ([W16](../../docs/bridges/web.md#w16-a-page-dials-a-native-device-from-the-code-it-shows), [ADR 0039](../../docs/adr/0039-a-browser-dials-a-native-device-from-a-code.md)).

The code arrives in the page's fragment, so the app's QR code is this page's URL with the code behind `#`:

```
https://<origin>/examples/web-direct/#op-direct=1&h=...&p=...&c=...&u=...&w=...&a=...
```

The page removes the code from the address bar as soon as it reads it (the code admits dials for as long as the listener is open) and keeps it in memory for **Dial again**. With no fragment, paste a code or a whole URL into the field.

It shows the state (`dialling`, `linked`, `failed: <reason>`, `disconnected`, `closed`), the dial time from the call to the open channel, the address it linked to, whether the listener's preamble proved that address, whether an MLS session is ready, and every message received. **Send** sends a direct message and shows its delivery latency; **Send 256 KiB** sends one message at the engine's content limit (262144 bytes). The key results are also in `document.title` (`linked 78ms | session | delivered 520ms | big 599ms | received 2`, or `failed: timeout`) and in `window.demo`, for a driver.

The node runs in memory, so every load is a new device. Its app id is `offline-messenger`, the one [the React Native example app](../react-native-app) uses; `?app=<id>` names another.

## Try it on one machine

```bash
cargo install wasm-bindgen-cli --version 0.2.105 --locked   # once; must match Cargo.lock
bash bindings/web/scripts/build.sh
cargo build -p offline-protocol-webrtc-direct --example listener
target/debug/examples/listener "$(ipconfig getifaddr en0)" --app-id offline-messenger
python3 bindings/web/test/serve.py 8000 .                    # in another shell, from the repository root
```

The listener's first line is JSON whose `code` is the fragment. Open `http://localhost:8000/examples/web-direct/#<code>`. The page links, a session forms, and a message you send comes back as `echo:<text>` (the example listener echoes every direct message; the 256 KiB one is delivered but not echoed, since the prefix would put it over the limit).

## From a phone

The page needs a secure context: `localhost`, or HTTPS. A phone's browser therefore loads it over HTTPS from the application's origin (or a tunnel to a machine serving the repository root), never from the listener. The dial itself stays on the Wi-Fi network: the listener's address is in the code and the page uses no ICE server. A dial with no answer (`failed: timeout`) means the listener did not admit it: a closed listener, a stale code, or two devices that cannot reach each other on that network.

See [docs/bridges/web.md](../../docs/bridges/web.md).

## Community

Built something with this example, or stuck running it? Share it or ask in the
[developer Discord](https://discord.gg/9TQGrxaPJk).
