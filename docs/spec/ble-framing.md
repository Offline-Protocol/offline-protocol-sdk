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

A peer stream (Wi-Fi Direct, a LAN socket) has nothing to fragment, so it
has no equivalent of the codec here. What it lacks instead is the identity
GATT serves and the message boundary an ATT write gives for free, and
[peer-stream framing](stream-framing.md) supplies both, reusing the identity
assertion below unchanged.

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

One more holds for the links that carry the frames, and it is a property of
the radio rather than of this format:

- **Two devices hold one Bluetooth LE link between them.** The Link Layer
  does not complete a second connection between two devices that already have
  one (Core 5.4, Vol 6 Part B, 1.1.1), so a dial from an address the peer
  already holds a link with times out, however often it is retried. A second
  link can still form when one side's random address has rotated, because the
  pair then looks new to both radios, and once it has formed neither side can
  reconnect after the other link drops until an address rotates again. So a
  device SHOULD close a link that proves the identity of a peer it already
  holds a usable link to (a client link of its own, or the peer's subscribed
  link to its server), and MUST NOT report a peer lost, or dial it, while such
  a link is up. In run 5 of the 2026-10-19 volume test both rules were missing: the
  phones held two links, the one that stalled was torn down, the redial of
  its address timed out and reported a peer lost that was still linked, and
  the pair was apart for ten minutes
  ([ADR 0030](../adr/0030-one-bluetooth-le-link-per-peer-pair.md)).

## The GATT contract

A device offers one primary service for each application on it that runs this
protocol, with three required characteristics and two optional ones.

| Role | UUID | Required |
|------|------|----------|
| Service | `6E400001-B5A3-F393-E0A9-E50E24DCCA9E` | |
| Message | `6E400002-B5A3-F393-E0A9-E50E24DCCA9E` | yes |
| Device id | `6E400003-B5A3-F393-E0A9-E50E24DCCA9E` | yes |
| Identity | `6E400004-B5A3-F393-E0A9-E50E24DCCA9E` | yes |
| App tag | `6E400005-B5A3-F393-E0A9-E50E24DCCA9E` | no |
| Hello | `6E400006-B5A3-F393-E0A9-E50E24DCCA9E` | no |

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

**App tag** names the application behind this instance of the service. It MAY
be absent: a device running one application loses nothing by omitting it. When
present it MUST be readable and carries eight bytes, the first eight of
`SHA-256("offline-protocol-ble-app-tag-v1" || 0x00 || app_id)`, where `app_id`
is the application id of [the wire format](wire-format.md) as UTF-8. It proves
nothing, and is read only to choose between instances, below.

**Hello** is written by a central to say who it is on the link it opened. It
MAY be absent; when present it MUST be writable with response. The rest of
the contract proves the peripheral to the central and nothing the other way,
so a peripheral that receives Message writes from a central it cannot map has
had only one way to learn who is writing: dial the central's address and run
the read side itself. That address is the central-role one, which need not be
the address the device advertises, so the dial can take tens of seconds, and
every fragment the central wrote waits for it. The hello removes the dial:

- A central that finds Hello on the instance it chose SHOULD write it once,
  after its subscription to Message is acknowledged and before its first
  Message write, as one write that fits the negotiated MTU. The value is the
  identity assertion below, the same value the central serves on its own
  Identity characteristic. A central that cannot fit it in one write, or has
  no assertion yet, skips it; a refused write is not a reason to drop the
  link.
- A peripheral MUST refuse a prepared (long) write, a write at a non-zero
  offset, and a value shorter than 96 or longer than 512 bytes, before
  verifying anything.
- It MUST verify the value with the one verifier, steps one to three below.
  There is no Device id string to compare against in step four: the claim is
  the derived address itself. It binds the writer's link to the **derived**
  address only, and announces the peer to the core only if no other link has
  announced it already.
- A value that fails verification is ignored, and the link is resolved as it
  would have been without one. So is a hello on a link already bound to a
  different address: a hello never rebinds a link.

A hello names a link for the life of the peripheral process that verified
it, which can be shorter than the life of the link. An iPhone restores its
links when its app restarts, and the new process has no binding for a
central that said hello to the old one, while nothing changes for the
central. The shipped Android central drops a link that takes its writes and
returns nothing ([transport lifecycle](../state-machines/transport-lifecycle.md#per-transport-constraints)),
and the redial writes the hello again. That is local policy, not wire format.

The shipped iOS, Android and Python peripherals serve Hello. A central that
can never be dialled back (a browser over Web Bluetooth, which serves no
GATT at all) is named only by its hello, so a peripheral that leaves Hello
out cannot attribute anything such a central writes.

The hello carries the same static assertion as Identity, with the same limits
below: it proves possession of the key once, and a device that copied a
peer's assertion can label its own link with that peer's address.

### The node hash in the scan response

A device MAY carry, as service data under the Service UUID, exactly eight
bytes: the first eight of SHA-256 over its `off1…` address as UTF-8
(`offline_protocol_transport::node_hash`). It names the advertiser of a
signal reading only, and a scanner MUST match it only against peers that
already proved their address on a link: it proves nothing itself, and it
never starts a link or names a new peer. A scanner MUST NOT read service data
of any other length under this UUID as a node hash. Service data of 19 bytes
or more under this UUID is already read by the shipped scanners as mesh
metadata (`MeshAdvertisementData`, version byte `0x02`), so the lengths a
current scanner ignores are 1 to 7 and 9 to 18. A later form (a token per
transport start, delivered over the link, as
[R29](../security/threat-model.md#r29-the-bluetooth-le-scan-response-names-the-device-and-rssi-is-unauthenticated)
describes) MUST use one of those, 9 to 18 bytes, so that no current scanner
misreads it.

The shipped Android peripheral puts it in its scan response when the
application sets `transports.ble.advertiseNodeHash` (off by default) and its
identity is open, as the 26-byte service data structure (2 + 16 + 8) in place
of the 18-byte Service UUID list, because both do not fit in 31 bytes; the
main advertisement still lists the UUID, and the shipped scanners accept
either as a mesh device. Service data under this 128-bit UUID needs no
Bluetooth SIG assignment, which a 16-bit service UUID or a company id for
manufacturer data would. iOS cannot advertise service data, so an iPhone
carries no node hash. The exposure is
[R29](../security/threat-model.md#r29-the-bluetooth-le-scan-response-names-the-device-and-rssi-is-unauthenticated).

### Several instances behind one link

**One link carries one peer.** A phone that runs several applications on this
protocol serves one instance of the service per application, and both iOS and
Android merge every application's service into one GATT database, so a central
sees all of them behind a single connection. The link still carries one peer,
because a peripheral attributes an inbound write to its sender by the link it
arrived on. A second identity on the same link would have its hop-0 control
frames refused by the receiving core's transport-sender check.

One engine instance is one identity and one tag. An instance that serves
several local applications does so behind its configured application id: a
per-send application id changes the `app_id` stamped on that one frame, never
the tag this service advertises and never the storage the identity lives in.

So a central that finds more than one instance MUST choose one before it reads
Device id or Identity, and MUST then read, subscribe and write on that instance
only. Among the instances that expose both Device id and Identity, it takes:

1. the first whose App tag equals its own;
2. otherwise, when exactly one has no App tag (absent, or its read failed),
   that one, since it is a build that predates the tag;
3. otherwise the first.

When no instance exposes both, it takes the first, and the handshake refuses it
for the characteristic it lacks. "First" is the order the platform reports the
instances in. A central that finds one instance has nothing to choose, and the
shipped centrals do not read App tag at all in that case. The shipped
centrals include the browser's Web Bluetooth carrier, which can never be
dialled back: it writes Hello before any Message write, so a peripheral that
serves Hello can attribute everything it sends. For the same reason it
refuses an instance with no Hello, and a link whose hello was refused,
where the rule above lets a central keep them.

The choice never refuses a link. A phone running only other applications is
still a mesh neighbour: relay forwarding is keyed on the recipient, never on the
application, so refusing it would cost relaying to fix discovery. Rule 3 is what
a central did before App tag existed.

A choice lasts one connection. If the chosen instance disappears while the link
is up, because its application quit or re-registered, a central SHOULD drop the
link and handshake again rather than move to another instance: the link was
announced under the chosen instance's identity, and another instance belongs to
another application.

App tag is not signed. Any identity key could sign any tag, so a signature would
prove nothing the Identity characteristic does not. An application on the peer's
phone that serves another's tag is chosen in its place, and is still bound
under its own proved address, never the displaced one's. That, and what the tag
reveals to an observer, is residual risk
[R15](../security/threat-model.md#r15-the-bluetooth-le-app-tag-is-unsigned-and-readable).

### The identity assertion

The Identity characteristic carries one fixed-layout value:

| Field | Size | Value |
|-------|------|-------|
| `public_key` | 32 | The peer's Ed25519 identity public key |
| `signature` | 64 | Ed25519 signature by that key over `signed_data` |
| `signed_data` | remainder | The bytes that signature covers |

A reader MUST refuse a value shorter than 96 bytes. `signed_data` is whatever
follows the signature, and may be empty.

The layout has one implementation. `offline-protocol-sealed` joins and splits
it (`encode_identity_assertion`, `parse_identity_assertion`), and the one
verifier, `verify_identity_assertion`, runs steps one to three below and
returns the derived address; step four, the comparison to a claim, is the
caller's, because the verifier is handed only the assertion. Bridges reach
it as a namespace-level FFI function, and a peripheral builds its value with
`identity_assertion` on the instance. A
binding that splits the 96 bytes itself is a second copy of this chapter, and
the Python peripheral used to serve a JSON document here for exactly that
reason. `crates/offline-protocol-sealed/tests/data/identity-assertion-v1.vectors.json`
pins the layout with the published RFC 8032 vectors, so every assertion in it
verifies under its own key.

Verification runs in this order, and any failure means the peer is not
surfaced at all rather than surfaced with a caveat:

1. Parse `public_key` as an Ed25519 verifying key.
2. Verify `signature` over `signed_data` under it, with RFC 8032's strict
   rules: a small-order key or a non-canonical signature is refused. The
   permissive check accepts the identity point as a key with a signature
   anyone can write down, over any message.
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

The shipped peripherals (iOS, Android, and the Python service, which offers
notification) differ, and firmware should know why before choosing.

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

The identity assertion has its own file,
`crates/offline-protocol-sealed/tests/data/identity-assertion-v1.vectors.json`,
generated by `tools/spec-vectors/generate.py` like the other sealed vectors:
three assertions whose key, signature and message are RFC 8032 section 7.1
test vectors 1 to 3 verbatim, one of them exactly 96 bytes, and three
refusals under the floor. The signatures are pasted from the RFC, never
computed, so a verifier in any language that accepts them and returns the
pinned address has been checked against a source outside this repository.
