# 0040. A reference gateway ships with the SDK

**Status:** Proposed

## Context

The gateway contract had a complete device half, written three times (the
Swift, Kotlin and Python managers behind the `reticulum` slot), and no
gateway. The documentation said so: what answers on the daemon address "is a
deployment rather than something this SDK ships". So the slot never became
available anywhere, and every mention of LoRa in the SDK described a path
nobody could run.

Three things made a gateway more than a weekend's port of the relay:

- **The attach proof had one builder and no verifier outside the engine.** A
  gateway daemon runs no engine. Building the 103-byte payload itself is the
  second copy [ADR 0022](0022-one-sealed-layer-shared-with-the-leaf.md)
  refuses, and the copy that drifts silently (a length written little-endian,
  a challenge prefixed as if it were a second field) while its own round-trip
  test passes.
- **The only mature Reticulum is Python.** A device has no Reticulum stack,
  and the contract keeps it that way (ADR 0026). The gateway is where
  Reticulum has to run, and `reticulum-rs` does not yet promise wire
  compatibility with the reference.
- **The verdict has a deadline the backbone does not.** The reference clients
  fail a frame with no verdict in 60 seconds. A round trip across a LoRa link
  at long-range settings can approach that.

## Decision

1. **The SDK exposes the gateway half of the proof:**
   `verify_gateway_address_proof(address, public_key, signature, challenge)`,
   instance-less, in every binding. It checks the signature over the sealed
   crate's payload under RFC 8032's strict rules and that the key derives to
   the claimed address. The challenge's freshness stays the gateway's,
   because only the gateway holds connection state.
2. **A reference gateway lives in this repository, under `gateway/`, in
   Python**, beside the binding whose verifier it calls. It implements the
   five verbs and Deliver, and nothing the contract leaves optional: no
   mailbox, no push, so it never sets `stored` or `pushed`.
3. **Its backbone is Reticulum**, through a seam (`Backbone`) that an
   in-process implementation also fills for tests. Each gateway announces one
   destination under its own identity; peers are provisioned destination
   hashes; frames travel as link requests, which Reticulum carries as
   resources; presence is a snapshot on link-up and coalesced changes after.
   There are two links per pair, one dialled by each side, because a
   Reticulum request travels only from a link's initiator to the destination
   it dialled.
4. **A backbone verdict waits for the peer gateway's answer, up to 45
   seconds**, then reports `backbone_timeout`, a plain failure the device
   retries. It is never reported as `MessageSent` on the strength of a
   hand-off alone: a frame the peer gateway never confirmed is one the device
   must still retry, and the retry costs only a duplicate the core already
   removes.
5. **The scarce class's budgets are sized from a measurement**, not a
   guess: two SDK engines pairing over MLS and exchanging one message send
   18 to 20 backbone frames each, about 16 KB, the largest about 4 KB. Every
   burst holds that first contact, and the backbone frame bound (6 KiB)
   passes its largest frame. A broad backbone skips these budgets.
6. **The airtime toward a peer is budgeted in bytes, twice.** Once per peer
   gateway, shared by every device, because the airtime toward a peer is the
   scarce thing and many devices together can ask for more than the radio
   carries. And once per source host, held below the shared budget by
   validation, so one device cannot hold the shared budget at zero. The
   second is per host rather than per address because addresses cost
   nothing to mint: a host that re-keys would otherwise get a fresh burst
   each time. The defaults fit the example radio (SF8 at 125 kHz, 10 %
   airtime: about 1.5 KB a minute toward a peer), and a deployment on
   another radio resizes them.
7. **Requests to a peer go one at a time, queued in the daemon.** Reticulum
   sends one outgoing resource per link and cannot withdraw one it has
   queued, so a frame left to queue there goes on air after its device has
   been told it failed. Queued in the daemon, a frame whose time runs out
   first is refused without reaching the radio.

## Consequences

- The `reticulum` slot works end to end: the tests run two real engines
  through one gateway, across two gateways in one process, and across two
  daemon processes joined by a real Reticulum backbone over TCP. Swapping TCP
  for an RNode changes the Reticulum configuration, not the daemon.
- A first contact over LoRa is expensive, about 20 frames each way at about
  472 bytes and up. That is the SDK's session setup, not the gateway's, and
  the gateway now makes the number visible instead of hiding it in a
  hypothetical. By arithmetic from the radio settings, it is several minutes
  on the example radio and hours at the 1 % duty cycle most of the EU
  868 MHz band allows. Nothing has been measured over radios yet.
- Reticulum 1.6 mints a proof-of-work stamp before a destination's first
  announce. A new gateway is unreachable for seconds on a laptop and minutes
  on a small board, once per identity. The daemon logs it; the README says it.
- A device reaches another gateway's devices only while both are attached
  over IP. Re-originating backbone frames into a Bluetooth zone needs the
  gateway to run an engine of its own, and is not done.
- The repository now ships a server-side component with its own deployment
  story. The SDK's own CI does not run it yet.

## Alternatives considered

**A Rust gateway on `reticulum-rs`.** One language with the SDK, and the
verifier without an FFI hop. Rejected for now: interoperability with the
Python reference is the property a backbone needs, and it is not yet
promised.

**Verdict on hand-off.** Answer `MessageSent` as soon as the frame is handed
to a live link whose peer claims the recipient, which keeps every verdict
well inside the clients' 60 seconds. Rejected: it reports a frame forwarded
that may never have arrived, and the device stops retrying on that report.
The peer's own answer costs one more packet and is the honest one.

**Leave the gateway to deployments.** The contract alone is enough to build
one. But a contract with no implementation is not tested by anything, and the
first deployment would have found the 18-frame bootstrap and the announce
stamp in the field.

## What would undo this

A second builder of the proof payload in the gateway, in any language: the
verifier exists so that there is none.

A verdict reported `MessageSent` before the peer gateway answered.

A scarce-class burst smaller than a first contact, or a backbone frame bound
smaller than a first contact's largest frame. Either looks like a tighter
budget and is a pairing that stalls on its own frames: intermittently for
the burst, always for the bound.

Handing Reticulum a second request on a link before the first resolves. It
works on TCP, where transfers finish before anything times out, and on a
radio it puts frames on air that their devices were told had failed.
