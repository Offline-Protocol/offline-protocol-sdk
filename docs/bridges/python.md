# Python bridge contract

Covers the generated Python bindings and the desktop package.

Read [the shared contract](README.md) first. This document covers what is
specific to Python.

## The generated layer

UniFFI produces Python from the interface definition. It is one third of the
artifact set described in [C1](README.md#c1-regenerate-every-binding-together)
and is never regenerated alone.

The desktop build script delegates to the same generation script, and CI builds
the desktop package through it.

## P1. This is the thinnest binding, and that makes it the ABI canary

Python has no platform integration to speak of: no Keychain, no Keystore, no
Bluetooth stack, no React Native lifecycle. It is close to a direct view of the
interface definition.

That makes it the cheapest place to detect an ABI break. If a change makes the
Python package fail to import or a call fail on a checksum, the Swift and Kotlin
bindings have the same problem and will surface it later, on a device, in a
harder-to-diagnose form.

Run the Python tests before the mobile ones when changing the interface.

## P2. Nothing here is a reference implementation of platform concerns

The Python package does not implement secure storage against a platform keystore.
Do not copy its storage handling into a mobile binding, and do not treat its
behaviour as the contract for one.

It does carry one shared constant, which is easy to miss precisely because the
rest of the binding is platform-free: `state_storage.py` holds one of the four
copies of the protocol-state record ceiling, and it must spell
`8 * 1024 * 1024` exactly. A Rust guard reads this file, so editing the ceiling
in the mobile bindings and not here fails the **Rust** suite, not `pytest`. See
[C5](README.md#c5-hand-mirrored-constants-must-be-pinned-in-every-language).

## P3. Packaging

The package is versioned in lockstep with the workspace. A release cut touches
the Python project metadata along with the Cargo manifests, the lockfile, and the
third-party notices.

## P4. The error enum is positional here too

The same append-only rule applies. See
[C2](README.md#c2-the-error-enum-is-append-only).

## P5. Events

Events arrive as JSON strings, exactly as in the other bindings. Python's
dynamism makes it tempting to consume them ad hoc, and that is fine for tooling,
but it means the Python surface offers no drift protection at all. It will not
catch a renamed event field for you.

## P6. Python is where a storage adapter is cheapest to get right

Python is the only binding where an application can hand in its own
`ProtocolStateStorageProvider` today (`ProtocolManager(state_storage=...)`),
which makes it the best place to develop and debug an adapter before writing
the same thing in Swift or Kotlin.

The contract and the gate are the same in every language:

```python
import json
from offline_protocol_sdk.offline_protocol import run_storage_conformance

report = json.loads(run_storage_conformance(my_adapter))
assert report["failures"] == [], report["failures"]
```

Two Python-specific traps, both of which the suite catches:

- **Values are `bytes`, not `str`.** Sealed records are ciphertext, so a
  provider that decodes to text anywhere in its path corrupts them. `sqlite3`
  in particular returns `memoryview` for a BLOB in some configurations — wrap
  it in `bytes()` before returning.
- **An absent key returns `None`,** it does not raise. The SDK asks for
  records that legitimately do not exist yet on every launch, and raising
  turns a normal startup into an error path.

A worked reference lives in
[`examples/storage-adapters/python/sqlite_state_storage.py`](../../examples/storage-adapters/python/sqlite_state_storage.py).

Python currently ships **no** `wipePersistedState` equivalent (the mobile
bindings do). An application that needs logout has to clear its own storage
root, and if it pointed documents at a separate backend, call
`DataStore.wipe_all()` too. Stop the protocol first: a wipe records no
removals, so on a running engine with live sessions it is undone by the peer's
next version offer, which recreates and refills every document.
`DataStore.remove_space()` is what removes documents from the other replicas.

Interest (`DataStore.set_interest`) belongs after `initialize_mls` and before
`start()`, because the engine's start-up exchange offers each space with the
interest in force when it starts. `ProtocolManager.start()` does both, and the
local API server opens its store after it, so a store reached through either
runs its first exchange under the default interest and narrows from the next
one. An application that needs the first exchange narrowed drives
`OfflineProtocol` itself in the engine's order.

## P7. The internet send loop is adaptive here, and fixed elsewhere

The core's internet outbox is poll-only across the FFI in every binding. The
Swift and Kotlin managers drain it on a fixed 100 ms tick. The Python manager
(`internet_manager.py`) drains it adaptively: an inbound frame wakes the send
loop immediately, a drain that moved frames re-drains at event-loop speed, and
an empty drain backs off exponentially from 2 ms to the shared 100 ms idle
interval.

The invariant both shapes preserve: a locally queued message waits at most one
idle interval, and a quiet link costs at most one poll per interval. What the
adaptive shape adds is that a reply to an inbound frame does not pay the poll
interval at all, which at 100 ms was most of a warm round trip.

Two consequences worth knowing before touching it:

- The policy is pinned by `TestAdaptiveSendLoop` in
  `tests/test_internet_manager.py`, not by a latency measurement. Change the
  tests with the policy, or a revert ships silently.
- The in-flight tracker sweep inside `_poll_and_send_messages` is gated to the
  old 10 Hz cadence (`_PRUNE_MIN_INTERVAL_MS`). The gate exists because the
  adaptive loop can call the drain at event-loop speed during a burst, and the
  sweep walks every tracked recipient; without the gate a burst turns into a
  sweep storm.

Porting the adaptive shape to Swift and Kotlin is intended eventually. Until
then the latency profiles differ by design: a warm Python round trip is
milliseconds, while the mobile bridges pay up to one tick per direction.

## P8. The BLE roles serve and verify the identity the core proves

The Python peripheral serves two values a phone reads before it will talk to
it: the Device id characteristic carries this device's `off1…` address, and
the Identity characteristic carries the identity assertion the core builds
over that address's key (`identity_assertion`, the whole
`public_key(32) ‖ signature(64) ‖ signed_data` layout). The binding assembles
nothing. Before this the peripheral served the profile label as its id and a
JSON document as its identity, which every conforming central refuses, so no
phone could ever pair with a Python node. `start()` now refuses to advertise
until MLS has minted an address, because advertising an unverifiable peer only
costs the phones a connect-read-refuse cycle each.

The Python central holds the other half: it reads both characteristics, hands
the identity bytes to `verify_identity_assertion` (the one verifier of the
layout, instance-less like `derive_address`), compares the returned address to
the Device id string exactly, and only then announces the peer, under the
derived address. A link that fails any step is disconnected and never
announced, not even under its Bluetooth address, which it used to be. See
[the identity assertion](../spec/ble-framing.md#the-identity-assertion) for
the four steps and what a verified assertion does not prove.

`test_identity_assertion.py` pins both halves against the real library and
the RFC 8032 vectors; `test_ble_peripheral.py` and `test_ble_manager.py` pin
what each role does with the answer. The JSON form is gone, and a test asserts
the constructor no longer accepts it.

## P9. A socket client's verdict is the preamble, not the connect

`PeerStreamManager` is the host's implementation of the peer-stream slot the
FFI names `wifi_direct`: TCP streams to configured `host:port` peers or to
hosts found over DNS-SD, framed as [the chapter](../spec/stream-framing.md)
specifies. It announces a peer to the core only under the address
`verify_identity_assertion` derived from the stream's first frame, and only
then, never on connect, never from the `addr` in a discovery record, and never
under a socket address. A stream whose preamble fails, arrives late, or names
a different address than the peer list claimed is closed and was never
announced, so its close reports nothing. The same holds for the outbound
side: the core queues a body only toward an address a stream proved, so this
manager is woken only for a stream it holds.

Two rules here are local policy the chapter leaves to the implementation,
and they are chosen so two hosts agree without talking: when a second stream
proves an address already announced, the one opened by the lower address is
kept (two hosts that each list the other open toward each other at once, and
without a shared rule each would keep what the other discards, forever), and
between two of the winning kind the newer supersedes the older (the lower
address reconnecting must not be blocked by its own half-open stream). Either
way the core sees one announcement and one loss per address. The iOS and
Android managers keep the same stream by the same rule, because a phone and
a host on one LAN both dial; `every_peer_stream_manager_keeps_the_same_stream`
pins the three copies together
([ADR 0027](../adr/0027-ios-peer-streams-ride-network-framework.md),
[ADR 0028](../adr/0028-android-peer-streams-join-the-lan.md)).

The tie-break gives the higher address no such way past a stale stream: its
reconnect is the losing kind for as long as the lower side holds a stream it
believes live. That is why every stream carries TCP keepalive (fifteen idle
seconds, three probes five seconds apart), and why an attempt that ends
without an announced stream climbs the reconnect ladder instead of retrying at
a fixed pace. A port on the mobile managers owes both, not only the tie-break.

Keepalive ends an *idle* dead stream only. A stream with unacknowledged data
is not idle, and outside Linux nothing bounds its retransmissions, so the
write path carries its own bound. The core hands every outbound body through
one queue for all peers, and the manager moves each onto its stream's own
queue without awaiting anything; each stream's writer then waits on its own
peer and aborts the stream once its write buffer has made no progress for
`write_timeout` (thirty seconds). That deadline starts only when the
buffers between the two hosts are full: until then the kernel takes every
write, and on a default host the two sides absorb on the order of a megabyte,
so a peer that is alive but has stopped reading stays announced under light
traffic, and detection can take two windows because bytes moving into the
kernel count as progress in the first. Nothing at this layer can do better;
the core's acknowledgements are the signal, and a port must not promise
more. A single drain that awaited each write
would let one peer that stops reading hold every other peer's traffic, and
`writer.close()` alone would not free it, because asyncio keeps a socket
open until its buffer flushes; a discarded stream with bytes still buffered
is aborted. A port on the mobile managers owes this shape too.

Both bounds on that path key on the peer's behaviour, never on the sender's
scheduling. The per-stream queue (4 MiB) drops a body only while its writer
is blocked on the peer: the core hands a burst over in one tick, before the
writer has run at all, so a bound applied then drops frames for a peer that
is reading, and those bytes were already held by the core's queue. The
deadline is on progress rather than per frame: a per-frame deadline is a
throughput floor (1 MiB in thirty seconds), and a slow link below it is
aborted, reconnected, offered the same frame by the retry path and aborted
again, forever.

Three bounds keep a listener that binds every interface (the default) from
being held by the network: one deadline covers the whole preamble frame,
prefix and body, so four bytes and then silence cannot keep a slot; one remote
host holds at most `max_streams_per_host` inbound streams (eight); and the
total is `max_streams` (sixty-four). A DNS-SD record carries its own host name
and the listener's interface addresses, because a record without them is seen
by every browser and resolved by none.

`test_peer_stream_manager.py` pins all of it on real loopback sockets, replays
the chapter's vectors through the real verifier, and asserts the framing
bounds as literals (`4`, `1_048_576`, `96`) for the C5 reason. DNS-SD needs
the optional extra (`pip install 'offline-protocol-sdk[lan]'`); the base
install carries no LGPL dependency.

The same type carries service instances under the subtype `_svc._sub`
([DNS-SD mapping](../spec/dns-sd-mapping.md)), published and read by
`dnssd_bridge.py` over the same optional extra. Two rules of that chapter are
the binding's to hold, because nothing in the core can see a LAN record: an
import is delivered with `source: "lan"` and never reaches
`register_service` (a registration made from an unsigned LAN record would go
out in signed discovery responses under this node's identity), and a peer
browser ignores any record carrying `sid` (each published service would
otherwise be one more connector to the same host). `test_dnssd_bridge.py`
asserts the chapter's bounds (`200`, `255`, `1300`, `63`) and the subtype as
literals for the C5 reason, and `services.py` mirrors the engine's closed
status set (`ok`, `not_found`, `error`) as a literal pinned the same way.

## P10. Freeing a core object never blocks, wherever the interpreter frees it

The generated bindings keep every callback object in one handle map behind
one lock. Rust goes through that map on every callback it makes (a lookup)
and on every callback it drops (a removal). The collector runs between any
two bytecodes, those inside the map's critical section included, and a core
object it finalizes there drops the callbacks the core holds, each of which
asks for the map's lock on the thread that already has it. With the plain
lock the generator emits, that thread waits for itself and the process
hangs.

A core object reaches the collector whenever it sits in a reference cycle,
and the ordinary shapes are cycles: an event handler that is a method of the
object owning the manager, an exception whose traceback names the manager.
It could not happen while a stopped manager was never freed at all, which is
why it arrived with the fix for that leak.

The package installs a re-entrant lock on every handle map when it is
imported (`_callback_reentrancy.py`), before any callback exists. Re-entry
is removal only, since a finalizer drops callbacks and never registers one,
and each of the map's critical sections is one dictionary operation or a
read followed by an insert under a fresh handle. The generated file cannot
carry the change itself: it is regenerated from the UDL and the drift gate
compares it byte for byte.

`test_callback_reentrancy.py` frees a core while holding the map's lock, in
a process of its own so that a regression fails one test instead of hanging
the suite, and finds the maps by looking rather than by name, so a
regenerated binding that moves the map or adds a second one is caught.

Releasing the file stores does not depend on any of this. `close()` asks
the core to release them (`close_file_stores`), and the directories are free
when it returns, whatever still refers to the manager.

## P11. A gateway session is announced only once it is bound, and a frame is settled only on its verdict

`GatewayManager` is the host's client for the gateway-daemon contract
([the chapter](../spec/gateway-contract.md)) behind the slot the FFI names
`reticulum`: newline-delimited JSON over TCP to a daemon on local IP. Two
things it promises, and the shape that keeps each:

**The carrier is offered to the core only for a session the gateway bound
to this device's address.** `reticulum_status_changed(True)` is called from
one place, on `StatusUpdate(connected)`, and only after the gateway's
`AddressDeclared` echoed the address `local_address()` holds. The
`Capabilities` frame is handed to the core as it arrives, and the contract
puts it before the announcement, so on a conforming gateway the core knows
what the gateway can do before the flush the announcement triggers. A
session announced before it is bound is closed rather than kept: it is verdict-only on the gateway's side, so
nothing addressed to this device would ever arrive over it, and a transport
that can only refuse must not be offered to the selector. An echo of
someone else's address, a refused declaration, a challenge that is not 32
bytes, or a gateway that says nothing for ten seconds each cost the
connection, and the reconnect ladder decides when to try again. The ladder
resets only on a bound and announced session, never on the TCP open: reset
there, a refusing gateway was retried at the floor forever with a signature
spent per turn. The declaration itself comes from the core
(`gateway_address_declaration`); the module names no signing domain, and a
Rust guard reads it to keep that true.

**The socket write is not the outcome.** Every `SendMessage` carries the
core's message id and is held in a verdict tracker until the gateway's
`MessageSent` or `DeliveryError` names it; only then is
`reticulum_confirm_sent` or `reticulum_send_failed_with_reason` called.
Confirming on the write is what the first clients of this contract did,
and it is why `recipient_unreachable`, the one verdict that parks a message
and offers it to the mesh, never reached the core from them. Verdicts are
correlated by id, never by order, because a gateway answers submissions as
their routing resolves. At most eight frames are unanswered at once, an id
already in flight is not sent again when the core re-queues it, and every
outstanding id is settled exactly once: a duplicate verdict is ignored, a
connection that closes fails what it carried with `Connection lost`, and a
gateway silent for sixty seconds has the frame failed under
`gateway_silent`, which the core reads as a retry. Sixty is chosen to stay
under the core's own 120 s pending-confirmation expiry; the Rust guard
`gateway_manager_constants_match_across_both_bridges` reads the Python
policy beside the two mobile ones and holds that relationship.

This is the first client on this carrier to read the two optional verdict
flags. `stored` on a `DeliveryError` is reported as `relay_stored`,
`pushed` on a `MessageSent` as `relay_pushed`, and both together as
`relay_pushed_stored`: the relay client's own mapping, so a gateway with a
mailbox or a push parks a plain message the way the relay does and
fast-fails nothing the push may have delivered. Each flag is a statement
about the one frame the verdict names, never about the recipient.

The decisions and frame shapes are pure functions in
`gateway_attach_policy.py`, `gateway_verdict_tracker.py` and
`presence_watch_policy.py`, ports of the Swift files of the same names,
with no socket and no core; `test_gateway_attach_policy.py` and
`test_presence_watch_policy.py` pin their hand-mirrored constants as
literals (C5, the seventh and eighth sets). `test_gateway_manager.py`
drives the manager against a fake daemon on a real loopback socket and
pins the attach order, the correlation, the flags and what a dead
connection owes. No daemon has been run against it; a conforming daemon is
a deployment this repository does not ship.

## P12. A relay answer reaches the core unattributed, and the rule lives in one place

The relay's group answers (`__GROUP_CREATED__`, the membership pair,
`__GROUP_INFO__`, `__USER_GROUPS__`, `__GROUP_ERROR__`) are frames this
binding synthesizes, so nothing can sign them. The core exempts them from the
control-frame signature gate only when they arrive on the Internet transport
with no transport peer identity, so an answer injected with a `sender_id` is
dropped as unsigned, and the caller sees a successful inject.

`relay_answer_prefixes.py` holds the list, and `_inject_group_frame` passes
every actor through its `attributable_actor`, as the Swift and Kotlin bridges
do. A new arm cannot attribute an answer by mistake. The list is one of the
four hand-mirrored copies under [C5](README.md#c5-hand-mirrored-constants-must-be-pinned-in-every-language),
and `test_relay_answer_prefixes.py` pins it as literals. Before #368 there was
no Python copy, and the injection had drifted to `__GRP_*` names the registry
never held.

## P13. The HTTP front carries bodies only in sealed messages, and its literals are pinned

`offline_protocol_sdk.http_front` is a client of the local API like any
other, so every rule of [the local API bridge](local-api.md) holds for it
unchanged: it declares one application id, its sends are stamped with it,
and the server holds its inbound messages while it is away. It sends every
request and response as a direct message, never on the service request path,
whose bodies are plaintext, and it acts only on a `message_received` the
engine reports as `encrypted`. Discovery is the one thing it takes from the
service path. It runs one per service, since every client under one
application id receives each request. Everything a handler can wait on
(an answer, a browse stream, a callback) is ended before the HTTP server's
cleanup, which otherwise waits two minutes for a running handler and
outlives a supervisor's stop grace period. The envelope limits, the carried-header list, the application
id and the error tokens of [the chapter](../spec/http-front.md) are module
literals, and its tests pin them as literals (C5). `aiohttp` is imported only
when the front starts, behind the `http` extra, so the base install's
dependency set is unchanged.

## P14. On BlueZ the peripheral learns its centrals from the bus, and one drain feeds both roles

bless tracks the centrals of its server only in its CoreBluetooth backend. Its
BlueZ characteristic drops the options BlueZ passes to `ReadValue` and
`WriteValue`, which name the calling device, and `StartNotify` names none. So
on Linux the peripheral adds a dbus_next message handler to bless's own bus
that reads `options["device"]` from each read or write of its application's
characteristics before bless dispatches the call, and never answers one. A
device is a central once it has called into the application, and stops being
one when BlueZ reports its `Device1.Connected` false or the object gone. That
is what tells a central apart from a peripheral the central role connected
to: both are connected `Device1` objects, and only a central reads our
identity. It is announced to the core only while the Message characteristic
has a subscriber, because BlueZ reports subscriptions per characteristic and
not per device, and only while one of its centrals is still connected: bless
forgets a subscription only on `StopNotify`, which BlueZ sends when an
unpaired central disconnects but not a bonded one. The caller the hook reads
is the caller of the call bless dispatches next only because bless's
`ReadValue` and `WriteValue` are plain methods, which dbus_next runs inline;
a coroutine method would be scheduled after the next call's hook, so a test
reads the method dbus_next dispatches and refuses a coroutine. Each inbound write is attributed to its own caller, so two
centrals no longer collapse into `ble-peer`. Notifications go to every
subscribed central on BlueZ, which offers no per-central notification; on
macOS see P15.

The core's outbound fragment queue is shared by the two roles and offers no
peek and no requeue (`ble_return_fragment` is a no-op), so a fragment one
role pops is gone. Two drains running at once split a message's fragments
across two links. With both roles present, `ProtocolManager` runs one drain
that pops each fragment once and writes it on the central's link to its
recipient, else notifies it when the peripheral has a subscriber, else drops
and counts it; the core's retry of the message re-sends it. A write or notify
that fails is reported and the drain goes on, so one failure never strands the
fragments behind it. The peripheral on
its own takes nothing while nobody is subscribed. `test_ble_peripheral.py`
pins both with a fake bus. Nothing here has run on a board.

## P15. A peripheral link carries the address its hello proved, or it cannot start a session

**Invariant:** a fragment the peripheral hands to the core carries, as its
transport identity, the address the writing central proved with its hello,
and nothing else proves it. The core refuses a hop-0 control frame whose
sender is not the link's identity (`TRANSPORT_IDENTITY_MISMATCH`), and a key
package is such a frame. A link known by a central UUID, or by the
`ble-peer` placeholder, can carry ciphertext for a session that already
exists but can never start one. That is how an Android phone and a Mac found
each other, verified each other, and never formed a session: the Mac rotates
its address, the phone held a link to each address, so the Mac's peripheral
saw two centrals, attributed every write to `ble-peer`, and the core refused
the phone's key package.

The pieces, each of which the failure needs only one of to come back:

- **The writer of each write is named.** bless hands a CoreBluetooth write to
  its callback without its `CBCentral`. The peripheral builds bless's server
  with a subclass of its delegate (swapped into bless's module for that one
  construction, under a lock) that passes each request's central, offset and
  value through, and answers with the ATT code the peripheral returns. On
  BlueZ the bus hook of P14 names the writer.
- **The peripheral serves Hello** ([the chapter](../spec/ble-framing.md),
  writable with response) and runs the chapter's order: refuse an offset write
  (`0x07`) or a value outside 96 to 512 bytes (`0x0D`) before verifying;
  verify with the core's one verifier; bind the central to the derived address;
  never rebind; announce the peer only when no other link has, which includes
  the central role's client link (`ProtocolManager` hands the peripheral
  `BleManager.holds_peer`). A message's `sender` field never replaces a hello
  binding: it proves nothing.
- **The central writes its hello** after its Message subscription and before
  its first write, when the peer serves Hello and the assertion fits one write.
- **One client link per peer.** A second link that verifies as a peer already
  held is closed unannounced, on Android and in `BleManager`. Kept, two such
  peers fill Android's four connection slots, and the cap then refuses every
  inbound central.
- **On macOS a notification goes to the recipient's central** when that
  central said hello, through `updateValue:forCharacteristic:onSubscribedCentrals:`,
  and a `NO` from it (a full transmit queue) waits for the stack's ready
  signal and is retried. bless ignores that `NO`, which dropped fragments of
  every burst.
- **A dead client link is found.** CoreBluetooth kept a link to a Mac whose
  service process had restarted "connected" for over five minutes while every
  write into it vanished. `BleManager` reads each link's Device id back every
  15 seconds and drops a link whose read fails, hangs past 5 seconds, or names
  another address.
- **A Bluetooth stack that never comes up fails the start.** bless's
  CoreBluetooth delegate waits in its constructor, with no deadline, for a
  power-on that never comes when the process is not authorised (a service
  started over ssh). On the loop thread that froze the whole service past the
  local API server's start deadline. The server is now built on a daemon
  thread with a 20 second deadline, and a process macOS reports as denied or
  restricted is refused before the stack is touched.

`test_ble_hello.py` pins each of these, and each has a mutation that turns it
red. The macOS pieces were run between two Macs and a Galaxy M36 (Android 16).

## Testing

```bash
cd bindings/python
# build the desktop library first, then
pytest
```

Tests live in `bindings/python/tests/`. The build script under
`bindings/python/scripts/` produces the library the tests load.

## Note on packaging tests

Guard tests that assert on repository layout **panic in a packaged tarball**,
because the layout is not there. Keep such assertions out of the packaged test
set.
