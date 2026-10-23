# Web binding contract

Covers the browser binding: the engine compiled for `wasm32-unknown-unknown`
in `crates/offline-protocol-web`, and the JavaScript in `bindings/web` that
runs it. Experimental: identities are held in memory, and only the tab
carrier exists so far.

Read [the shared contract](README.md) first. This document covers what is
specific to a page that runs the engine.

## What it is

**A browser is a full node.** It mints its own identity, forms MLS sessions,
creates and administers groups, and relays like any other node. It is not a
[leaf](../spec/leaf-provisioning.md): a leaf never commits, so a browser
built on one could join a game's lobby but never host it.

The engine is the same Rust as every other binding's, compiled without the
two default features a browser cannot run (`telemetry-pipe`, a background
thread and a blocking HTTPS client; `file-store`, POSIX files). It needs no
async runtime: it is a synchronous state machine the host ticks, as every
binding already drives it.

```
page (main thread)                 dedicated worker
 OfflineNode  ── postMessage ──►   worker.js ── WebEngine (wasm)
 carriers     ◄── frames ──────      tick every 100 ms: process,
 (tabs, later WebRTC)                 then frames out, then events out
```

## Rules

### W1. The engine runs in a dedicated worker

Every storage trait the engine writes through is synchronous, and a returned
`Ok` means durable: the engine immediately writes state that depends on it,
and OpenMLS saves group state inside every encrypt. The only browser storage
that is synchronous and can flush is an OPFS sync access handle, and the
platform exposes those only to dedicated workers. A worker also keeps MLS work
off the thread a game renders on.

The failure a main-thread engine invites is the one the trait documents: an
in-memory mirror that acknowledges a write before IndexedDB commits it, a
crash, and on restart a ratchet generation reused (an AEAD nonce reused) or a
second identity minted.

### W2. Nothing calls back into the page

`WebEngine` never calls JavaScript. Events wait in a queue the worker drains
(`pollEvent`, capped at 1024, oldest first), and outbound frames in another
(`nextOutbound`). The engine fires its own callbacks while it holds its state
lock, and on the only thread a worker has, a callback that called back in
would re-enter that lock.

### W3. The page moves bytes; the engine decides what they mean

A carrier is an opaque link id. The identity preamble, the length prefix on
every frame (even on a carrier with its own message boundaries), the refusal
of a link whose first frame does not verify, and one announced link per
address are all done in Rust, by the one implementation of
[peer-stream framing](../spec/stream-framing.md) (`PeerStreamReader`,
`frame`). A JavaScript copy would be a second implementation the conformance
vectors never see.

A carrier address (the tab carrier's random instance id, later a WebRTC
connection) proves nothing. Who is on the other end is what the preamble
proves, and a link is announced under that address only.

A link that has proved nobody is temporary. The engine holds at most sixteen
links and closes one whose preamble has not verified within ten seconds, the
mobile managers' values for the chapter's local policy. Without them, a
carrier that opens links and never speaks holds memory and one of our
preambles each for as long as the worker lives.

**Every close reaches the other end.** When the engine refuses a link
(`linkReceived` returns a reason) or closes one itself (`nextClosed`), its
side is already gone, and the carrier tells the other end, which closes its
side. The tab carrier sends `close` to that instance. A carrier that only
forgot the link would leave the other end announcing a peer it will never
hear from again, and its own heartbeat would keep that link alive: the
chapter's rule is that the close is the refused side's signal. Both ends then
ignore each other for thirty seconds before trying again, so a heartbeat does
not reopen a link the engine just refused.

### W4. The clock is read through `offline_protocol_core::time`

On `wasm32-unknown-unknown`, `std::time::Instant::now()` and
`SystemTime::now()` compile and panic when called: the target has no host
clock. A green build therefore proves nothing. Every crate the browser build
compiles reads the clock through `offline_protocol_core::time`, which is
`std::time` on every other target and `web-time` (`performance.now()`,
`Date.now()`) on this one. `clocks_are_read_through_core_time_only` refuses a
direct import.

### W5. A panic ends the worker

The standard library for this target is built to abort, so no
`catch_unwind` seam the other bindings rely on catches anything here. A panic
traps the module, and JavaScript sees the trap as a `WebAssembly.RuntimeError`
it can catch. The worker must not: a module that trapped stopped in the
middle of whatever it was changing, and ticking it again drives state no
invariant covers. The worker reports the trap once, says `bye` on the
carrier, and closes itself. `OfflineNode` then rejects every call in flight
and every later one, and dispatches one `fatal` event; it does the same when
the worker fails to load. The page's recovery is a new node, which with
in-memory storage is a new device.

### W6. Entropy comes from the browser

`getrandom` 0.2 and 0.3, `uuid` and OpenMLS each select a JavaScript backend
only when told to. The crate names each one for this target, and
`.cargo/config.toml` sets the `getrandom_backend="wasm_js"` cfg that
getrandom 0.3 also requires (only the final build can set it). Without them
the build fails, which is the safe direction.

### W7. The wasm-bindgen versions agree

The crate pins `wasm-bindgen` with `=`, and the command-line tool must be the
same version: the two share an unversioned schema, and a mismatch fails only
when the module is post-processed. `the_wasm_bindgen_pins_agree` holds the
lockfile, the crate, `scripts/build.sh` and CI's install to one version.

## Which shared rules apply

| Rule | How it reaches the web binding |
|---|---|
| [C1](README.md#c1-regenerate-every-binding-together) | Does not apply: nothing is generated from the definition. The surface is `#[wasm_bindgen]` on `WebEngine` |
| [C2](README.md#c2-the-error-enum-is-append-only) | Does not apply yet: errors cross as a JavaScript `Error` carrying the engine's message, not a position |
| [C3](README.md#c3-events-cross-as-opaque-json) | Applies unchanged: `pollEvent` returns the engine's JSON, and the client dispatches it as a DOM event named by its `type` |
| [C7](README.md#c7-ordering-constraints) | `initialize` before `start`: the address replaces the placeholder the transport was built on |
| [C9](README.md#c9-bridge-behaviour-is-not-covered-by-the-rust-test-suite) | The browser test page is the binding's own test; the Rust tests in the crate run two engines over links on the host |
| [C11](README.md#c11-a-storage-adapter-is-a-supported-extension-point-and-is-verified) | The in-memory protocol-state store passes the conformance suite. A durable store owes the same |
| [C12](README.md#c12-telemetry-is-callback-free-and-the-binding-fills-the-platform) | Not built: the pipe's uploader is a thread and a blocking client. A browser build has no telemetry until it has a `fetch` seam |

## What the host owes

- One worker per node, started through `OfflineNode.start({ appId })`.
- A `fatal` listener that starts a new node, or tells the user the node
  ended (W5).
- `node.stop()` on `pagehide`, so the other nodes see the link close instead
  of waiting out the tab carrier's seven-second silence.
- Nothing about framing, identity or encryption: those are the engine's.

## Testing

```bash
cargo install wasm-bindgen-cli --version 0.2.105 --locked   # once
cargo test -p offline-protocol-web                          # two engines on the host
bash bindings/web/scripts/build.sh                          # module + glue in bindings/web/pkg
bash bindings/web/scripts/test-browser.sh                   # headless Chrome
```

The browser tests start two nodes in two workers of one page, which reach each
other over the origin's `BroadcastChannel` exactly as two tabs do, and check
an MLS session both ways, an encrypted direct message, a group message, and a
loss on stop. A second page opens a link to a node from a carrier instance that
sends no preamble and checks that the node's carrier tells it the link closed
(W3). CI's `web-binding` job runs both pages.

[`examples/web-two-tabs`](../../examples/web-two-tabs) is the same thing as a
page to open in two tabs.

## What is not built yet

Durable storage (an OPFS log under W1), WebRTC between devices, and an
unacknowledged send for game state. The plan and its open questions are
tracked with the experiment.
