# Bluetooth LE framing

## What this chapter is for

Every other chapter in this specification describes something that is true on
all carriers. This one is the exception: it describes how bytes cross one
particular radio, because on that radio the framing is part of the interop
surface rather than an implementation detail.

The reason is [leaf nodes](leaf-provisioning.md). A leaf does not own its
radio, so its firmware is what talks to a phone, and firmware that cannot
produce a conforming frame cannot pair no matter how correct everything above
it is. Until a device implemented this protocol, both ends of every BLE link
were built from the same source tree and the framing could stay an internal
agreement. It cannot any more.

Two layers are specified here, and firmware needs both:

1. **The GATT contract.** Which service, which characteristics, and which
   direction each one carries.
2. **The fragment codec.** How a message becomes ATT-sized pieces, and what a
   receiver owes when it reassembles them.

What sits above this is [the message model and wire format](wire-format.md):
the fragment payload is a hop-local encoding, and this layer neither reads nor
constrains it. What sits below is Bluetooth itself, which this document does
not restate.

Wi-Fi Direct has no equivalent chapter because it has no equivalent problem: it
carries a stream, so a whole message crosses in one write and there is nothing
to fragment.

## Invariants

Three things hold regardless of the mechanism, and a change that breaks one of
them is a wire break rather than a refactor:

- **Every write is framed.** There is no path on this radio that carries a bare
  message. A message small enough to fit one ATT write still crosses as a
  fragment with `total_fragments = 1`. A receiver that special-cases short
  writes will drop every message a conforming sender emits.
- **The index carries the order, not the arrival.** Fragments may arrive in any
  order and a receiver reassembles by index. Nothing in the format lets a
  receiver infer position from when a fragment turned up.
- **The header is not authenticated.** Nothing at this layer is signed, and
  everything in it comes from whoever is in radio range. Sender authenticity is
  established above, at the layer that owns it
  ([identity and addressing](identity.md)). Every bound in this chapter exists
  to keep a hostile header from costing memory or CPU, never to establish
  trust.

## The GATT contract

A device offers one primary service with three characteristics.

| Role | UUID |
|------|------|
| Service | `6E400001-B5A3-F393-E0A9-E50E24DCCA9E` |
| Message | `6E400002-B5A3-F393-E0A9-E50E24DCCA9E` |
| Device id | `6E400003-B5A3-F393-E0A9-E50E24DCCA9E` |
| Identity | `6E400004-B5A3-F393-E0A9-E50E24DCCA9E` |

These are the Nordic UART Service UUIDs, adopted rather than minted. That was
not a considered choice and it collides with any NUS peripheral in range, which
costs a discovery filter rather than a failure: a peer that answers the service
scan but serves none of the characteristics below is not a peer. Changing them
is a wire break and is not proposed here.

**Message** carries fragments in both directions. A central writes fragments to
it; a peripheral returns them through subscription. It MUST be writable without
response, and it MUST be subscribable.

**Device id** is read by a central to learn the peer's address. It MUST be
readable and carries the `off1…` address as UTF-8.

**Identity** is read to prove that address. It MUST be readable and carries the
signed identity assertion whose verification is specified in
[identity and addressing](identity.md); this chapter does not restate its
contents. The pair matters together: the device id says who a peer claims to
be, the identity characteristic is what makes the claim checkable, and a
central that reads the first without checking the second has learned nothing.

### Notify or indicate

The two shipped peripherals differ, and firmware should know why before
choosing.

A peripheral MAY offer notification, indication, or both. A central MUST
subscribe to whichever the peripheral offers, and MUST NOT require a particular
one.

A peripheral SHOULD offer **indication alone**. Notifications are not
flow-controlled, so a burst of fragments from a fast peripheral can out-run a
slower central's receive buffer and lose one fragment per pass, and a message
that loses one fragment per pass never reassembles. The failure is worst
exactly where it hurts most, on the first large message of a pairing. A central
that is offered both will generally prefer notification, so offering both is
how a peripheral opts into the fast path it cannot afford; offering only
indication is how it declines.

## The fragment codec

### Header

A fragment is a header followed by its slice of the payload. All multi-byte
integers are **little-endian**.

| Field | Size | Value |
|-------|------|-------|
| `magic` | 2 | `0x4F 0x50`, ASCII `OP` |
| `version` | 1 | `1` |
| `id_len` | 1 | Length of `message_id` in bytes, 1 to 255 |
| `message_id` | `id_len` | UTF-8, no terminator |
| `fragment_index` | 2 | Zero-based, little-endian |
| `total_fragments` | 2 | Little-endian, at least 1 |
| `data_len` | 2 | Length of `data`, little-endian |
| `data` | `data_len` | This fragment's slice of the payload |

The fixed part is 10 bytes, so a fragment's total header is `10 + id_len`.

A receiver MUST refuse a frame whose magic is not `OP`, and MUST refuse a
version it does not implement rather than guessing at the layout. Refusing an
unknown version is what makes the version byte usable later: a format change
takes a new number and a new vector file, and old software rejects the new
frames cleanly instead of misparsing them.

`message_id` groups fragments of one message and does nothing else. It is
**not** authenticated, and a receiver MUST NOT cross-check it against any
identifier inside the reassembled payload or draw any conclusion from it. It is
an assembly key.

### Payload

`data` concatenated in index order is a hop-local encoding of one message,
exactly as [the wire format chapter](wire-format.md) specifies: the JSON floor,
or the binary v1 codec when the peer negotiated it. A receiver distinguishes
them by the first byte of the reassembled payload, `0xF5` selecting binary v1,
and needs no negotiation state to do it. This layer never inspects `data`.

### Sizing

A sender chooses its payload size from the link:

```
max_fragment_payload = mtu - 10 - id_len
```

where `mtu` is the peer's negotiated maximum ATT write payload. The message is
split into `ceil(len / max_fragment_payload)` fragments; every fragment except
the last carries exactly `max_fragment_payload` bytes.

A sender MUST refuse to fragment when `10 + id_len >= mtu`, because there is no
room left for payload.

On this encoding a `message_id` is a hyphenated UUID, so `id_len` is 36 and the
real overhead is 46 bytes per fragment. At the 185-byte floor that leaves 139
payload bytes, which is the number that makes the encoding choice matter: the
same direct message takes ten fragments under the JSON floor and four under the
compact envelope plus binary codec (see
[Size and fragmentation](wire-format.md#size-and-fragmentation)).

### What a receiver owes

A receiver maintains one assembly per `message_id`. In order, before it stores
anything:

1. Refuse `total_fragments` above the cap. A peer MUST NOT be able to declare
   more fragments than the receiver would ever produce.
2. Refuse `fragment_index >= total_fragments`. This is the bound that matters
   most: without it a peer declares a small total and then fills the assembly
   with distinct out-of-range indices, none of which the size bound below
   catches because per-entry overhead is not payload bytes. It also refuses
   `total_fragments = 0`, an assembly that could never complete and never be
   freed.
3. Refuse a `total_fragments` that disagrees with what an existing assembly for
   the same id already recorded.
4. Refuse when the payload bytes buffered so far, plus this fragment, would
   exceed the maximum message size, and drop the whole assembly when that
   happens rather than holding its bytes until a timeout. The check MUST run as
   fragments arrive, not once an assembly completes; an assembly that never
   reaches its declared total would otherwise never be measured at all. A
   repeated index MUST replace rather than accumulate, so a retransmission
   cannot inflate the total.

A fragment with `total_fragments = 1` completes immediately and skips the
assembly table entirely.

Beyond those refusals a receiver needs two bounds on what it will hold, both
local policy rather than wire format. The shipped values are in the table
below.

An idle timeout evicts a partial assembly, measured from the **most recent**
fragment rather than the first. Measuring from the first would tear up a large,
slow message mid-flight for taking too long to arrive, which is a real message
lost to defend against a hypothetical one. The cost is that a peer dribbling
one fragment per interval holds an assembly open indefinitely; that is bounded
by the assembly cap rather than by the clock, and it is a deliberate trade.

An assembly cap bounds concurrent partial messages. When it is reached a
receiver SHOULD evict the assembly with the least progress, so a message one
fragment from completion is not discarded in favour of one that has barely
started.

### Constants

Shipped values. The first three are wire-visible and a peer must agree on them;
the rest are local policy, stated so an implementer has a working starting
point rather than a blank.

| Constant | Value | Meaning |
|----------|-------|---------|
| `FRAGMENT_VERSION` | `1` | The version byte in every header |
| `FRAGMENT_HEADER_FIXED` | `10` | Header bytes before `message_id` is added |
| `BLE_MAX_FRAGMENT_SIZE` | `185` | Payload floor when no MTU has been negotiated |
| `MAX_REASONABLE_BLE_PAYLOAD` | `512` | Clamp on a reported MTU |
| `BLE_MAX_FRAGMENT_COUNT` | `512` | Cap on `total_fragments` |
| `BLE_MAX_FRAGMENT_ASSEMBLIES` | `64` | Concurrent partial messages held |
| `BLE_FRAGMENT_TIMEOUT_SECS` | `30` | Idle window before a partial assembly is evicted |

The floor is the historical iOS auto-negotiated minimum ATT MTU. The clamp is
BLE 5's 517-byte maximum less the 3-byte ATT header, rounded down for margin. A
sender that has not yet learned a peer's MTU MUST use the floor rather than
guess upward.

## Conformance vectors

`crates/offline-protocol-transport/tests/data/ble-framing-v1.vectors.json`
carries frames in hex with the outcome each one requires: four reassembly cases
including out-of-order and duplicated indices, and nine refusals covering every
check above. They are computed from this chapter rather than from the
implementation, so a disagreement is evidence about one of them rather than two
copies of one mistake agreeing.

A vector that fails means the wire format moved, which needs a new
`FRAGMENT_VERSION` and a new vector file. Editing an expected value to make a
test pass converts a caught break into a shipped one.
