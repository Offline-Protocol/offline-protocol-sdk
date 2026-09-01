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
`DataStore.wipe_all()` too. Stop the protocol first: there are no deletion
tombstones, so a wipe on a running engine with live sessions is undone by the
peer's next version offer, which recreates and refills every document.

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
