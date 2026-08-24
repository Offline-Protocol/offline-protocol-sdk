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

**Either end may send the first key package.** The order above is the one an
out-of-band anchor produces, and a phone that meets a device over the radio
sends its own package first instead, unprompted, on discovering the neighbour.
A leaf MUST accept that order: it answers with a key package of its own, and
the exchange continues from step 3. Refusing it does not make a device safer,
it makes a phone-initiated pairing impossible, because the frame refused is the
only thing that would have told the device the phone exists.

That first frame is signed under the **older control payload**, whatever
release its sender runs. A sender picks the payload from what the recipient has
advertised in `ctrl_versions`, and `ctrl_versions` travels in a key package, so
a peer never met has advertised nothing yet
([ADR 0023](../adr/0023-a-control-frame-states-when-it-was-made.md)). A leaf
MUST therefore accept `__MLS_KEY_PKG__` under the older payload, MUST ignore
that frame's `session_reset`, and MUST NOT judge its age, since the payload
leaves the timestamp outside the signature. Every other control frame a leaf
accepts MUST be refused under it, and none of them needs the exception: a
Welcome and a confirmation probe can only follow this device's own key package
reaching the peer, which is the frame that teaches the peer to sign the
freshness-bound payload.

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
- **answers** a frame it accepted with a delivery acknowledgement, which is not
  a control frame and carries no prefix (see
  [What a leaf owes for a frame it received](#what-a-leaf-owes-for-a-frame-it-received));
- **never emits** a Welcome, a commit, a proposal, or any group, rich, document
  or relay frame.

Every frame it accepts is one **addressed to it**. A leaf MUST establish that
before it acts on anything, whatever prefix the frame carries and however well
the frame verifies, and nothing further down asks the question again: a control
frame's signature covers the recipient, so one honestly signed for somebody
else verifies perfectly, and a sealed frame carries no signature at all, so its
recipient is whatever the last hand to touch it wrote there. Being able to open
a frame is a different claim from having been sent it.

What the check saves is not a message opened by the wrong node, since the group
and credential gates hold either way. It is an overheard key package admitting
a peer, spending flash, minting a private init key nobody asked this device
for, and answering a node that never addressed it; a sealed frame the device
can open being acted on after anyone who captured it rewrote the recipient; and
every other prefix arriving as an identity-binding failure, so ordinary traffic
between two neighbours reaches firmware wearing the shape of an attack on a
device whose only account of itself is that error stream.

A frame addressed elsewhere is **ignored rather than refused**. Overhearing is
what a shared radio does, and firmware that carries frames for its neighbours
needs "not mine" to be a fact it can act on rather than a failure it has to
interpret.

A leaf MUST join only on a Welcome that spends **the key package it minted for
the peer that sent it**, and MUST establish that before joining, because
joining spends the init key.

A key package is a bearer token. It travels in a frame that is signed and
addressed but not encrypted, so a shared radio hands a copy to everyone in
range, and a copy is exactly as spendable as the original. Every other gate on
the Welcome that a listener builds around a copied package passes honestly:
the listener does hold the key its own address derives from, it does name
itself as inviter, and the group it built really is the one this pair's id
names. A leaf that cannot tell the two apart therefore has no gate at all here,
only checks that a copier satisfies for free.

What the check saves is not confidentiality, since the joined group is one this
device holds keys for either way. It is the **init key**, which is single use:
spent by a listener, it leaves the peer it was minted for holding a Welcome
that can no longer be joined, and a pairing that only a driven reset recovers.
Refusing before the join leaves that package unspent. Refusing after would not.

Recording which package went to which peer is what makes the difference
checkable, and it is why a leaf MUST NOT treat "this peer was once given a
package" as the test: a listener that has also paired holds a package of its
own, and a Welcome spending somebody else's is the case that costs the most.

A leaf MUST establish that its group is still a pair **every time it uses
that group**: before it seals, before it opens, before it answers a probe, and
again on each commit once that commit has applied. The Welcome gate keeps a
device out of a room it never chose, and it runs exactly once. A commit changes
the roster without changing the group id, and in this profile every commit is
the peer's to make, so a leaf that checked only at the join follows its peer
into a room one member at a time and never sees it happen. The roster is the
only thing that says otherwise, and the member addresses in it MUST be derived
rather than read, a basic credential being a bare assertion.

Checking only where the roster changes is not enough, and the reason is that
the commit is the one moment whose answer cannot be kept. It is applied and
durable by the time there is a roster to read, so the refusal is a returned
value and nothing more: it survives no reboot, and a device that came back up
would seal its next message into the widened room and call it an ordinary
session. Reading the roster where the group is used puts the answer somewhere
that cannot be lost. It costs a roster read and two derivations per frame, both
of them symmetric work, so the profile's per-message cost is unchanged.

Such a commit is **reported rather than rolled back**. A member cannot skip one
commit and keep decrypting the next, so by the time there is a roster to read
the commit is applied and durable. What the refusal buys is that firmware hears
the pair stopped being a pair, on a device where the alternative is not
noticing.

A leaf MUST answer a probe only while it holds a session with that peer **that
it can still load and that is still this pair**, which is the rule a phone
already applies to the same frame. The acknowledgement is not a liveness signal: a peer confirms its
session on receiving one and then flushes everything it had queued into that
session. A device that answered after losing its store would confirm a session
it cannot decrypt one frame of, and the silence afterwards is
indistinguishable from a quiet link, so the peer never learns. Staying quiet
leaves it unconfirmed, which is a state it has a path out of. Stored bytes are
not the test: state that is present and unloadable decrypts exactly as much as
state that is absent, so a leaf MUST NOT answer on the strength of a record it
has not opened.

A leaf MUST NOT treat an inbound `__MLS_CONFIRM_ACK__` as evidence of a
session, which is why the frame appears above under what a leaf emits and not
under what it accepts. A leaf emits acknowledgements and never probes, so it
never has one outstanding and every inbound one is unsolicited. Acting on one
would let any holder of a keypair assert that a session exists: the frame is
signed, and a signature that derives to its own address costs nothing to
produce. The frame is answered by a peer that probed, and a leaf is not one.

Per-commit cost on the device is two elliptic-curve operations. Per-message
cost is symmetric only.

### What a leaf owes for a frame it received

A leaf MUST answer a frame it accepted from a peer it holds a record for with a
**delivery acknowledgement**, when the frame asked for one.

This is a different frame from `__MLS_CONFIRM_ACK__` above and means a different
thing. That one is sealed, answers a probe, and says *this pair still shares a
session*. This one is a plain message: no prefix, no signature, empty content,
and a single metadata entry naming the id of the frame it answers. It says only
*the frame with this id reached the node it was addressed to*, and it MUST NOT
be read as evidence of anything else.

**Why a device answers at all**, on a link chosen for how little it costs to
keep quiet: because staying quiet costs more. A peer marks its frames as needing
an acknowledgement and settles them against the answer. Against a device that
never answered, every frame ran the full retry ladder, which is ten
retransmissions of a sealed frame over about thirteen minutes. Each one arrived
as a replay of a generation the ratchet had already spent, so the device refused
it correctly and firmware saw a run of decrypt failures indistinguishable from
somebody replaying frames at it deliberately. The one signal that would tell an
integrator they are under attack was buried under traffic the protocol generated
itself. The answer is empty content and one metadata entry; what it prevents is
ten full sealed envelopes. It is also the only way the peer's application ever
learns that the command it sent to a lock arrived.

**Only a peer it already knows.** A record exists for a peer that got through
the key package gate, which means a verified signature over a freshness-bound
payload and an address that derives to the key that made it. A leaf MUST NOT
answer anyone else. Answering would hand a stranger within radio range two
things a device should not give away: a way to make it transmit on demand, and
a reply to "is there a node at this address", which against a lock is the first
question worth asking. Nothing is lost, because the peer whose frames are worth
acknowledging is the one the device is paired with.

**Only what it accepted.** A frame that is refused MUST NOT be answered. An
acknowledgement is a receipt, and handing one to whoever just failed the
signature gate tells them their frames are being processed. This is the rule a
phone already applies when it withholds an answer from a frame its own security
gate rejected.

**And only a frame that proved who sent it**, which is a narrower rule than
"not refused" and is the one that must be implemented. A leaf ignores rather
than refuses a frame carrying no prefix it acts on, and an unsolicited
`__MLS_CONFIRM_ACK__` likewise: neither is a failure, and neither verifies
anything, because there is nothing in such a frame to verify. A leaf MUST NOT
answer either. The record above proves an address has paired at some point, and
a frame's sender is a plaintext field, so on its own the record admits anyone
who overheard the pair once: the two gates are one rule, and only a frame whose
control signature verified, or which opened under the pair's group key, has
earned an answer. Answering the rest would hand back the transmit-on-demand and
the presence oracle the known-peer rule was written to withhold, and would let
frames costing nothing to forge evict the ids a device really does owe answers
for.

**The record MUST NOT decide the frame.** If the id cannot be stored, the
answer MUST be withheld and the frame's own result MUST still be reported. By
then the frame is open and the ratchet has spent that generation, so discarding
its result to report the storage failure would lose a command the device
carried out; what is lost instead is the answer, and the peer's retry ladder is
what recovers it.

**A frame that arrives twice is answered twice.** The second copy is
overwhelmingly a retransmission, because the answer is the frame most likely to
have been the one that went missing: it is last in the exchange and nothing
retries it. Opening the copy is impossible, since the ratchet spent that
generation on the first, so a leaf MUST remember the ids of the last few frames
it answered and repeat the answer without opening the frame again. A second copy
and a lost answer are not distinguishable at the receiver, and staying quiet for
the second turns a delivered frame into a failed one.

The memory is bounded, and the bound is flash rather than correctness: past it a
device has nothing to answer from and the ratchet refuses the frame as it always
did. That edge is deliberate. Absorbing *every* replay would trade one invisible
attack for another, and the point of quietening the protocol's own
retransmissions is to leave a real one visible.

The record MUST be durable before the answer is emitted, like every other state
this profile writes. A device that answered and then forgot would meet the
retransmission with silence, which is the state the whole mechanism exists to
leave.

**What the answer does not carry.** A phone puts two further entries on its own
acknowledgements, naming the hop count and the carrier the answer came back
over. A leaf writes neither, and the peer's defaults for their absence are
already right for a device: no hops, because a leaf is a direct peer, and BLE,
because that is the radio a leaf is paired over. A device does not own its
transport, so naming one would be firmware's guess crossing the wire as fact.

### Post-compromise security is the phone's to originate

Healing happens when a commit rotates the device's leaf in the ratchet tree. A
device that never commits does not originate those, so the phone's session
rekey path is the only thing that drives recovery in such a pair.

**Nothing drives it on a timer.** An engine fires that path on an epoch desync
and on an application asking for one, and the rekey interval is a floor on how
often either may happen rather than a schedule that fires anything. A pair that
never forks and whose application never asks therefore never heals, and the
window an attacker holding old key material keeps open is bounded by nothing.

So the cadence is the integrator's, and it is an obligation rather than a
setting: a deployment that wants post-compromise security has to ask for it, on
an interval it chooses. The engine cannot choose one, because a rotation costs a
teardown, a key-package exchange and a re-establish, and what that is worth to a
mains-powered lock and to a phone on a metered link are different answers that
nothing on the wire distinguishes.

Nothing about this is leaf-specific except how sharply it bites. Two installs
that simply never desync are in the same position; a leaf is where it is
guaranteed, because the device cannot originate a rotation even in principle.

A phone-driven rekey reaches the device as a `__MLS_KEY_PKG__` frame with
`session_reset` set, not as an unsolicited Welcome. A leaf MUST, on receiving
it, discard the existing session and mint a fresh key package, so that the
exchange begins again from step 2 above. A leaf that treats `session_reset` as
an ordinary key package refresh keeps a session the phone has already discarded,
and every later frame from it decrypts to nothing.

A leaf MUST NOT act on a reset frame it has already spent, and MUST NOT act on
one older than the last it acted on. A reset tears a live session down, and each
teardown a captured frame earns is a session the pair has to rebuild, so a
signature alone is not enough authority to carry one.

The rule is a **high-water mark**: a leaf records the signed timestamp of the
most recent reset it acted on for a peer, and admits a later reset only when its
stamp is strictly newer. One integer per peer, persisted, and written **before**
the teardown rather than after, because the teardown is followed by a fresh
pairing and a power cut in between would otherwise leave the frame able to break
the replacement too.

This is sound only because the stamp is inside the signature (see
[Control message freshness](control-messages.md#freshness)). On a payload that
left it outside, an attacker would rewrite it, park the mark past every future
reset and permanently deny the pair the ability to heal, which is a worse
failure than the replay. That is why a leaf ignores the `session_reset` on the
one frame it does accept under the older payload: the exception admits an
advertisement, never a teardown.

It supersedes the bounded list of recently-seen frame ids that earlier releases
of this profile described, which denied a repeat of a remembered frame and left
an older capture spendable once. It closes
[issue 403](https://github.com/Offline-Protocol/offline-protocol-sdk/issues/403)
on the device side.

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
