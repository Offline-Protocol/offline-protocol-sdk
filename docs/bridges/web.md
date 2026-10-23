# Web binding contract

Covers the browser binding: the engine compiled for `wasm32-unknown-unknown`
in `crates/offline-protocol-web`, and the JavaScript in `bindings/web` that
runs it. Experimental. A node's identity and groups are durable, in sealed
origin-private files; tabs of one origin link over a `BroadcastChannel`, and
devices on one network over WebRTC, set up with two codes and no server.

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
page (main thread)                   dedicated worker
 OfflineNode    ── postMessage ──►   worker.js ── WebEngine (wasm)
 WebRtcCarrier  ◄── frames ──────     tab carrier (BroadcastChannel)
 (RTCPeerConnection)                  OPFS log store (sync handles)
                                      tick every 100 ms: process,
                                      then frames out, then events out
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
second identity minted. The store that honours it is W8.

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
side. The tab carrier sends `close` to that instance; a WebRTC carrier closes
its connection, which the other end sees as its channel closing. A carrier
that only
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
the worker fails to load, and when the page calls `terminate()`. In each case
the page says `bye` on the tab carrier for the worker, which can no longer
say it, so the other tabs drop the link at once instead of after seven
seconds of silence; the page chooses the worker's carrier address for this
reason. The page's recovery is to start the node again: a durable node (W8)
comes back as the same device, with every write the engine had
acknowledged, and a node in memory comes back as a new one.

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

### W8. Storage is a sealed log, one node per account

**`Ok` is durable.** Each category (MLS material, protocol state) is an append
log in two origin-private files, and every write appends one record and
flushes the file before it returns. A file cannot be created synchronously,
so a store cannot make a file per record as the POSIX file stores do: the
worker opens all four files before the engine starts, and the engine never
asks for another. The current value of every key is held in memory; a log
past 1 MiB and over twice its live size is compacted into the spare file,
which commits by writing its header last. An I/O error once that header
write has begun empties the spare again, because a header that did reach
the disk would make the next open take the spare and drop every later
append; if the spare cannot be emptied, the log refuses every later write
rather than acknowledge one a restart would lose. A failed compaction is
retried only once the log has grown by another mebibyte, because compacting
seals every live record again and retrying it on each write would multiply
the cost of every write for as long as the cause lasts.

**Every record is sealed and bound to its place.** ChaCha20-Poly1305 under a
key derived per category and per generation from a 32-byte store key, with
the category, the file's generation and the record's sequence number as
associated data. A record moved to the other category, another generation
or another position does not authenticate, and each compaction starts a
fresh key, which bounds the random nonces any one key seals under.

**Only a torn tail is dropped; a wrong key is refused.** The header carries a
key check, and a log whose header does not authenticate is refused at open
with nothing truncated, because the common cause is the wrong key and
truncating on that would destroy the identity. A file shorter than a header
is a crash during the first header write, not a wrong key, and starts a
fresh log. Once the header authenticates, the first record that is short or
does not authenticate ends the log, which is what a crash during a write
leaves. A damaged record followed, at any offset, by an intact record sealed
under the next sequence number is not a tear, and the log is refused
untouched: cutting there would roll back acknowledged ratchet state, and a
reused ratchet generation is a reused AEAD nonce. The search tries every
offset rather than the one the damaged record's length prefix names, because
a corrupted prefix is the damage that otherwise reads exactly as a tear. Two
cases still read as a tear and are cut: damage to the last record, and two
damaged records in a row, since the search looks for the next sequence
number only.

**The store key is sealed too.** The worker seals it under a non-extractable
AES-GCM key kept in IndexedDB and stores the sealed copy beside the logs. A
node that finds logs without a sealed key, or a sealed key without its
wrapping key or that does not unseal (site data partly cleared), refuses to
start and changes nothing: a new key over old logs would strand the identity
they hold. While every log is still empty there is nothing to strand, and a
new key is minted, which is what a crash while the first key was written
needs. This
protects the files at rest; it is not a keystore, and a script on the origin
can drive the node or unseal the key
([R26](../security/threat-model.md#r26-a-browser-nodes-store-key-is-reachable-from-its-origin)).

**One node per account per origin.** The worker holds a Web Lock named for
the account (`account_storage_namespace` of the application id and profile,
the derivation every binding uses) for its whole life, and a second node for that
account is refused before it opens a file. The request waits up to two
seconds before refusing, because a worker the page terminated releases its
lock and its file handles shortly after, and a restart must not lose that
race. Two engines over one log would
each advance ratchet state the other never sees, and the groups would diverge
without an error. A page that wants several nodes in one browser gives each
its own profile, or asks for `storage: "memory"`, a new device every start.

The store is `LogStore` in `crates/offline-protocol-web/src/log_store.rs`, and
it passes both storage conformance suites on the host, over files in memory.
The OPFS adapter is a thin layer of five calls, and the browser's restart page
exercises it. The suites themselves do not run in the browser: the MLS
suite's concurrent-writes check spawns threads, which a worker does not have.

### W9. WebRTC lives on the page and is set up offline

`RTCPeerConnection` exists only on the main thread, so `WebRtcCarrier` lives
on the page and opens each link through `OfflineNode.openLink`. Its frames
cross to the worker and back by message under the link id the engine
allocated. The worker answers `openLink` before it sends that link's first
frame (our preamble), because the page drops a frame for a link it does not
know yet.

One reliable, ordered data channel per peer, negotiated on both sides, so the
link is a byte stream: the preamble arrives first and a frame's bytes in
order. Outbound bytes are cut into chunks of at most 16 KiB, a size every
browser accepts, and the engine's stream reader joins them; nothing in the
carrier reads a byte. Sends wait while more than 1 MiB is buffered on the
channel, and the frames behind that wait are held on the page. A peer that
lets more than 8 MiB pile up there has its connection closed, so a slow or
stalled peer holds the page's memory to about 9 MiB rather than without
bound. Closing loses nothing the engine promised: an acknowledged message
waits in its outbox for the next link, and an unacknowledged one (W10) was
never promised.

Signalling needs no network. Each side's description waits for ICE gathering
to finish, so one code carries every candidate, and the two codes cross by
whatever the people have: a QR code, a paste, or a Nostr room (W11). By
default no ICE server is configured, so a connection is made on the local
network or not at all. `new WebRtcCarrier(node, { iceServers })` takes the
application's own STUN and TURN servers for networks where devices cannot
reach each other directly; the project runs none, and every server named
learns which addresses are connecting. The codes are unauthenticated and name
addresses
([R27](../security/threat-model.md#r27-a-webrtc-connection-code-is-unauthenticated-and-names-local-addresses));
who is on the other end is still what the preamble proves (W3).

### W10. An ephemeral send goes once, over a live link

`sendMessage(to, content, { ephemeral: true })` and the same option on
`sendGroupMessage` are for state the next send replaces, such as a game's
positions ([ADR 0032](../adr/0032-ephemeral-sends-go-once-over-a-live-link.md)).
The frame asks for no acknowledgement and is handed once to the link
straight to each recipient; nothing is kept to retry it, park it or give it
to the relay, so it never arrives late. With no link, or for a direct message
no confirmed session, the call rejects at once and nothing was encrypted.
It carries plain content only.

A group send that misses a member still moves this device's ratchet for
them, so once 256 such sends have missed someone, one goes out reliable and
waits in their outbox: a rung they can decrypt across the gap when they come
back. Until that rung is acknowledged the member is handed no newer
ephemeral frame, because a receiver keeps only 32 generations behind the
newest it has seen, and a rung that arrived after more than that could never
be read. The first ephemeral send to a group after the node starts is one of
those. On the browser's sealed log an ephemeral group message costs the
sender two flushes, against four for a reliable one.

### W11. Signalling can ride a Nostr room

`NostrSignaller` moves the two connection codes through Nostr relays the
application names, so a player types a short room code instead of carrying
two long ones. The host listens in the room and answers every joiner; each
join becomes one `WebRtcCarrier` link.

**The room code is a shared secret, not an identity.** Both sides derive the
room's public topic and its sealing key from it, in Rust (`signal.rs`). A
relay sees the topic and ciphertext: the connection code is sealed under the
room key, with the topic and the role as associated data so an answer cannot
pass as an offer, and each event is signed by a key used once. Generated
codes carry eighty bits, and a code under ten characters is refused, because
the topic is public and a short code could be found from it. Whoever holds
the code can open a carrier toward the host and learn the addresses in the
codes; who is on that carrier is still what the preamble and the MLS session
prove ([R28](../security/threat-model.md#r28-a-signalling-room-code-admits-whoever-holds-it)).

**The page moves text.** The relay sockets live on the page beside the
WebRTC carrier, and every REQ, EVENT and relay message crosses to the worker
to be built or opened; the page reads only a relay's EOSE, to know its
subscription is live before it publishes. Signalling events use an ephemeral
kind (25912), which a relay forwards to whoever is subscribed and never
stores, and an event more than two minutes from this clock is refused, so a
relay cannot hold one and replay it later.

**The host is bounded.** It answers each joiner's session once (again only
when that session asks again, as its answer may have been lost), holds at
most sixteen answered sessions for two minutes each, and the engine's link
ceiling (also sixteen) stands behind that. The hold is at least the
two-minute freshness window, so an offer a relay re-sends late reaches the
session it belongs to instead of opening a second connection. A relay that
refuses the connection counts once, so one dead relay in the list does not
stop the room opening on the others. A joiner republishes its offer as a
new event every two seconds until an answer for its session arrives or its
deadline passes, because a host that subscribed after an ephemeral event was
sent never sees it.

## Which shared rules apply

| Rule | How it reaches the web binding |
|---|---|
| [C1](README.md#c1-regenerate-every-binding-together) | Does not apply: nothing is generated from the definition. The surface is `#[wasm_bindgen]` on `WebEngine` |
| [C2](README.md#c2-the-error-enum-is-append-only) | Does not apply yet: errors cross as a JavaScript `Error` carrying the engine's message, not a position |
| [C3](README.md#c3-events-cross-as-opaque-json) | Applies unchanged: `pollEvent` returns the engine's JSON, and the client dispatches it as a DOM event named by its `type` |
| [C7](README.md#c7-ordering-constraints) | `initialize` before `start`: the address replaces the placeholder the transport was built on |
| [C9](README.md#c9-bridge-behaviour-is-not-covered-by-the-rust-test-suite) | The browser test page is the binding's own test; the Rust tests in the crate run two engines over links on the host |
| [C11](README.md#c11-a-storage-adapter-is-a-supported-extension-point-and-is-verified) | The durable log store passes both conformance suites on the host, and so does the in-memory protocol-state store (W8) |
| [C12](README.md#c12-telemetry-is-callback-free-and-the-binding-fills-the-platform) | Not built: the pipe's uploader is a thread and a blocking client. A browser build has no telemetry until it has a `fetch` seam |

## What the host owes

- One worker per node, started through `OfflineNode.start({ appId })`, and
  one node per account per origin: a second tab gets its own `profile`, or
  the page uses `storage: "memory"` (W8).
- A secure context. OPFS, Web Locks and WebRTC exist only there: HTTPS, or
  `localhost`.
- A `fatal` listener that starts the node again, or tells the user the node
  ended (W5).
- A way to move two WebRTC codes between devices (by hand, or a
  `NostrSignaller` room and the relays it names), and the joining address
  shown to the people involved before anything is shared with it (R27, R28).
- Its own STUN or TURN servers, if links must cross networks (W9).
- Nothing about framing, identity, encryption or storage: those are the
  engine's. The client posts `stop` to the worker on `pagehide` itself.

## Testing

```bash
cargo install wasm-bindgen-cli --version 0.2.105 --locked   # once
cargo test -p offline-protocol-web                          # two engines on the host
bash bindings/web/scripts/build.sh                          # module + glue in bindings/web/pkg
bash bindings/web/scripts/test-browser.sh                   # headless Chrome
```

`scripts/test-browser.sh` runs every page in `bindings/web/test`:

- `two-nodes.html`: two nodes in two workers of one page, which reach each
  other over the origin's `BroadcastChannel` exactly as two tabs do. An MLS
  session both ways, an encrypted direct message, a group message, and a
  loss on stop.
- `refusal.html`: a carrier instance that sends no preamble is told its link
  closed (W3).
- `restart.html`: a durable node in a group is terminated mid-session and
  started again. It must come back with the same address, read the group's
  next message with no decryption failure, and a second node for the running
  account must be refused (W5, W8). With its wrapping key taken out of
  IndexedDB the node must refuse to start and leave its files as they were,
  and with the key put back it must start as the same device. Restarting it in memory instead makes
  the page fail, which is how the page was checked.
- `webrtc.html`: two nodes with no tab carrier, linked only by a data channel
  set up from two codes. A session, an encrypted direct message, a 100 KiB
  message that crosses in chunks, a group at 10 Hz sent acknowledged and
  then ephemeral, each with its delivery latency and the sender's store
  flushes (the ephemeral phase must cost fewer), and a loss when the
  connection closes (W9, W10). Chrome runs with `WebRtcHideLocalIpsWithMdns`
  off, so the candidates are addresses a runner can reach.
- `nostr-signal.html`: two nodes that share only a room code meet through
  a Nostr relay, link over WebRTC and send an ephemeral direct message. A
  join to a room nobody hosts gives up at its deadline, and a call to a
  stopped node rejects instead of hanging (W10, W11). The relay is
  `test/nostr-relay.mjs`, a NIP-01 relay in Node's standard library that
  stores and verifies nothing, so the page's own checks are what refuse a
  forged event.

CI's `web-binding` job runs them all.

[`examples/web-two-tabs`](../../examples/web-two-tabs) is the first page to
open in two tabs, and [`examples/web-lobby`](../../examples/web-lobby) a
two-player game between devices over WebRTC with durable nodes.

## What is not built yet

- **A TURN server.** The carrier takes the application's own (W9); the
  project runs none, and no page tests one.
- **Signalling through the Offline Protocol relay.** Nostr carries it (W11);
  the relay's protocol has no frame for an unauthenticated room.
- **Ephemeral sends on the native bindings.** The engine has them and the
  browser exposes them; the UDL record does not yet carry the field
  ([ADR 0032](../adr/0032-ephemeral-sends-go-once-over-a-live-link.md)).
- **Coverage beyond Chrome on one machine.** No page runs in Safari or
  Firefox, and no run has crossed two physical devices. The storage
  conformance suites run on the host only (W8).
