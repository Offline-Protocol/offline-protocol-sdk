# Web binding contract

Covers the browser binding: the engine compiled for `wasm32-unknown-unknown`
in `crates/offline-protocol-web`, and the JavaScript in `bindings/web` that
runs it. Experimental. A node's identity and groups are durable, in sealed
origin-private files; tabs of one origin link over a `BroadcastChannel`, and
devices on one network over WebRTC, set up with two codes and no server. A
node the application gives relays also carries the Nostr transport, which
reaches any device by its address and resolves usernames, mobile ones
included (W12), and browsers behind one network address find each other in
a nearby room (W13). In Chromium a page can also link to a phone or a board
over Bluetooth LE, as a central the person points at a device (W14).

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

`getrandom` 0.2, 0.3 and 0.4, `uuid` and OpenMLS each select a JavaScript
backend only when told to. The crate names each one for this target, and
`.cargo/config.toml` sets the `getrandom_backend="wasm_js"` cfg that
getrandom 0.3 also requires (only the final build can set it); 0.4 needs only
its feature. Without them the build fails, which is the safe direction, so a
dependency that brings in a new getrandom major shows up here first.

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

A connection closes on `failed` or `closed`, or once it has stayed
`disconnected` for 15 seconds (`disconnectedMs`). `disconnected` may recover
on its own, which the WebRTC specification allows, and closing on it at once
would turn a Wi-Fi blip into two new codes typed by hand. But nothing obliges
a browser to move on to `failed`, and Android's WebView was seen to sit in
`disconnected` for good after the other device restarted. The engine takes
one link per address (W3), so that dead link refused every new link from the
same device, and the two never met again until a page reloaded. The deadline
is longer than Chrome's own, so in Chrome it changes nothing: Chrome reaches
`disconnected` 5 seconds after the last answered connectivity check and
`failed` 15 seconds after it. Firefox and WebKit get there later, so in them
a blip that lasts longer than 15 seconds now closes where it might have
recovered: that is the price of a link a WebView would otherwise never
release. The deadline reads the state again when it fires, so a recovery
whose change event has not run yet is not closed. Either way, a close is what
reaches the engine as a lost peer. So the carrier dispatches a `connection`
event for every change, `connected`, `disconnected` and `closed`, naming the
object `offer()` or `answer()` resolved to. A page that waits for
`neighbor_lost` alone looks connected for those seconds, and a page that
watches for it on one side only looks connected for ever: the lobby's player
did, over a frozen court, until the [two-device run](web-devices.md#results)
found it. Events start once `offer()` or `answer()` has resolved: a call that
rejects, including one whose connection closed while it gathered, reports
nothing, as an event naming an object the page never received matches nothing
it holds.

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
most sixteen sessions, and the engine's link ceiling (also sixteen) stands
behind that. It remembers a session until the newest offer it has seen for
that session stops being fresh, which the worker reports with each signal
(the offer's `created_at` plus two minutes), so an offer a relay re-sends
late reaches the session it belongs to instead of opening a second
connection. A fixed hold counted from the first offer forgets too early: the
joiner republishes for up to thirty seconds, and a joiner whose clock runs
ahead sends offers that stay fresh for up to four minutes after the host
first sees them. A relay that refuses the connection counts once, so one dead
relay in the list does not stop the room opening on the others, and a room
that fails to open, on either side, closes every socket it opened. A joiner republishes its offer as a
new event every two seconds until an answer for its session arrives or its
deadline passes, because a host that subscribed after an ephemeral event was
sent never sees it.

**A hosted room stays reachable until it is closed.** `host()` resolves once
a relay confirms the subscription, and a room is meant to be held for as long
as the device wants to be found, which on a phone spans sleeps and network
changes. A relay socket that closes is dialled again, after one second,
doubling to thirty. A live one is asked to confirm its subscription every
`keepAliveMs` (default sixty seconds): NIP-01 lets a REQ under an open
subscription's id replace it, and the relay answers with EOSE again. A socket
that has not answered within ten seconds (or `keepAliveMs`, if shorter) of
being dialled or asked is dropped and dialled again, because a socket the
network lost without a word still reads as open until TCP gives up, minutes
later, and a relay that accepts a redial and says nothing would otherwise
hold its place for good. A relay waits on one confirmation at a time: a probe
is never sent while another is unanswered. Browsers run an interval's next
tick ahead of a deadline of the same length set inside it, so a probe that
reset the last one's state made an answer read as silence and dropped a
relay that had answered. The page coming back online or into view probes at
once and cuts every backoff short, and a room whose node stops closes
itself. Before this, a room whose sockets had
dropped answered nobody, while `host()` had long since resolved and the page
had no way to tell: a phone that had slept was unreachable until the app
restarted. A join keeps none of this; it lasts seconds, and fails on its
deadline.

### W12. The Nostr transport is driven from the worker, over the application's relays

A node started with `nostr: { relays }` registers the same `NostrTransport`
every binding uses, once its address exists, and the worker holds the relay
sockets beside the engine (`src/nostr-relays.js`). No binding opens a relay
socket in Rust: the Swift, Kotlin and Python drivers each hold their own and
move text, and this is the browser's copy of their host half
(`crates/offline-protocol-web/src/nostr_host.rs`). The sockets sit in the
worker rather than on the page, as the signalling ones do (W11), because
nothing here hands bytes to a page-only object: a relay message goes to the
engine and nowhere else.

**The relays are the application's.** `start` refuses `nostr` without a
non-empty list of `ws://` or `wss://` URLs, and no file the project ships
names one (`no_shipped_file_names_a_relay`). A relay the SDK named would
become the relay every application uses: a dependency nobody chose, and one
place that sees every application's traffic timing.

**The worker reads what every driver reads, and no more.** A relay
message's type and subscription id, and an `OK`'s event id and verdict. An
`EVENT` crosses to the engine whole, which decodes, unseals and attributes it
along the FFI's own path: unseal once, deliver with no transport peer id (a
Nostr key is not a protocol identity), drain, advance the receive watermark
only for bytes that decoded into a message, then announce the message's own
sender. In another order, junk addressed to this device's public routing tag
would move the watermark past history a relay still owes it.

**A relay is live while it keeps answering.** It is live from the first
`EOSE` of this device's subscription, and every 60 s it is sent a probe: a
`REQ` for an event id nobody can have, closed again on its `EOSE`. A relay
that has not answered within 10 s is dropped and redialled, from 1 s
doubling to 30 s, and coming back `online` redials at once. The probe is
never the subscription sent again under its own id: a relay answers that
replace with every stored event since the receive watermark, which sits more
than an hour back because gift wraps backdate `created_at`, so it would
replay that history from every relay every minute. The engine hears "live"
when the first relay is, which flushes the outbox, and "not live" when the
last one stops, before anything that relay held is failed or completed: a
flush in between would hand the next query to no relay and spend that peer's
five-minute resolution limit on an empty answer. Nothing is published to a
relay that is not live, and a relay the browser will not dial (a `ws://` URL
from an `https:` page) or that closes the subscription reaches the page as
an `error` event, once until that relay confirms a subscription again: a
relay that wants authentication refuses every redial, and a page told on
each one would hear it for ever. Stopping the node closes every socket and
says "not live" before it ends the queries still open, for the same reason
a lost relay does.

**An event is confirmed by the first relay that accepts it.** It goes to
every live relay, and fails only when each of them refused it or dropped.
The transport expires a confirmation nobody reported, so a silent relay costs
a retry rather than a message.

**A resolution query ends when every relay it went to has answered.** Each
relay's `EOSE` closes the query there, and the query completes when the last
one has, or after 10 s, below the engine's 30 s sweep, so the driver that
knows which relays answered is the one that ends it. Completing on the first
answer would hand the result to whichever relay is fastest, and an empty or
hostile one wins that race by having nothing to send
([the username chapter](../spec/username-discovery.md#a-resolution-ends-when-every-relay-has-answered)).
The 10 s is the sixth hand-mirrored constant
([C5](README.md#c5-hand-mirrored-constants-must-be-pinned-in-every-language)),
pinned here as in the two mobile trackers.

**Reaching a device by its address alone asks the relays for its key
packages.** The engine asks, as it does on every binding: `sendMessage` and
`establishSecureSession` to a device with no session and no live link queue
a lookup of the packages it published (cold contact, on by default), and the
tick asks again while a message waits. A linked device forms its session
over the link, and asking the relays anyway would only tell them which
address this node wants. The first package that opens forms the session,
and a message sent meanwhile waits for it. The worker only moves the query
and its answers; it decides nothing
([#633](https://github.com/Offline-Protocol/offline-protocol-sdk/issues/633)
is where the engine stopped leaving this to the binding).
`establishSecureSession` answers `"created"`, `"existing"` or
`"pending"`; for a device met only by its address, pending is the ordinary
first answer.

**The directory is one switch.** `usernameDiscovery` publishes `profile` as
a username and lets this node resolve others', both or neither, as the
chapter's gating rule requires, and it needs cold contact: a name that
resolves to an address with no published key package dead-ends one hop
later. `resolveUsername` resolves once the lookup is under way, and the
answer is one `username_resolved` event carrying every claim. A name is a
claim, not an identity: show the whole set and keep the address the person
chooses.

### W13. A nearby room is derived from the network's address

A page cannot scan for Bluetooth, browse DNS-SD or join Wi-Fi Direct, so a
browser has no local discovery of its own
([ADR 0034](../adr/0034-browsers-find-each-other-through-their-network-address.md)).
`NearbyRoom` gives the browsers behind one network address a place to meet:
a Nostr topic and key derived from the application id and the address a
STUN server reports (the IPv4 address, or the IPv6 /64, since each IPv6
device has its own address). Each device publishes a presence every 5 s,
lists the others it has heard within 15 s, and links over WebRTC to one its
person picks. The relays carry only the setup; the link runs on the local
network.

**The room key is public.** Anyone can derive it from an address, so it
seals only presences, and a presence holds a key used for this room and the
application's label. Because the room key authenticates nothing, a presence
is also signed by the key it announces: one sealed under the room key alone
let anyone list a key they do not hold, relabel a device that is there, or
keep one listed after it left. A connection code and the joiner's label are
sealed with NIP-44 from the joiner's room key to the presence key it picked,
and the answer back, under the room key as a second layer, with the role
bound into both. A holder of the room key sees who is present and can offer,
and reads no code
([R30](../security/threat-model.md#r30-a-nearby-room-is-readable-by-anyone-who-can-guess-the-networks-address)).
Sealing a code under the room key alone would hand every guesser the local
addresses the code names (R27), and a label under it alone would let anyone
re-wrap a real joiner's offer under a name of their choosing.

**The application admits.** Anyone who can guess the address can offer, so
`admit({ peer, label, signal })` is required and every offer goes to it;
only `true` answers. Each session is asked about once: a refused session is
ignored for as long as its offer stays fresh, so a joiner's republished
offers do not ask again, and `signal` aborts when the offer stops being
fresh or the room closes, so a question nobody answered comes down. The
label is a claim: who is on a link that forms is what the preamble and MLS
session prove (W3).

**A cap never becomes a lock.** The limits a signalling room keeps (W11)
assume only a holder of an eighty-bit code can offer; here anyone can. So a
refused session is kept apart from the sixteen the host is deciding on or
has admitted, and only so its repeats are not asked about again (at most
256, oldest forgotten first). Kept among the sixteen, as it first was,
sixteen refused sessions that a stranger kept fresh from anywhere on the
internet stopped the host admitting anyone, with nothing to say so. An offer
that arrives while sixteen are still waiting on the application is dropped
and reported as a `full` event. A list of devices that reaches sixty-four
forgets the one heard from longest ago, rather than refusing newcomers to
whoever filled it first.

**The relays and the STUN servers are the application's.** A room without
relays, or without a STUN server and without a `network` the application
already knows, refuses to open. `publicAddress` takes a server-reflexive
candidate a throwaway connection gathers and gives up after 5 s. It prefers
IPv4, and waits half a second for an IPv4 candidate after an IPv6 one: on a
dual-stack network the family that answers first varies, and two devices
that took different families would derive different rooms.

**A room opens once.** A room that fails to open, or is closed (during
`open()` included), holds no relay socket and no room in the worker, and
refuses to open or connect again.

**The room's events are signalling events.** Kind 25912, refused more than
two minutes from this clock, on a topic derived under a different salt from
a signalling room's, so the two never read each other. Offers and answers
are signed by a key used once, a presence by its own key. The signature is
checked on every event, after the room key, because it binds `created_at`:
without it a relay could re-stamp an old offer and replay it fresh.

### W14. Bluetooth is a central the person points at a device

Web Bluetooth lets a Chromium page be a GATT central and nothing else. It
cannot advertise, so nothing ever discovers a browser, and it serves no
GATT, so a peripheral can never dial it back. `BluetoothCarrier` links the
page's node to a phone or a board running the SDK as a peripheral, once its
person picks the device in the browser's chooser (inside a click, as the
browser requires) or from a device this origin was granted before. Firefox
and Safari have no Web Bluetooth, and neither does any browser on iOS.

The engine carries the same `BleTransport` every binding does, so the
fragment codec is the one the conformance vectors check
(`crates/offline-protocol-web/src/ble_host.rs`). The page holds the GATT
connection and moves bytes.

**A peer is announced only under the address its identity proves.** The
page reads the Device id and Identity values and hands both over;
`bleVerify` runs the one verifier, compares the derived address with the
Device id exactly, and remembers it. `bleLinkUp` announces only an address
verification produced, so a page cannot name a peer the engine did not
check.

**The contract's order, and the hello before anything else.** Choose the
instance, read and verify, subscribe to Message (notify or indicate,
whichever the peripheral offers), write Hello with response, then tell the
engine the link is up. Nothing is written to Message before the hello: a
peripheral cannot dial a browser back, so the hello is the only way it can
attribute a write, and one it cannot attribute is dropped. The iOS, Android
and Python peripherals serve Hello. With several instances of the service
behind one device, the carrier binds the one carrying this application's
App tag, else the only untagged one, else the first, as the chapter says.

**The hello is required, where the chapter makes it optional.** The
chapter lets a central skip Hello, and keep a link whose hello was
refused, because the peripheral can still dial that central back and read
who it is. Nothing can dial a browser. A device that serves no Hello, or
refuses the write, would be announced as a neighbour that neither receives
nor sends, so the carrier refuses it and disconnects. A refused write is
not rare: at an ATT MTU below 99 the browser sends the 96-byte hello as a
prepared write, which a peripheral must refuse.

**A link is announced only once the whole handshake held.** The carrier
watches for a disconnect from the moment the GATT connects. A device that
drops during the handshake is refused and never announced. One that drops
later is reported lost once, and so is a link the page closes after its
server already went away, when no disconnect event follows. A link that
replaces an earlier one to the same address keeps the peer announced: the
engine neither loses nor rediscovers it. Linking a device that is already
linked on a live connection hands back its peer and does nothing else: the
browser returns the same device and the same characteristics for one
physical device, so a second handshake would subscribe twice and deliver
every fragment to the engine once per handshake. Every path that ends a
link removes its notification listener for the same reason. Two connects
to one device at once share one handshake: the engine holds one
verification per address, so a second handshake would fail its link-up
and take the first one's link down with it.

**The write payload is the browser's if it says, and the floor if not.**
Chromium does not expose the negotiated ATT MTU to a page. The carrier
reads `maxWriteWithoutResponseSize` on the Message characteristic, where
the Web Bluetooth proposal puts it, and otherwise uses the 185-byte floor
every binding falls back to. No browser reports it yet, so the floor is
what runs today. A link whose ATT MTU is
below 188 cannot carry a 185-byte fragment, and no shipped device is known
to negotiate one, but it has not been measured from Chrome on Android
(see what is not built).

**Writes to a link go one at a time, in order, and a failed write closes
it.** A fragment lost mid-message leaves the peer reassembling something
that never completes. A peer linked over a peer stream as well as
Bluetooth is lost only when the last link goes, and stopping the node
reports every Bluetooth link lost and closes the carrier's connections.

**One carrier per node.** The node has one outbound Bluetooth sink. A
second `BluetoothCarrier` on the same node throws until the first is
released (`release()`); before, it took every outbound fragment and
dropped those for links it did not hold, and the first carrier's links
went silent with no error.

**A remembered device can keep `connect` waiting.** `known()` returns
devices the origin was granted, in range or not, and the browser's
connect to one out of range may wait on its own timeout. An application
that reconnects on load should bound the wait itself.

### W15. A page that loads offline caches one build whole

A node that links over a local network with no internet is no use if its
page cannot load without one. A service worker makes it load: the lobby
(`examples/web-lobby/sw.js`) is the worked example, and the SDK ships none,
because the scope, the files and the update policy are the application's.

**One build serves the page and the worker.** `index.js` on the page and
`worker.js` speak a message protocol of their own, and the wasm-bindgen glue
loads only the module it was built with, so a page and a worker from two
builds fail in ways no test covers. The cache is named for a build id that
`scripts/build-id.sh` derives from every file it holds (`build.sh` runs it;
a change to a served file that needs no Rust build must run it before it is
served, or the change reaches no device that has the page), and filled with
`addAll`, which keeps a cache complete or absent; it skips the HTTP cache,
which could hand back the last build's glue beside this build's module. Only
the current build's cache answers, and the update check fetches the build
id past the HTTP cache too (`updateViaCache: "none"`): the service worker's
own script is the same in every build, so a stale build id hides a new
build for as long as the HTTP cache calls it fresh, hours on a server that
sends only `Last-Modified`. A new build never runs under a page of the old
one: a worker the page starts again after a trap (W5) would load the new
build under the old page. It waits until every page of the scope has closed,
or until one page asks it to take over. Taking over moves every page of the
scope at once and deletes the old build's cache, so every page, not only
the one that asked, stops its node and reloads on `controllerchange`. A
page whose node was refused does not: it runs no engine, and reloading at
once it would take the account's lock (W8) from the tab still stopping the
node it plays on, which then comes back refused. It asks its person to
reload instead. A page whose node is still starting reloads, as its worker
may yet fetch the new build's files.

**Registering comes before the node and never waits.** A page that throws
before it registers, because its node could not start (a bad deploy, a
store that refuses to open), is served that build from the cache with no
way to be offered the next one, so the lobby registers before `start`.
Registration waits on the network, and on an update on the whole new
build, so it is never awaited: awaited between `start` and the listeners,
it let the engine's first events, a tab carrier's `neighbor_discovered`
among them, go to a target nobody listened on yet.

**The engine's worker must be inside the scope, under a page the scope
serves.** A dedicated worker is matched to a service worker by its own URL,
not by the page that started it. The lobby's scope is its own directory, so
it starts the engine through a one-line module there (`engine-worker.js`,
through `start({ workerUrl })`) that imports `worker.js`; the worker's
imports and its module fetch are then its own requests, which the scope
covers. Started from `bindings/web/src/worker.js` the worker loads online
and fails offline, which is what `test/offline.mjs` was checked against. It
does so only when the service worker controls the page: a page it does not
control (a hard reload, the first visit) came from the network, and an
in-scope worker under it would still come from the cache, the old build's
engine under the new build's page. That page starts the worker from its
default URL, out of the scope, so it comes from the network with the page.
An application that serves the SDK under its own scope needs no such module.

**The cache is not the node.** OPFS, IndexedDB and Web Locks are apart from
the cache, so an update, which replaces only the cache, leaves the identity
as it was. A browser that evicts storage takes the origin's whole, the
identity included unless `persist()` was granted, and clearing a site's data
clears both; only the cache comes back with the next visit online. The first visit must be online. On iOS a page added to
the home screen has storage apart from Safari's, so it is a second device
with its own identity.

`the_lobby_caches_every_sdk_file` holds the cached list to every file in
`bindings/web/src`: a file left out still loads online, where everyone
works.

## Which shared rules apply

| Rule | How it reaches the web binding |
|---|---|
| [C1](README.md#c1-regenerate-every-binding-together) | Does not apply: nothing is generated from the definition. The surface is `#[wasm_bindgen]` on `WebEngine` |
| [C2](README.md#c2-the-error-enum-is-append-only) | Does not apply yet: errors cross as a JavaScript `Error` carrying the engine's message, not a position |
| [C3](README.md#c3-events-cross-as-opaque-json) | Applies unchanged: `pollEvent` returns the engine's JSON, and the client dispatches it as a DOM event named by its `type` |
| [C5](README.md#c5-hand-mirrored-constants-must-be-pinned-in-every-language) | The resolution-query completion deadline is the one mirrored constant here, pinned in `nostr-relays.js` beside the two mobile trackers (W12) |
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
- Its own Nostr relays, if the node is to reach devices it has no link to
  (W12), and its own STUN server and an `admit` decision for a nearby room
  (W13). The SDK names no relay and no STUN server.
- A click that calls `BluetoothCarrier.request()`, for a Bluetooth link
  (W14): the browser opens its chooser only inside a user gesture.
- The engine module where the worker can fetch it: beside `worker.js`, or
  handed to `OfflineNode.start({ wasm })` by a host whose worker starts from
  a Blob URL (an editor plugin, an extension), where a relative fetch has
  nothing to resolve against.
- A service worker that caches the page, the SDK and the module as one
  build, with the engine's worker inside its scope, if the page is to load
  offline (W15).
- Nothing about framing, identity, encryption or storage: those are the
  engine's. The client posts `stop` to the worker on `pagehide` itself.

## Testing

```bash
cargo install wasm-bindgen-cli --version 0.2.105 --locked   # once
cargo test -p offline-protocol-web                          # two engines on the host
bash bindings/web/scripts/build.sh                          # module + glue in bindings/web/pkg
(cd bindings/web && npm ci && npx playwright install chromium firefox webkit)   # once
bash bindings/web/scripts/test-browser.sh                   # every page, three engines
CHROMEDRIVER=... GECKODRIVER=... \
  bash bindings/web/scripts/test-conformance.sh             # storage suites on OPFS
```

`scripts/test-browser.sh` runs every page in `bindings/web/test` in
Chromium, Firefox and WebKit, headless, through Playwright (`BROWSERS`
narrows the list). The engines are Playwright's builds, pinned by its version
in `package-lock.json`; WebKit is Safari's engine, not Safari. A page that
passes in one engine and not another is a real difference: the first run in
Firefox found that awaiting `navigator.storage.persist()` hangs every durable
start there, because Firefox answers it by asking the user.

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
  connection closes (W9, W10). Then a connection the page makes read
  `disconnected`: a blip shorter than `disconnectedMs` leaves it up, one that
  stays is closed and reported lost at both ends, and the same two devices
  link again. Chromium runs with `WebRtcHideLocalIpsWithMdns`
  off and Firefox with `media.peerconnection.ice.obfuscate_host_addresses`
  off, so the candidates are addresses a runner can reach. WebKit has no such
  switch and gathers `.local` names.
- `nostr-signal.html`: two nodes that share only a room code meet through
  a Nostr relay, link over WebRTC and send an ephemeral direct message. A
  join to a room nobody hosts gives up at its deadline, and a call to a
  stopped node rejects instead of hanging (W10, W11). A hosted room whose
  relay sockets the page closes must still answer a later join, and one on a
  relay that confirms its first subscription and then goes quiet (the relay's
  `/once` path) must drop that socket at its next probe and dial another.
  A relay that answers every probe of a half-second keep-alive keeps its one
  socket, coming back `online` redials ahead of the backoff, and a redial
  the relay accepts and never confirms (its `/first/<token>` path) is
  dropped and dialled again.
- `nostr-transport.html`: two nodes with no link and no code, only a relay
  (behind a dead one), on the Nostr transport. A message to a bare address
  forms a session and arrives end-to-end encrypted, a username the other
  published resolves to its address, an invite opens to its maker and a
  tampered one is refused, and a node offered no relays refuses to start
  (W12).
- `nostr-relays.html`: the relay driver alone, with a recording engine and
  its timers shortened. A keep-alive probe replays no stored event, an
  `OK false` fails the event with the relay's reason, a relay that goes
  quiet is reported down before its event fails and its query ends, a query
  one relay never answers ends at its deadline rather than on the first
  answer, a query two relays answer ends on the slower one's answer, a relay
  that closes the subscription is reported once however often it is
  redialled and never counts as live, and closing the driver reports it down
  before it ends a query (W12).

- `bluetooth.html`: a node links to SDK peripherals over its Bluetooth
  carrier, through a stand-in for `navigator.bluetooth`
  (`test/fake-bluetooth.js`) whose devices are other nodes playing the
  peripheral, so the engine runs on both sides. The chooser is asked for the
  SDK's service, the hello is the first write, a session forms and a
  message of several fragments crosses each way. A peripheral serving a
  Device id its identity does not prove, one serving no Hello, one refusing
  the hello and one dropping mid-handshake are each refused, disconnected
  and never announced. Linking a device already linked hands back its
  peer and delivers each fragment once, and two connects to one device at
  once share one handshake. A second carrier is refused until the first is
  released. Of two instances behind one device the one tagged with this
  application is bound; a disconnect reports the peer lost, as does closing
  a link whose server is already gone; stopping the node closes the
  carrier's links (W14). Moving the hello after the link-up makes the page
  fail, which is how the page was checked.
- `nearby.html`: four nodes in nearby rooms, three behind one address and
  one behind another. The address comes from the runner's STUN server, which
  reports a fixed one (`test/stun-server.mjs`); Firefox runs with
  `media.peerconnection.ice.loopback` on so it gathers toward a server on
  loopback. The three see each other's labels. One links to another that
  admits it and a message crosses, while a stranger keeps sixteen refused
  sessions fresh against the host. One the host does not admit gets no
  answer and is asked about once. A host whose person never answers is asked
  sixteen times, reports the seventeenth offer as `full`, and withdraws every
  question when closed. The fourth sees nobody. A room with no relays, no
  `admit` or no STUN server is refused, as is a closed one, and
  `publicAddress` over a stubbed connection takes IPv4 on a dual-stack
  network (W13).

- `offline.mjs`, after the pages: the lobby example opened once online,
  then reloaded with the browser offline. It must come back controlled by
  its service worker and as the same address, and a second profile's page
  (a URL the cache was not filled with) must start a node too. Then, online,
  a new build with two tabs open: Update in one must bring both onto the
  new build as the same addresses, with only the new build's cache left,
  and a third tab whose node was refused (its account open in the second,
  W8) must be offered the update too, and must not reload on it (W15).
  Reloading only the tab that pressed Update makes it fail, so does
  registering after `start`, and so does reloading the refused tab, whose
  reload takes the second tab's account.
  Starting the worker from `bindings/web/src/worker.js` instead makes it
  fail in all three engines, which is how it was checked. The server
  (`test/serve.py`) serves the repository root and marks every response
  `no-store`: with Python's plain server, Chromium and Firefox answered the
  out-of-scope worker from their HTTP cache and passed it.

  The relay is `test/nostr-relay.mjs`, a NIP-01 relay in Node's standard
  library. It keeps what is not ephemeral, replaces an addressable event by
  kind, author and `d` tag, and answers a subscription with what it holds,
  as the transport's key packages and directory records need; it verifies
  nothing, so the engine's own checks are what refuse a forged event.

`scripts/test-conformance.sh` runs
`crates/offline-protocol-web/tests/browser_conformance.rs` in a dedicated
worker of Chrome and Firefox, through wasm-bindgen's test runner and each
browser's WebDriver (`CHROMEDRIVER`, `GECKODRIVER`). It is the storage
adapter verification every other binding's store passes (C11), against the
log store over real OPFS files: both conformance suites, then a reopen after
enough writes to compact several times. The suites alone run against one open
store and cannot see a write that never reached the file; the reopen can,
and a store that wrote every record at offset zero fails it. A worker has
one thread, so the MLS suite's concurrent-writers check runs its writers in
turns there.

CI's `web-binding` job runs both. [The two-device checklist](web-devices.md)
covers what one machine cannot: a phone's browser, a local network, and a
tab the phone suspends.

[`examples/web-two-tabs`](../../examples/web-two-tabs) is the first page to
open in two tabs, and [`examples/web-lobby`](../../examples/web-lobby) a
two-player game between devices over WebRTC with durable nodes.

## What is not built yet

- **A TURN server.** The carrier takes the application's own (W9); the
  project runs none, and no page tests one.
- **Signalling through the Offline Protocol relay.** Nostr carries it (W11);
  the relay's protocol has no frame for an unauthenticated room.
- **The Offline Protocol relay as a transport.** Its handshake names the
  device in an `X-Device-ID` header, which a browser cannot set on a
  WebSocket, and it needs an application token. Nostr reaches the same
  devices without either (W12).
- **Ephemeral sends on the native bindings.** The engine has them and the
  browser exposes them; the UDL record does not yet carry the field
  ([ADR 0032](../adr/0032-ephemeral-sends-go-once-over-a-live-link.md)).
- **An end to a rung that can never be decrypted.** A group member's
  reliable rung that arrives behind more than 32 newer ephemeral frames is
  retried until it fails, and holds that member out of ephemeral frames
  meanwhile ([ADR 0032](../adr/0032-ephemeral-sends-go-once-over-a-live-link.md)).
- **Bluetooth from a real browser to a real device.** No engine runs Web
  Bluetooth without a person at the chooser, so CI drives the carrier
  through a stand-in. Chrome on Android, macOS and Linux against an
  Android phone and a board is a manual check, and so is the ATT MTU Chrome
  negotiates on each (W14). Reaching an iPhone needs its peripheral to serve
  Hello, which iOS releases before this one do not.
- **The lobby offline and installed on a phone.** `offline.mjs` reloads it
  offline in three engines on one machine, which installs nothing and is
  not Safari. Opening it offline on two phones and installing it is step 9
  of the [two-device checklist](web-devices.md), not run yet (W15).
- **A published package.** `bindings/web` is private on npm and the
  `offline-protocol-web` crate has not been released; build both from
  source.
- **Coverage beyond one machine.** CI runs every page in Chromium, Firefox
  and WebKit, but on one host over loopback, and WebKit there is
  Playwright's build rather than Safari. Two physical devices are a manual
  checklist ([web-devices.md](web-devices.md)), not a gate. The storage
  conformance suites run in Chrome and Firefox, not in WebKit.
