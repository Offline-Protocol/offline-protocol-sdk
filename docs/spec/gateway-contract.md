# The gateway contract

A **gateway** bridges a zone (a governed BLE / Wi-Fi Direct flood) to somewhere
a zone cannot reach: the internet, or a wide-area Reticulum backbone. This
document specifies what a gateway must do to be one, and the wire protocol a
device speaks to a gateway daemon over local IP.

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

- A Reticulum gateway MUST implement Verdict, or it is not a gateway. This is
  what closes the Reticulum half of the mixed-neighbourhood residual.
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

This promotes the protocol both mobile bridges already speak to a configurable
`daemonAddress` (default `localhost:4242`), rather than designing a fresh one:
the client side is already implemented twice, and no daemon exists anywhere yet,
so extending it breaks nobody.

### Framing

Newline-delimited UTF-8 JSON objects, one per line. Each object MUST carry a
`type` field. A receiver MUST ignore an object whose `type` it does not
recognise, and MUST NOT close the connection because of one: that is what allows
a version to add messages without a flag day.

### Versioning

`Identify` carries `protocol_version`, an integer, `1` for this document. The
daemon answers with its own. A daemon that does not recognise the client's
version MUST still answer, so the client can decide; a client that does not
recognise the daemon's version SHOULD proceed and rely on unknown-type tolerance
above.

### Attach

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
other domain, live or reserved (see
[Signing domains](username-discovery.md#signing-domains)).

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
→ {"type":"SendMessage","recipient":"<off1…>","content":"<base64>","encoding":"base64","reply_to_msg":"<id>"}
← {"type":"MessageSent","message_id":"<id>"}
  or
← {"type":"DeliveryError","message_id":"<id>","reason":"recipient_unreachable: <text>"}
```

A gateway MUST answer every `SendMessage` with exactly one of these. Silence is
not permitted: the sender's outbox holds the message either way, but a gateway
that neither forwards nor refuses converts a routing decision into a timeout.

`reason` MUST begin with `recipient_unreachable` when the recipient is not
reachable through this gateway. That token is the one the SDK classifier matches
by prefix, and it drives parking, the mesh offer and the escalating probe. Any
trailing prose after the token is for human logs and MUST NOT be relied on: the
SDK discards it at the classification boundary and never carries it into an
event.

### Deliver

```
← {"type":"MessageReceived","sender":"<off1…>","content":"<base64>","encoding":"base64"}
```

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

### Status

```
← {"type":"StatusUpdate","status":"connected"|"degraded"|"disconnected"}
```

Advisory. A device MUST NOT treat `StatusUpdate` as a delivery signal for any
particular frame.

### No new zone control prefixes

The daemon protocol is its own wire between a device and its gateway. It
introduces **no** new reserved control-message prefixes, and therefore adds no
entries to the [reserved prefix registry](control-messages.md#reserved-prefix-registry).
A gateway that wants to speak to the zone speaks ordinary frames.

## The backbone

Between gateways, the wide-area carrier is Reticulum. Wide-area routing over
intermittent, slow, heterogeneous links is the problem Reticulum spent a decade
solving, and this protocol does not reinvent it.

### Gateway-owned destinations

Reticulum announces are signed by the destination's own identity key.
Per-device backbone destinations would therefore require either phones running a
Reticulum stack (they do not) or gateways holding device private keys
(unacceptable). So:

- Each gateway announces **one** destination under its own Reticulum identity.
- An egress frame is wrapped: the outer layer is a link to the destination
  gateway, the inner layer is the ordinary Offline Protocol wire message naming
  the `off1` recipient. The gateway sees ciphertext and routing metadata only.
- A gateway locates a recipient's gateway from its own attach sessions and zone
  presence, and by asking its peer gateways (the Presence verb, reused
  gateway-to-gateway). The peer list is provisioned configuration; announce-based
  discovery MAY layer on later without changing the query shape.

Recorded alternative, rejected for v1: device-signed announce blobs, where a
phone pre-signs its own announce and gateways republish it. It buys true device
mobility across zones at the cost of a new device-side crypto surface, lifetime
coupling between the `off1` identity and a Reticulum identity, and pressure on
the announce bandwidth budget.

### Framing and the scarce path

The Reticulum MDU is 465 bytes and this protocol's frames routinely exceed it.
Gateway-to-gateway transfer therefore uses links with resources, which handle
sequencing, compression and integrity for arbitrary sizes, and gateways keep
long-lived links to provisioned peers.

Backbone links range from LoRa-class kbps down to a few bps, so **backbone
egress is a scarce path by default**: direct messages, acknowledgements and
control frames only. Media MUST be excluded unless a capability says otherwise.

### Re-origination into the zone

A frame arriving from the backbone is re-originated into the zone as an
**ordinary frame from the gateway**. It MUST NOT be a relay-hint frame, and it
inherits no zone-internal signature exemption. Hint frames are self-addressed
frames the local bridge intercepts and replaces, with acknowledgement disabled
and transport pinned ([ADR 0015](../adr/0015-relay-hint-frames-unacked-and-pinned.md));
a re-originated frame is the opposite disposition in both respects and needs no
amendment to that decision.

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
governor already applies per peer. Combined with the scarce-path rule, this caps
what a single device can do to a multi-bps backbone link.

The threats a gateway introduces (a fake gateway, verdict abuse, zone metadata
at the operator, backbone exhaustion) are enumerated in the
[threat model](../security/threat-model.md).
