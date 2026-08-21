# Leaf node provisioning

## What a leaf node is

A leaf node is a constrained device that speaks this protocol as a peer: a
door lock, a sensor, a mains-powered relay. It parses frames, validates
addressing, runs MLS, and holds an end-to-end encrypted conversation with a
phone under the same guarantees a phone gets.

**A leaf is a peer, not a class of peer.** It sends and receives the frames in
[Control messages](control-messages.md), seals with the envelope in
[Encryption envelopes](encryption-envelopes.md), derives and checks addresses by
[Identity and addressing](identity.md), and advertises capabilities the way
[Capability negotiation](capability-negotiation.md) describes. There is no
second sealing path, no leaf-only prefix, and no reduced guarantee. A device
that cannot meet the obligations in this chapter is not a leaf node with fewer
properties; it is a relay that forwards ciphertext it cannot read.

The decision behind this, and the measurements that justified it, are in
[ADR 0021](../adr/0021-a-leaf-node-speaks-mls.md). The pieces both ends share
rather than implement twice are in
[ADR 0022](../adr/0022-one-sealed-layer-shared-with-the-leaf.md).

## Five obligations

These hold before any mechanism below makes sense. Three of them are invisible
in a passing build and expensive to discover on a bench. The fifth is invisible
in a working deployment as well, because a device that grants too much works
perfectly until somebody asks it to.

### 1. A static artifact carries an address and a key, never a key package

The artifact a person scans or a label carries MUST contain `{address,
pubkey}`, in the `InviteV1` form specified in
[Username discovery and invites](username-discovery.md#the-invite-payload). It
MUST NOT contain an MLS key package.

An MLS init key is single use and a sticker is not. The second person to scan a
key package printed on a label gets a collision, and the failure surfaces as an
establishment that silently does not converge rather than as a refusal. That is
the same collision [ADR 0012](../adr/0012-one-key-package-per-peer.md) removed
from the push path, where one package advertised to every peer left every peer
after the first unable to establish. A fresh key package flows over the pairing
radio instead, one per pairing.

### 2. A key package is minted from a supplied time, not a read clock

A leaf MUST obtain a wall-clock time at pairing and pass it into key package
generation. It MUST NOT rely on its MLS implementation to read one.

A bare-metal device has no clock, and an implementation that cannot read one
stamps a validity window beginning at the Unix epoch. The peer refuses that
package as expired, so a device that ships this way never pairs at all. The
time may come from the radio stack, from the commissioner, or from the pairing
exchange itself; this specification does not choose among them, and requires
only that one of them supplies it.

Key package validity is a **freshness bound, not an authentication mechanism**.
A wrong clock costs availability, never confidentiality. It still has to be
designed rather than discovered.

### 3. State is durable before a frame is emitted

A leaf MUST persist its MLS state before emitting any frame whose production
advanced that state, and MUST NOT emit the frame if the write fails.

The failure this prevents is not a delivery hiccup. A device that answers and
then loses power before its ratchet state reaches flash comes back and reuses an
AEAD nonce, which is a confidentiality failure in a protocol whose whole claim
is the AEAD boundary. Ordering the persist before the emit is the requirement.
The write SHOULD be atomic, so that a power cut during the write leaves either
the old state or the new one and never a torn record.

### 4. Entropy is real

A leaf MUST source randomness from a hardware entropy source. MLS key
generation is exactly as strong as what that source returns, and nothing else
in the protocol compensates for a weak one.

Randomness is the integrator's obligation because it is the integrator's part
that has it: an MLS implementation built for bare metal reaches a symbol the
firmware supplies. Measurement harnesses in this repository register a counter
in that slot so an image that never executes can be linked and sized. That stub
must never reach firmware, and it is documented as such where it appears.

### 5. A session is authentication, not authorization

A leaf MUST NOT treat an established session as permission to act. It MUST
decide what a peer may do from that peer's address, and the integrator MUST
control when the device accepts a new pairing at all.

Every gate in this protocol answers "is this peer the address it claims to be".
None of them answers "did the owner mean this peer". Producing a key that
derives to its own address costs nothing, so a device left open to pairing ends
up holding sessions with whoever was in range, every one of them
cryptographically impeccable. A lock that opens for any message on an
established session opens for anyone patient enough to pair with it, and every
frame in that exchange verifies.

This is the obligation a test cannot fail for you: the device works, the peer
is authenticated, the ciphertext is sound. The bound is an implementation
choice, whether that is a pairing button, a commissioning window, or an owner
list written at first pairing, and this chapter requires only that one exists.

## The pairing exchange

Pairing is the ordinary session establishment described in
[Session lifecycle](../state-machines/session-lifecycle.md), carried over
whatever radio the device and the phone share. Nothing about it is leaf
specific except where the time in step 2 comes from.

1. **Out-of-band anchor.** The person scans the device's `InviteV1` artifact,
   or the commissioner supplies the same `{address, pubkey}` pair. The verifier
   MUST run the full verification order in
   [Username discovery and invites](username-discovery.md#verification),
   ending in `derive_address(pubkey) == address`. Any failure means refuse.

2. **The device mints a key package** from a supplied timestamp and sends it as
   a signed `__MLS_KEY_PKG__` frame.

3. **The phone establishes the session**: it imports the package, creates the
   two-member group, and sends a signed `__MLS_WELCOME__`.

4. **The device joins** from the Welcome and confirms, by sealing an
   `__MLS_ENC_CONFIRM__` marker inside an ordinary `__MLS_ENC__` envelope. A
   group-aware decrypt is the only proof that converges a peer which created a
   session of its own, so a device that answers only the plaintext probe leaves
   that peer unconfirmed.

From step 4 the pair is an ordinary 1:1 session. Every later message is an
`__MLS_ENC__` envelope in both directions.

### What the anchor is, and is not

The out-of-band artifact is the whole of first-contact trust, exactly as it is
for two phones. It carries no key package, so it grants no session by itself;
what it grants is the address to compare a presented key against. Everything
after it is `derive(presented_key) == claimed_address`, unconditionally, at
every site that accepts an identity claim.

A leaf MUST apply that comparison at each of them, and MUST refuse an identifier
that does not parse as an address rather than skipping the check. Answering
"acceptable" for an unparseable identifier is the bypass, not a lenience: a
claim that cannot be checked is the one an attacker makes.

## The key package a leaf mints

Three properties are load bearing. Each is a default that produces a package
the peer refuses, or a policy this protocol asks an application to set.

| Property | Requirement | Why |
|----------|-------------|-----|
| `not_before` | MUST be backdated below the supplied time | Receivers test `not_before < now` strictly, so a package stamped with the current second is refused as not yet valid. The backdate is also the margin that absorbs clock skew between the two devices |
| Total lifetime | SHOULD be bounded to weeks rather than a year | RFC 9420 asks an application to define a maximum. A shorter window bounds how long an unused init key stays usable. This is leaf-side policy, not something the peer enforces |
| Framing | The frame body carries a **bare** key package | An implementation whose convenience API returns one wrapped in an `MLSMessage` MUST unwrap it before encoding. Both forms are legal MLS; only one is this protocol's wire |

The reference values for the backdate and the lifetime are constants in
`offline-protocol-sealed`, shared by both ends rather than restated.

## The never-committing profile

A leaf joins a two-member group and never commits to it. The phone creates the
group, adds the device, and issues every commit; the device joins, opens what
arrives, answers, and persists.

A conforming leaf:

- **emits** `__MLS_KEY_PKG__`, `__MLS_ENC__`, and `__MLS_CONFIRM_ACK__`;
- **accepts** `__MLS_KEY_PKG__` (including one that sets `session_reset`),
  `__MLS_WELCOME__`, `__MLS_ENC__` carrying either an application message or a
  commit, and `__MLS_CONFIRM_PROBE__`;
- **never emits** a Welcome, a commit, a proposal, or any group, rich, document
  or relay frame.

A leaf MUST answer a probe only while it holds a session with that peer, which
is the rule a phone already applies to the same frame. The acknowledgement is
not a liveness signal: a peer confirms its session on receiving one and then
flushes everything it had queued into that session. A device that answered
after losing its store would confirm a session it cannot decrypt one frame of,
and the silence afterwards is indistinguishable from a quiet link, so the peer
never learns. Staying quiet leaves it unconfirmed, which is a state it has a
path out of.

A leaf MUST NOT treat an inbound `__MLS_CONFIRM_ACK__` as evidence of a
session, which is why the frame appears above under what a leaf emits and not
under what it accepts. A leaf emits acknowledgements and never probes, so it
never has one outstanding and every inbound one is unsolicited. Acting on one
would let any holder of a keypair assert that a session exists: the frame is
signed, and a signature that derives to its own address costs nothing to
produce. The frame is answered by a peer that probed, and a leaf is not one.

Per-commit cost on the device is two elliptic-curve operations. Per-message
cost is symmetric only.

### Post-compromise security arrives on the phone's cadence

Healing happens when a commit rotates the device's leaf in the ratchet tree. A
device that never commits does not originate those, so the phone's session
rekey path drives recovery and its interval bounds the window.

A phone-driven rekey reaches the device as a `__MLS_KEY_PKG__` frame with
`session_reset` set, not as an unsolicited Welcome. A leaf MUST, on receiving
it, discard the existing session and mint a fresh key package, so that the
exchange begins again from step 2 above. A leaf that treats `session_reset` as
an ordinary key package refresh keeps a session the phone has already discarded,
and every later frame from it decrypts to nothing.

A leaf MUST NOT act on the same reset frame twice. Nothing in the signed payload
states freshness, so a captured reset verifies as well on its tenth delivery as
on its first, and each teardown it earns is a session the pair has to rebuild.
Remembering the frames already acted on costs one bounded list per peer and
denies the repeat. It does not close replay: a frame older than that list can
still be spent once. **Closing it needs a freshness field inside the signed
payload**, which is a change to the wire and to both ends rather than to a
device, and is an open gap rather than a decision this chapter has taken. It is
tracked as
[issue 403](https://github.com/Offline-Protocol/offline-protocol-sdk/issues/403).

Letting a device originate Update proposals would make it self-healing on its
own schedule. That is deliberately outside this version.

## Identity and key storage

A leaf holds one long-term identity keypair, generated at provisioning, with
the properties [Identity and addressing](identity.md#the-identity-key) requires
of every install: it signs control frames, it derives the address, and it is
the signature key inside the MLS credential.

**One device, one key.** A fleet provisioned with a shared identity key is one
identity on many devices, which this protocol neither prevents nor supports
([Identity and addressing](identity.md#what-an-address-does-not-tell-you)). For
devices the consequence is sharper than for installs: extracting the key from
one unit in a laboratory yields every unit's identity, and the address that
names one lock names all of them.

The key SHOULD live in the part's secure key storage rather than in general
flash. A device that stores it in the clear is a device whose identity is
recoverable by anyone who holds it.

## The provisioning-time adversary

Pairing anchors on an artifact a person handles. That admits an adversary the
phone-to-phone paths do not face in the same form: someone who controls the
device, or its label, before the owner does.

- **Sticker swap.** An attacker replaces the artifact on the box with one
  naming a key they hold. The scan then pairs the owner's phone with the
  attacker's device, and every check passes, because the check is that the key
  derives to the address on the label and it does.
- **Malicious installer.** Whoever commissions the device chooses what it
  pairs with, and may pair it with themselves first.

**Neither is a cryptographic failure and neither has a cryptographic fix.** The
anchor for a leaf is the same anchor as for an invite: the out-of-band human
context in which the artifact was obtained. A label on a device in a sealed box
from a trusted supplier carries that context; one photographed and re-printed
does not.

What the protocol does provide is that the compromise is **detectable and
bounded**. The address a device presents is stable and self-certifying, so a
substituted device has a different address, and an owner who records the
address at first pairing sees the substitution on any later re-pair. An
implementation SHOULD surface the address at pairing rather than only a
petname, because a petname is chosen by whoever made the artifact and the
address is not.

## What this chapter does not decide

The storage backend, the radio, the pairing user experience, whether a device
ever joins a group space rather than a pair, and where the time in obligation 2
comes from. Each is an integration choice, and none of them changes a frame on
the wire.
