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
signed identity assertion specified below. The pair matters together: the
device id says who a peer claims to be, the identity characteristic is what
makes the claim checkable, and a central that reads the first without checking
the second has learned nothing.

### The identity assertion

The Identity characteristic carries one fixed-layout value:

| Field | Size | Value |
|-------|------|-------|
| `public_key` | 32 | The peer's Ed25519 identity public key |
| `signature` | 64 | Ed25519 signature by that key over `signed_data` |
| `signed_data` | remainder | The bytes that signature covers |

A reader MUST refuse a value shorter than 96 bytes. `signed_data` is whatever
follows the signature, and may be empty.

Verification runs in this order, and any failure means the peer is not
surfaced at all rather than surfaced with a caveat:

1. Parse `public_key` as an Ed25519 verifying key.
2. Verify `signature` over `signed_data` under it.
3. Derive an address from `public_key` using the single derivation in
   [identity and addressing](identity.md#address-derivation).
4. Compare that address to the Device id characteristic's string. The
   comparison is exact. Addresses are canonical bech32m, so a value differing
   only in case belongs to a peer that did not derive its own id the way this
   one did, and is refused rather than normalised into agreement.

There is no accept-but-flag state, because what this produces is not a label:
it becomes `Message.recipient` on every outbound frame and the
transport-verified peer id the receiving core matches `Message.sender`
against. The address announced is always the **derived** one, never the string
read from Device id. The two are equal whenever verification succeeds, so
using the derived value costs nothing, and it means no code path can announce
a name that arrived unauthenticated.

Two limits are deliberate, and firmware should not read more into a verified
assertion than it proves.

**`signed_data` carries no meaning at this layer.** The shipped peripherals
put their mesh advertisement there; the shipped centrals never decode it,
never compare it against the advertisement received over the air, and never
feed it into routing. It is a message to sign, and the signature over it
proves possession of the private key for `public_key`, which is what makes
step 3 mean anything: deriving an address without checking a signature proves
nothing, because anyone can copy a public key. Nothing domain-separates these
bytes, so any signature that key ever produced over any message verifies here.
That is harmless only while `signed_data` is uninterpreted. Giving it meaning
requires adding a domain separator first, and that is a wire break.

**The assertion is static, so it is replayable.** Nothing in the read is
challenged or timestamped, so a device in radio range can serve a value it
copied from another peer and bind its own link to that peer's address. What it
cannot do is produce the frames that address must sign, so the exposure is a
link labelled with a name its holder cannot use, and message authenticity is
established above by the layer that owns it. A receiver MUST NOT treat a
verified assertion as evidence that the peer is live, is recent, or is the
only holder of that address.

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

Two frames are malformed on this encoding without being refused. A sender MUST
NOT emit `id_len = 0`, because an empty assembly key is not a key: every such
message from every peer in range merges into one assembly and none of them
reassembles. A sender MUST NOT emit trailing bytes past `data_len` either. The
reference receiver enforces neither, accepting the empty key and reading no
further than `data_len`, so firmware MUST NOT infer from that silence that
either frame conforms. Both cost the sender its own message, which is why the
receiver spends nothing rejecting them.

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
   exceed `DEFAULT_MAX_MESSAGE_SIZE` (in the constants table below, 1 MiB on
   this encoding), and drop the whole assembly when that
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

Shipped values. **Agreed** means a peer must use the same value: a receiver
that picks its own drops messages a conforming sender is entitled to produce,
which is an interop break between two implementations that each believe they
conform. **Local** means the value is one implementation's policy and a peer
cannot observe it, stated here so an implementer has a working starting point
rather than a blank.

| Constant | Value | Kind | Meaning |
|----------|-------|------|---------|
| `FRAGMENT_VERSION` | `1` | Agreed | The version byte in every header |
| `FRAGMENT_HEADER_FIXED` | `10` | Agreed | Header bytes before `message_id` is added |
| `BLE_MAX_FRAGMENT_SIZE` | `185` | Agreed | Payload floor when no MTU has been negotiated |
| `BLE_MAX_FRAGMENT_COUNT` | `512` | Agreed | Cap on `total_fragments`, both emitted and accepted |
| `DEFAULT_MAX_MESSAGE_SIZE` | `1048576` | Agreed | Ceiling on one reassembled message, in bytes |
| `MAX_REASONABLE_BLE_PAYLOAD` | `512` | Local | Clamp on a reported MTU |
| `BLE_MAX_FRAGMENT_ASSEMBLIES` | `64` | Local | Concurrent partial messages held |
| `BLE_FRAGMENT_TIMEOUT_SECS` | `30` | Local | Idle window before a partial assembly is evicted |

`BLE_MAX_FRAGMENT_COUNT` is agreed rather than local because both sides use it:
a sender refuses to emit more than 512 fragments and a receiver refuses to
accept a larger declared total, so a receiver that lowered it unilaterally
would reject messages a conforming sender emits.

The floor is the historical iOS auto-negotiated minimum ATT MTU. The clamp is
BLE 5's 517-byte maximum less the 3-byte ATT header, rounded down for margin. A
sender that has not yet learned a peer's MTU MUST use the floor rather than
guess upward.

### The ceiling this radio actually has

The two agreed bounds are both ceilings on one message, and on this carrier the
fragment count binds first:

```
max_message_on_ble = BLE_MAX_FRAGMENT_COUNT * (mtu - 10 - id_len)
```

With a UUID `message_id`, that is 512 x 139 = 71168 bytes at the 185-byte
floor, and 512 x 466 = 238592 bytes at the 512-byte clamp. Both are far under
`DEFAULT_MAX_MESSAGE_SIZE`, so a payload that the message layer accepts can
still be unsendable over Bluetooth LE, and it fails at the sender rather than
in flight. A firmware author sizing anything large, an MLS Welcome into a big
group most of all, needs that number rather than the 1 MiB one.

## Conformance vectors

`crates/offline-protocol-transport/tests/data/ble-framing-v1.vectors.json`
carries frames in hex with the outcome each one requires: four reassembly cases
including out-of-order and duplicated indices, and nine refusals. They are
computed from this chapter rather than from the implementation, so a
disagreement is evidence about one of them rather than two copies of one
mistake agreeing.

The refusals cover every check above except the size bound, which no practical
vector can reach: exceeding 1 MiB takes at least seventeen full 64 KiB frames,
and a vector file that carried them would be megabytes of hex pinning
arithmetic that a unit test pins for free.
`test_ble_reassembled_payload_rejects_oversized` in
`crates/offline-protocol-transport/src/ble.rs` covers that bound instead, and
pins the incremental part of it: rejection fires on the fragment that crosses
the limit, not at completion.

A vector that fails means the wire format moved, which needs a new
`FRAGMENT_VERSION` and a new vector file. Editing an expected value to make a
test pass converts a caught break into a shipped one.
