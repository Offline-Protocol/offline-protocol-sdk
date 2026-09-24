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
way the core sees one announcement and one loss per address.

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
peer and aborts the stream once the peer has taken no byte for
`write_timeout` (thirty seconds). A single drain that awaited each write
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
