# The gateway contract

A **gateway** bridges a zone (a governed BLE / Wi-Fi Direct flood) to somewhere
a zone cannot reach: the internet, or a wide-area backbone. This document
specifies what a gateway must do to be one, the wire protocol a device speaks
to a gateway daemon over local IP, and what a backbone owes a gateway. The
backbone is the gateway's property: a device never touches one, and learns
that one exists only as a token at attach.

The contract is deliberately a *reframing* rather than an invention. The
internet relay already implements every verb below; naming them is what lets a
second kind of gateway plug into machinery that was written for the first.

## Invariants

These outrank every mechanism in this document. A gateway implementation that
preserves the mechanisms but breaks an invariant is wrong.

**1. Only the recipient settles a message.** Delivery settles on the
recipient's end-to-end acknowledgement, or terminally on outbox expiry. A
gateway accepting a frame, forwarding it, or reporting anything about it settles
nothing. This is what makes a lying or broken gateway a latency-and-battery
problem instead of a data-loss problem, and it is why every other invariant here
can be as permissive as it is.

**2. Layer 1 stays role-blind.** A gateway inside its zone is an ordinary peer.
No frame is marked "for the gateway", no forwarding decision consults a role,
and the flood has no gateway state machine. Being powered and stationary, a
gateway wins forwarding races through the same continuous relay bias as a
charging phone, which is a scoring outcome and not a role.

**3. No facts, no change.** Absent any claim from a gateway, a device MUST
behave exactly as it would with no gateway present. A gateway's answers may open
paths and economise retries; they MUST NOT be required for correctness.

**4. End-to-end encryption regardless of path.** Every gateway is an untrusted
transport that sees ciphertext plus routing metadata. A backbone's own link
encryption is a transport-layer courtesy and forms no part of the trust
argument.

## The five verbs

A carrier is a gateway if and only if it implements these five. The contract is
semantic: each gateway type MAY carry it over its own wire.

| Verb | Meaning |
|------|---------|
| Attach | Establish an authenticated session bound to the device's `off1` address |
| Submit | Accept one frame for a named `off1` recipient |
| Verdict | Answer, per recipient, whether that frame was forwarded or the recipient is unreachable; refuse rather than silently hold |
| Presence | Answer and watch per-recipient reachability |
| Capabilities | Advertise what this gateway can do, at attach time |

### The internet relay already implements all five

| Verb | Relay mechanism |
|------|-----------------|
| Attach | WebSocket + JWT, plus an address declaration over `offline-relay-addr-v1` (`AddressDeclared` / `AddressError`) |
| Submit | `SendMessage` / `MessageSent` |
| Verdict | `DeliveryError`, classified at the SDK boundary to the `recipient_unreachable` token |
| Presence | `CheckPresence` / `PresenceStatus`, against an SDK-owned watchlist |
| Capabilities | Capability tokens delivered on `Authenticated` |

So "relay-as-gateway" requires no server work. Everything the SDK already does
with the relay's answers (parking, escalating probes, mesh offers,
presence-driven flush) is gateway-verdict machinery that predates the name.

### Verdict is the load-bearing verb

It is the reason the contract exists. A device's transport policy is otherwise
blind to where the recipient is: a carrier being up counts as reachable for
every recipient. The verdict is the only thing that contradicts that.

- A gateway daemon MUST implement Verdict, whatever backbone stands behind it,
  or it is not a gateway. This is what closes the daemon half of the
  mixed-neighbourhood residual, recorded against Reticulum when that was the
  only backbone.
- **Nostr structurally cannot implement Verdict.** A broadcast relay reports no
  per-recipient delivery. Nostr therefore remains a carrier and never becomes a
  gateway, and its half of the residual is permanent. This is recorded so the
  gap stops being rediscovered as a bug; see
  [ADR 0017](../adr/0017-nostr-is-a-carrier-not-a-gateway.md).

Verdicts are **claims, not settlement**, and they are unauthenticated. A verdict
MAY open a path (a mesh offer, a probe) and MAY economise retries. A verdict
MUST NOT close a path, and a "reachable" claim MUST NOT suppress the
acknowledgement ladder. That rule is the entire answer to their being
unauthenticated: a hostile gateway lying in either direction costs delay, never
delivery.

## Gateway-daemon contract v1

The wire protocol between a zone device and a gateway daemon reachable over
local IP (venue Wi-Fi, or a hotspot the gateway box provides).

This promotes the protocol the iOS, Android and Python clients already speak
to a configurable `daemonAddress` (default `localhost:4242`), rather than
designing a fresh one: the client side is already implemented three times,
and every field this chapter adds to it is additive, so a daemon built to the
earlier shape is unaffected.

### Framing

Newline-delimited UTF-8 JSON objects, one per line. Each object MUST carry a
`type` field. A receiver MUST ignore an object whose `type` it does not
recognise, and MUST NOT close the connection because of one: that is what allows
a version to add messages without a flag day.

A client MAY bound the length of a line, because a line that never ends is
memory it cannot reclaim and a line past its buffer cannot be resynchronised.
The reference clients abandon the connection past 1 MiB, so a gateway MUST NOT
emit a longer line; the largest frame a device can be handed is well inside
that.

### Versioning

`Identify` carries `protocol_version`, an integer, `1` for this document. The
daemon answers with its own. A daemon that does not recognise the client's
version MUST still answer, so the client can decide; a client that does not
recognise the daemon's version SHOULD proceed and rely on unknown-type tolerance
above.

### Attach

The full sequence, in order. `Capabilities` and `StatusUpdate` are separate
sections below; they are shown here because the order is what a client
implements against:

```
→ Identify
← Challenge
→ DeclareAddress
← AddressDeclared | AddressError
← Capabilities
← StatusUpdate(connected)
```

A gateway MUST send `StatusUpdate(connected)` once it has bound the session,
MUST send `Capabilities` before it, and MUST NOT send it while a declaration it
has received is unanswered. A client SHOULD treat `StatusUpdate(connected)` as
the point at which the carrier becomes usable, and MAY treat one that arrives
before its own declaration has been answered as a failed attach and close: a
session announced before it is bound is the verdict-only session described
below, and a gateway that announces it is not speaking this contract. A client
that starts submitting earlier is not wrong, but it is submitting into a gateway
whose features it has not been told.

A gateway MAY still send the same two frames on a session that never declared,
after a grace period, for clients that do not attach; such a session is
verdict-only. That grace SHOULD be no shorter than ten seconds, the attach
timeout the reference clients use, so a client whose declaration is merely slow
is not announced out from under it and closed by its own rule above.

**A client SHOULD NOT offer an unbound session to its transport selector.** A
session whose declaration was refused, or never made, may submit and be told a
verdict and is never registered as a recipient, so nothing addressed to that
device arrives over it. This differs from the internet relay, where an
undeclared connection keeps delivering on established sessions in account-name
space; a gateway has no such space.

```
→ {"type":"Identify","device_id":"<off1…>","protocol_version":1}
← {"type":"Challenge","challenge":"<base64, 32 bytes>","protocol_version":1}
→ {"type":"DeclareAddress","address":"<off1…>","public_key":"<base64>","signature":"<base64>"}
← {"type":"AddressDeclared","address":"<off1…>"}
  or
← {"type":"AddressError","reason":"<text>"}
```

The signature is over the canonical bytes

```
"offline-gateway-addr-v1" ‖ u32be(len(address_utf8)) ‖ address_utf8 ‖ challenge
```

matching the relay's address-declaration layout exactly, under a **different
domain**. The domain MUST be `offline-gateway-addr-v1` and MUST NOT be
`offline-relay-addr-v1`: a shared domain would let a signature harvested by a
hostile gateway be replayed against the relay, and vice versa. The domain itself
is not length-prefixed, so it MUST remain mutually non-prefixing with every
other domain (see [Signing domains](username-discovery.md#signing-domains)).

The address is the **only** length-prefixed field: the challenge is appended
raw, and the one prefix already fixes where the address ends. Prefixing the
challenge as well is the plausible misreading, and it produces a payload four
bytes longer that no conforming verifier accepts, while an implementation's own
round-trip test agrees with itself and reports nothing. For a 44-character
address and a 32-byte challenge the payload is therefore 103 bytes.

A worked example, which the
[conformance vectors](conformance.md#the-vectors) carry with two more:

```
address    off1qysluvwl5922yctzd0u9gpr06gn3k7ldfvgtwgvn
challenge  000102030405060708090a0b0c0d0e0f101112131415161718191a1b1c1d1e1f
payload    6f66666c696e652d676174657761792d616464722d7631   "offline-gateway-addr-v1"
           0000002c                                         address length, u32be (44)
           6f6666317179736c7576776c…                        the address, UTF-8
           000102…1f                                        the 32 challenge bytes
```

The address is inside the signed bytes deliberately. A signature over a bare
gateway-chosen challenge would be a signing oracle: the gateway picks challenge
bytes that are also a valid control-frame payload and replays the result.

Both ends have something to verify, and neither check substitutes for the other.

**The gateway** MUST verify all three of the following before answering
`AddressDeclared`, and MUST answer `AddressError` otherwise:

1. The signature verifies over the canonical bytes above under `public_key`.
2. `address` is the address derived from `public_key`. Addresses are
   self-certifying (bech32m over `0x01 ‖ SHA-256(ed25519_pub)[..20]`), so this
   is the check that makes the binding mean anything: without it, a signature
   made under any key the sender holds attaches the session to any address it
   cares to name.
3. `challenge` is the one this gateway minted for this connection, and has not
   been accepted before. A challenge honoured twice turns one captured
   `DeclareAddress` into a reusable credential.

A gateway that skips these attaches sessions under addresses the attaching
device does not control, which is how a hostile device draws another device's
inbound traffic to itself and poisons the presence answers given about it.

**The device** MUST verify that the address in `AddressDeclared` is its own. A
mismatch is a security event, not a retry: it means the gateway bound the
session to an address this device does not control.

### Submit and Verdict

```
→ {"type":"SendMessage","recipient":"<off1…>","content":"<base64>","encoding":"base64","message_id":"<id>","reply_to_msg":"<id>"}
← {"type":"MessageSent","message_id":"<id>","recipient":"<off1…>","pushed":false,"stored":false}
  or
← {"type":"DeliveryError","message_id":"<id>","recipient":"<off1…>","reason":"recipient_unreachable: <text>","stored":false}
```

A gateway MUST answer every `SendMessage` with exactly one of these. Silence is
not permitted: the sender's outbox holds the message either way, but a gateway
that neither forwards nor refuses converts a routing decision into a timeout.

`message_id` on the request is **optional and client-chosen**. A gateway MUST
echo a client-supplied id verbatim on the verdict, and MUST mint one when the
request carries none or carries one it will not accept. It MAY bound what it
accepts; the shape both implementations use is 1 to 64 characters of
`[A-Za-z0-9._-]`, which a UUID satisfies.

**A client MUST correlate by id and MUST NOT correlate by order.** A gateway
answers submissions as their routing resolves, which is not the order they
arrived: one recipient is attached locally and answers immediately, the next
needs a backbone query. A client that sends no id therefore cannot correlate at
all, because the id it is answered under is one the gateway chose. That is the
reason to send one, and the reason a gateway that silently rewrote an id it
disliked would be worse than one that rejected the submission outright.

`recipient` on both verdicts is the address the verdict is about. Additive, and
a client that does not read it is unaffected; a client that does can act on the
recipient without keeping its own id-to-recipient map.

`pushed` and `stored` are **optional booleans, and both default to `false`
when absent.** A gateway that implements neither omits both and is a conforming
gateway; a client MUST treat a missing key as `false`, never as unknown.

- `pushed` (on `MessageSent`) says the gateway had no live session for the
  recipient and handed the ciphertext to a device push instead. The frame was
  accepted but not delivered to a session, so a client MUST NOT treat this
  `MessageSent` as proof of arrival.
- `stored` (on either verdict) says the gateway has kept a copy of this frame
  and will re-send it on the recipient's next attach. A gateway that sets it
  MUST actually redeliver, and MUST advertise the capability that names its
  mailbox (`mailbox_v1` on the internet relay) at attach time. A gateway
  whose store failed MUST omit it, because a client reads it as a commitment
  and stops probing on the strength of it.

**Both are statements about the one frame the verdict names, never about the
recipient.** A gateway MUST NOT set `stored` on a verdict to mean that other
frames to the same recipient are also held. This matters because a
`DeliveryError` resolves every frame a client had in flight to that recipient,
so a client applies the flag to the named id alone and reports the rest as
ordinary unreachable. It follows that a gateway holding several frames MUST set
`stored` on each frame's own verdict.

`reason` MUST begin with `recipient_unreachable` when the recipient is not
reachable through this gateway. That token is the one the SDK classifier matches
by prefix, and it drives parking, the mesh offer and the escalating probe. Any
trailing prose after the token is for human logs and MUST NOT be relied on: the
SDK discards it at the classification boundary and never carries it into an
event.

Every other reason is a plain failure to the SDK, which puts the frame back on
its retry ladder and acts on nothing else in the text. Tokens a gateway may use
for those, none of which is normative: `attach_required` (the session is not
bound), `backbone_timeout`, `budget_exceeded`, `frame_too_large`,
`malformed_request`.

### Deliver

```
← {"type":"MessageReceived","sender":"<off1…>","content":"<base64>","encoding":"base64","message_id":"<id>","reply_to_msg":"<id>"}
```

`message_id` and `reply_to_msg` are additive and MAY be absent. They carry the
sender's own correlation forward so a gateway does not have to be the place
where it is lost; nothing in delivery depends on either, and the frame
authenticates itself regardless.

A frame the gateway holds for this device, handed to it over the attach
connection. `encoding` is optional; absent means UTF-8 text, and every binary
frame uses `base64`.

Delivery is deliberately **not** a sixth verb. It is what any carrier does, not
what makes a carrier a gateway, and the internet relay's own inbound path is not
a verb either. It is specified here because the daemon link is the only inbound
path a device attached over local IP has: such a device need not be inside the
flood a gateway re-originates into, so a daemon built to the five verbs alone
would attach devices, accept their frames, answer their verdicts, and never
deliver anything to them. A gateway MUST implement it.

`sender` is a **claim** by the gateway, with the same standing as a verdict. It
supplies peer attribution and reachability, and it MUST NOT be read as
authentication of the frame's origin: what authenticates a frame is the frame,
on this carrier exactly as on every other.

### Presence

```
→ {"type":"CheckPresence","peers":["<off1…>", …]}
← {"type":"PresenceStatus","peer":"<off1…>","online":true,"last_seen_ms":1786924800000}
```

A gateway MAY answer unsolicited when a watched peer's state changes. Presence
answers are claims with the same standing as verdicts: they may open a path, they
may not close one, and they decay. They decay faster, on a shorter TTL than a
verdict, because presence is a statement about a moment and a verdict is a
statement about an attempt.

### Capabilities

```
← {"type":"Capabilities","tokens":["gateway_v1","backbone_reticulum_v1"]}
```

Delivered at attach, before the device is told the carrier is available.
Tokens are opaque strings compared exactly. A receiver MUST bound what it
stores: at most 64 tokens of at most 128 bytes each, matching the relay
capability rules in [capability negotiation](capability-negotiation.md#relay-capabilities).
Capabilities MUST be cleared when the session drops, or a stale advertisement
outlives the gateway that made it.

Capability tokens MUST NOT drive a security decision. They gate features, and
an attacker who can set them is already the gateway.

`backbone_reticulum_v1` is one member of the family
[`backbone_<kind>_v1`](#the-backbone-token), which is how a gateway says what
stands behind it. The family's rules, including that a client ignores a kind
it does not know and that a gateway may advertise no backbone at all, are in
the backbone section.

### Status

```
← {"type":"StatusUpdate","status":"connected"|"degraded"|"disconnected"}
```

Advisory for delivery: a device MUST NOT treat `StatusUpdate` as a delivery
signal for any particular frame. `connected` is also the frame that completes
an attach, with the obligations [Attach](#attach) places on it.

### No new zone control prefixes

The daemon protocol is its own wire between a device and its gateway. It
introduces **no** new reserved control-message prefixes, and therefore adds no
entries to the [reserved prefix registry](control-messages.md#reserved-prefix-registry).
A gateway that wants to speak to the zone speaks ordinary frames.

## The backbone

Between gateways runs a wide-area carrier this document calls the backbone.
It is the gateway's property, never the device's: a device opens no backbone
link, holds no backbone identity, and learns that a backbone exists only as a
capability token at attach. Everything in this section binds the gateway. The
device half of this chapter is complete without it, which is what lets a
gateway change its backbone, or run with none, and have no device notice more
than a token.

### What a backbone owes a gateway

A carrier is a backbone for a gateway when the gateway can get four things
from it. Each is stated with what its absence costs, because a backbone that
provides three of the four fails in a way the device cannot see.

1. **Gateway-to-gateway presence.** A gateway MUST be able to ask its peer
   gateways whether a recipient is attached to them, and MUST answer the
   same question when asked. This is the Presence verb reused between
   gateways, and it is what turns "not attached here" into a verdict rather
   than a guess: without it every recipient outside the local zone is
   unreachable and the backbone carries nothing.
2. **Arbitrary-size framing.** A frame accepted from a device MUST reach the
   peer gateway whole, up to the largest line the daemon link accepts (the
   daemon's own line bound; the reference clients' is 1 MiB, per
   [Framing](#framing)). How the backbone segments, sequences and
   checks it is the backbone's business. A gateway MUST NOT ask a device to
   size its frames to the backbone, because the device does not know what
   the backbone is.
3. **A provisioned peer list.** The gateways a gateway asks and forwards to
   are configuration, installed with it
   ([ADR 0016](../adr/0016-gateways-are-provisioned-not-emergent.md)). A
   backbone MAY offer discovery of its own, and a gateway MAY layer it on
   later without changing the query shape; nothing here depends on it.
4. **A declared rate class.** Every backbone kind has one, and the gateway
   applies it at its own egress. Two classes exist:

   | Class | Links | What egresses |
   |-------|-------|---------------|
   | `scarce` | A few bps up to LoRa-class kbps | Direct messages, acknowledgements and control frames only. Media MUST be excluded. |
   | `broad` | Broadband | Whatever the daemon link accepts, under the gateway's budgets. |

   The class is a property of the kind, given in the [table of
   kinds](#the-backbone-token); a kind not in that table is `scarce`. The
   exclusion is the gateway's to enforce. The device side only prefers
   against it: media pins to whichever carrier is current and the daemon
   carrier is absent from the fallback order, so a device whose current
   carrier is the daemon link does hand it media. A scarce gateway then
   refuses each chunk with a plain-failure verdict (`budget_exceeded` or
   `frame_too_large`, never `recipient_unreachable`) and the pinned
   transfer fails on its retry ladder; the device does not know the class
   (invariant 3), and that cost is the price of not knowing.

### The backbone token

A gateway advertises each backbone it can reach as one capability token of
the family

```
backbone_<kind>_v1
```

where `<kind>` is a lowercase name; the kinds this revision defines are in
the table below.

- A gateway MAY advertise zero backbone tokens. A gateway with no backbone is
  a conforming gateway: it attaches devices, answers verdicts for the
  recipients it can see, and reports the rest unreachable. The zone gets mesh
  fallback for everyone beyond it, which is invariant 3 doing its job.
- A client MUST ignore a kind it does not recognise and MUST keep the session.
  An unknown kind is a gateway newer than the client, not a broken one.
- The token is **advisory, never a routing input.** A device MUST NOT choose
  a carrier, park a message or size a frame on the strength of a backbone
  token. What a device may do with one is show it, log it and hand it to the
  application. The reference implementation stores the set and reads no
  token from it, and
  [ADR 0026](../adr/0026-the-backbone-is-a-gateway-property.md) records why
  that is a decision and not an omission: a token is set by whoever holds
  the socket, so a routing choice that trusts it is a routing choice the
  gateway makes for the device.
- One kind per token. A gateway with two backbones of different kinds
  advertises two tokens.

| Kind | Rate class | Mechanism |
|------|------------|-----------|
| `reticulum` | `scarce` | [The Reticulum backbone](#the-reticulum-backbone) |

`backbone_reticulum_v1` is the first member of the family and the only one
this revision defines. It was the token before the family was named, so a
gateway or client built to the earlier text agrees with this one.

### One attached daemon at a time

A device attaches to one gateway daemon at a time. Every gateway answer is
recorded against the carrier that gave it, and the daemon link is one carrier
however many backbones stand behind it. A second daemon on the same device
would therefore overwrite the first one's verdicts and presence answers with
its own, recipient by recipient, with neither wrong on its own terms. Two
daemons need two carriers, which is a change to the transport set and is
deferred, with that reason, in ADR 0026. A device that wants a second
gateway's reach gets it the way the zone does: the two gateways are peers
over the backbone, and the one daemon answers for both.

### Gateway-owned destinations

The invariant, independent of the backbone kind: **a gateway is reachable on
the backbone under its own identity, and only under its own identity.** A
device's key never leaves the device, so no gateway can sign for a device,
and no gateway is handed a device key so that it could. It follows that a
device is reachable across the backbone only through the gateway it is
attached to, and that a device moving between zones moves nothing on the
backbone: the new gateway starts answering presence for it and the old one
stops.

Recorded alternative, rejected for v1: device-signed announce blobs, where a
device pre-signs its own backbone announce and gateways republish it. It buys
true device mobility across zones at the cost of a new device-side crypto
surface, lifetime coupling between the `off1` identity and a backbone
identity, and pressure on the backbone's announce budget.

### Re-origination into the zone

A frame arriving from the backbone is re-originated into the zone as an
**ordinary frame from the gateway**. It MUST NOT be a relay-hint frame, and it
inherits no zone-internal signature exemption. Hint frames are self-addressed
frames the local bridge intercepts and replaces, with acknowledgement disabled
and transport pinned ([ADR 0015](../adr/0015-relay-hint-frames-unacked-and-pinned.md));
a re-originated frame is the opposite disposition in both respects and needs no
amendment to that decision.

### The Reticulum backbone

The reference backbone, and the one the bundled managers were written against,
is [Reticulum](https://reticulum.network/). Wide-area routing over
intermittent, slow, heterogeneous links is the problem Reticulum spent a decade
solving, and this protocol does not reinvent it. This subsection is how
Reticulum meets the four obligations; nothing in it binds a gateway on another
backbone.

- **Presence** is the Presence verb carried gateway-to-gateway over a
  Reticulum link.
- **Framing.** The Reticulum MDU is 465 bytes and this protocol's frames
  routinely exceed it, so gateway-to-gateway transfer uses links with
  resources, which handle sequencing, compression and integrity for
  arbitrary sizes, and gateways keep long-lived links to their provisioned
  peers.
- **Peers** are provisioned destination hashes. Reticulum's own announces
  MAY inform the list later without changing the query shape.
- **Rate class** `scarce`: Reticulum links range from LoRa-class kbps down
  to a few bps.

Destinations follow from the invariant above and from how Reticulum signs
them. A Reticulum announce is signed by the destination's own identity key, so
a per-device destination would need either a Reticulum stack on every device
(there is none) or gateways holding device private keys (which the invariant
forbids). Each gateway therefore announces **one** destination under its own
Reticulum identity. An egress frame is wrapped: the outer layer is a link to
the destination gateway, the inner layer is the ordinary Offline Protocol wire
message naming the `off1` recipient, and the gateway sees ciphertext and
routing metadata only. A gateway locates a recipient's gateway from its own
attach sessions and zone presence, and by asking its peer gateways.

## Provisioning

A gateway is a **deployment**, never a runtime state a phone reaches: a powered,
stationary box someone installs and configures, attached to at least one zone
and at least one wide-area carrier. The reasoning is in
[ADR 0016](../adr/0016-gateways-are-provisioned-not-emergent.md).

A zone MAY have any number of gateways and a device treats them as a set. A zone
with **zero** gateways is a state the policy MUST be able to surface rather than
hide: absent facts mean today's behaviour (invariant 3), which is exactly what a
zone with no gateway should experience.

Operational guidance for anyone deploying: install two. One gateway per zone is
a chokepoint, and nothing in this contract prevents a second.

## Abuse and exhaustion

A gateway MUST bound what one attached device can consume: token-bucket budgets
per attached device and per peer gateway, the pattern the zone's own forwarding
governor already applies per peer. The budgets are sized to the backbone's
declared rate class, and combined with the class's exclusions they cap what a
single device can do to a multi-bps backbone link.

The threats a gateway introduces (a fake gateway, verdict abuse, zone metadata
at the operator, backbone exhaustion) are enumerated in the
[threat model](../security/threat-model.md).
