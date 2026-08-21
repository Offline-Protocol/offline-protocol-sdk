# 0021. A leaf node speaks MLS, through a second implementation

**Status:** Accepted

## Context

[ADR 0020](0020-core-compiles-without-std.md) made `offline-protocol-core`
build for bare metal and measured it: about 95 KB of flash for the receiving
half of the protocol. It closed by naming what it had not solved.

> It does not make MLS run on a leaf node, and it should not be read as
> promising that: OpenMLS is not a `no_std` crate, and MLS key-schedule and
> ratchet-tree state does not fit alongside a vendor radio stack in 256 KB. A
> leaf node's payload cryptography is a separate decision, still open.

That leaves a device that can parse a frame, validate its addressing, and
forward it, but cannot read one word of it. Every payload in this protocol is
MLS ciphertext. A lock in that state is a relay, not an endpoint: the phone can
send it "unlock" and it can neither open the message nor produce an answer
anyone would believe.

The obvious next move is to design something smaller than MLS for the device
and teach the phone to speak both. That move costs more than it looks like it
costs, and the cost is not on the device. The engine has exactly one sealing
path today. A second one means a second envelope prefix in a registry where new
prefixes are signature-gated by default, a second wire form for the data layer
and group spaces and media to learn, a second trust story with its own
provisioning adversary, and a threat model that has to state which guarantees
apply to which peer. It also means picking what to give up, because the
realistic candidates all give up something: a static-key HPKE seal has no
forward secrecy and no replay defence, and Noise with a periodic rehandshake
bounds post-compromise recovery by the rehandshake interval.

## The measurement that changed the decision

ADR 0020's premise is true about OpenMLS and about large groups. It is not true
about the shape this problem actually has.

A phone paired with one device is a **two-member group**. The ratchet tree is
three nodes. Published state estimates for that shape are roughly 2 KB of
logical state, single-digit kilobytes persisted. The frightening numbers
attached to MLS state are all O(N) numbers, and N here is 2.

And RFC 9420 exists in `no_std` Rust. `mls-rs` carries `no_std` as a
CI-gated configuration, built for `thumbv6m-none-eabi`, which is a weaker part
than the M33 this SDK targets. Its RustCrypto provider supports
`CURVE25519_AES128`, which is
`MLS_128_DHKEMX25519_AES128GCM_SHA256_Ed25519`: the one ciphersuite this SDK
pins, in one place, and never negotiates.

So the question stopped being "what weaker thing does a leaf speak" and became
"does the same thing fit". Two harnesses answer that, and both are in the tree
because a decision resting on numbers should ship the thing that reproduces
them.

**It fits.** `tools/embedded-footprint` links a whole leaf image, protocol layer
included:

| Configuration | Flash | vs baseline |
|---|--:|--:|
| Application messages only, not shippable | 361.7 KiB | 360.6 KiB |
| **Never-committing leaf, the candidate** | **391.3 KiB** | **390.2 KiB** |
| `rfc_compliant`, X.509 included, upper bound | 403.8 KiB | 402.7 KiB |

On a 1536 KB part that is about a quarter of flash for the candidate, against a
budget that also has to hold a radio stack and an application. Roughly 111 KiB
of it is P-384 and P-256 arithmetic that nothing here uses, linked because the
provider keeps all four curves in one enum with no feature gating and filters
suites at runtime. A curve-gated provider is worth about 28% of the image and is
not needed to clear the bar.

**And it interoperates.** `tools/mls-interop` runs OpenMLS 0.7.4 against
mls-rs 0.56.0, both pinned, with this SDK's ciphersuite, this SDK's group
configuration, and this SDK's credential shape: pair, join from a Welcome with
no out-of-band tree, an application message each way, a commit from the phone,
and a message after it. No published result covered that pair before this one.

## Decision

**A leaf node speaks MLS. The protocol does not change, and the phone does not
learn a second way to seal anything.** The device runs a second RFC 9420
implementation, sized for the part, and the frames on the wire are the frames
that are there today.

The device is a **never-committing member**. The phone creates the group, adds
the device, and issues every commit; the device joins, opens what arrives,
answers, and persists. Per-commit cost on the device is two elliptic-curve
operations, tens of milliseconds on this class of part. Per-message cost is
symmetric only.

What this buys is not a smaller compromise. It is the absence of one. Forward
secrecy, post-compromise security, replay defence, sender authentication, the
three `derive(presented_key) == claimed_address` gates, and the unconditional
leaf identity binding of [ADR 0010](0010-unconditional-leaf-identity-binding.md)
all apply to a leaf exactly as they apply to a phone, because it is the same
protocol and the same checks. For this device class that is not merely adequate:
continuous ratcheting is something no shipped smart-home protocol provides.
Z-Wave S2 distributes static network-lifetime keys and its own specification
argues against ever rotating them; Matter and Aliro perform an ephemeral
exchange at establishment and then run a non-forward-secret fast path for most
sessions. No regulation requires better. The phrase "forward secrecy" does not
appear in ETSI EN 303 645 at all.

It also removes a problem rather than solving it. A device with no MLS has no
way to advertise a capability, because capability advertisement in this protocol
rides in a key package and nothing else, so every frame sent to such a device
would fall to the JSON floor forever. A device that does MLS mints a signed key
package like any other peer, and the whole negotiation layer applies unchanged.

### Two things a leaf's key package must do differently

These were found by running the two stacks against each other, not by reading
either one's documentation, and each is a default that produces a key package
the phone refuses.

**`not_before` must be backdated.** OpenMLS tests `not_before < now`, strictly,
while mls-rs's client builder writes `not_before` as exactly the timestamp it is
given. A package stamped with the current second is refused for being not yet
valid. The backdate is also the margin that absorbs clock skew between the two
devices.

**The timestamp must be supplied, not read.** This is the one with consequences
past the call site. A bare-metal leaf has no clock, and mls-rs stamps
`not_before = 0` when it cannot read one, with a source comment saying the value
exists so that tests can run. A device shipping that way emits a validity window
in 1970 and is refused as expired. **A leaf therefore needs a time source at
pairing**, from its radio stack, its commissioner, or the pairing exchange. Note
what this is and is not: key package validity is a freshness bound, not an
authentication mechanism, so a wrong clock costs availability rather than
security. It still has to be designed rather than discovered on a bench.

`tools/mls-interop` restores each default in turn and requires the phone to
reject the result. Those negative controls are the guard: each correction is a
default someone will eventually tidy back, and a harness that only proves the
corrected path works cannot tell them they broke it.

### A third default that is policy, not a correction

An earlier draft of this ADR listed a third correction, that a leaf must shorten
mls-rs's one-year key package lifetime because OpenMLS refuses any leaf node
whose total lifetime range exceeds one hour plus three months. **OpenMLS 0.7.4
does not do that.** It declares the bound
(`MAX_LEAF_NODE_LIFETIME_RANGE_SECONDS`) and the predicate that tests it
(`Lifetime::has_acceptable_range`), and nothing in the crate calls the
predicate. `KeyPackageIn::validate` checks only the `not_before < now <
not_after` window, and names its failure `InvalidLifetime`, which is what made a
year-long lifetime look like it was refused for its range when it was really
refused for its `not_before`.

The correction is kept as leaf-side policy, at 28 days: RFC 9420 asks an
application to define a maximum total lifetime, and a shorter window bounds how
long an unused init key stays usable. It is simply not something the phone makes
a leaf do, and this ADR should not have said it was.

Two things follow. The first is a harness rule: negative controls restore one
default at a time, because a control that broke all three at once was refused
for whichever reason OpenMLS checked first and could not tell a real requirement
from an imagined one. That is how this was found, on the first run after the
split. The second is a gap on the phone, recorded here and deliberately not
fixed in this ADR's change: because no cap is applied,
`MlsManager::import_key_package` admits a key package from any peer with an
arbitrarily long lifetime, where RFC 9420 asks an implementation to reject it.
It is a freshness bound rather than an authentication one, so it is not urgent,
but it is ours to enforce and no library is doing it for us.
`tools/mls-interop` step 0.3 pins the current behaviour and fails if OpenMLS
starts applying its own cap.

### What must be true of the device that is not true of a phone

**State must be durable before a frame is emitted.** A leaf that answers and
then loses power before its ratchet state reaches flash comes back and reuses an
AEAD nonce, which is a confidentiality failure, not a delivery hiccup. Ordering
the persist before the emit is the requirement; the SDK's existing storage seam
is the shape it takes.

**Randomness is the integrator's problem.** `mls-rs` reaches `getrandom`, which
has no bare-metal backend. The device wires that symbol to its vendor TRNG, and
MLS key generation is exactly as strong as what it returns. The footprint
harness registers a counter in that slot, which is sound for measuring an image
that never executes and must never be copied into firmware.

**Post-compromise security arrives on a cadence the phone sets.** Healing
happens when a commit rotates the device's leaf. A never-committing device does
not originate those, so the phone's existing session rekey path is what drives
recovery, and its interval is what bounds the window. Letting a device originate
Update proposals would make it self-healing and is deliberately not in this
decision.

**A QR code must not carry a key package.** An MLS init key is single-use and a
sticker is not, so the second person to scan one gets a collision. Pairing keeps
the shape it already has: the static artifact carries `{address, pubkey}`, and a
fresh key package flows over the pairing radio.

## Consequences

A door lock can hold a real end-to-end encrypted conversation with a phone,
under the same guarantees a phone gets, on about a quarter of an xG24's flash.

The engine keeps one sealing path. Nothing in `send.rs`, the prefix registry,
the data layer, group spaces, or the media envelope learns a new case, and the
threat model does not fork into "peers like this" and "peers like that".

The pieces the two ends share rather than implement twice (the envelope codec,
the address derivation, the canonical signing payload, the ratchet bounds) were
moved into one bare-metal-capable crate by
[ADR 0022](0022-one-sealed-layer-shared-with-the-leaf.md), which is the
plumbing this decision needs and changes nothing it decided.

The device class picks up obligations the SDK has not had to state before: a
time source at pairing, durable state before emission, and an entropy source
that is real. They are written down here because they are invisible in a passing
build and expensive on a bench.

Two risks are accepted with open eyes. mls-rs has not had a third-party security
audit and its only `no_std` crypto provider is the one its own authors label
experimental; this is a monitored dependency, not a settled one. And an interop
result covers exactly the two versions it pinned, which is why both are pinned
with `=` and why the harness is in the tree rather than in a commit message.

This ADR does not decide the storage backend, the radio, the pairing user
experience, or whether a device ever joins a group space rather than a pair.

## What would undo this

Adding a second sealing path to the engine. The moment a leaf speaks something
other than `__MLS_ENC__`, every argument above is void: the negotiation layer
stops applying, the threat model forks, and the reason this was affordable
disappears. If a future device genuinely cannot run MLS, that is a new ADR that
has to re-derive the cost on the phone side, not an extension of this one.

Re-inheriting the leaf's crypto configuration from a default is the smaller
version of the same thing. The key package corrections above are load bearing,
and the `tools/mls-interop` step 0 lines are what say so out loud, one per
default so that each can be believed on its own.
