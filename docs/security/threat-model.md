# Threat model and trust boundaries

This document states what the Offline Protocol defends, against whom, and what
it does not defend. It is the reference for judging whether a proposed change
weakens a security property.

For vulnerability reporting and safe harbor, see [SECURITY.md](../../SECURITY.md).

## Assets

Ranked by what an attacker gains from compromising them.

| Asset | Where it lives | Loss means |
|-------|----------------|------------|
| Identity private key | Platform secure storage | Total impersonation of the identity, retroactive and prospective |
| MLS group secrets | Platform secure storage, via the MLS provider | Reading all traffic in that group for as long as the epoch stands |
| Message plaintext | In memory, and in the outbox until acknowledged | Disclosure of conversation content |
| Cloud media keys | Inside the sealed rich payload | Decryption of media stored on a third-party host |
| Social graph | Inferable from mesh frames and relay routing | Who talks to whom, when, and from where |
| Delivery metadata | Acknowledgements, presence, receipts | Liveness and behaviour patterns of a target |

The social graph is deliberately listed as an asset. In an offline-first mesh
the traffic pattern is visible to every device in radio range, and the protocol
reduces but does not eliminate what that reveals.

## Adversary classes

| Class | Position | Capabilities assumed |
|-------|----------|---------------------|
| **A1. Passive radio observer** | In range of a mesh transport | Reads every frame sent nearby |
| **A2. Active network attacker** | On any path, including the pre-session bootstrap | Injects, drops, reorders, replays, and re-addresses frames |
| **A3. Hostile relay** | Operates or has compromised the internet relay | Everything A2 has, plus chooses delivery order, plus originates relay answers |
| **A4. Group insider** | Holds a legitimate leaf in a group | Everything a member can do, plus can craft frames as a member |
| **A5. Malicious application** | Runs above the SDK on the same device | Full access to the SDK's public API |
| **A6. Device compromise** | Root or equivalent on the device | Everything |

A6 is out of scope. The protocol assumes platform secure storage holds. A5 is
partly in scope: the SDK refuses control-frame injection through public send
surfaces and refuses to hand back attacker-chosen identifiers, but it does not
defend an application against itself.

## Trust boundaries

```
┌─────────────────────────────────────────────────────────────┐
│ Application process                                         │
│                                                             │
│  ┌───────────────────────────────────────────────────────┐  │
│  │ Application code            (A5)                      │  │
│  └───────────────────────────────────────────────────────┘  │
│      │ boundary 1: public API + reserved prefix refusal     │
│  ┌───────────────────────────────────────────────────────┐  │
│  │ Language binding / bridge                             │  │
│  └───────────────────────────────────────────────────────┘  │
│      │ boundary 2: FFI contract, event JSON, entry points   │
│  ┌───────────────────────────────────────────────────────┐  │
│  │ Protocol core (Rust)                                  │  │
│  │   ┌─────────────────────────────────────────────────┐ │  │
│  │   │ MLS state, identity key                         │ │  │
│  │   └─────────────────────────────────────────────────┘ │  │
│  │      │ boundary 3: platform secure storage           │  │
│  └───────────────────────────────────────────────────────┘  │
└─────────────────────────────────────────────────────────────┘
       │ boundary 4: control-plane signature gate
┌──────┴──────────────────┐   ┌──────────────────────────────┐
│ Mesh peer         (A1,A2)│   │ Internet relay        (A3)   │
└─────────────────────────┘   └──────────────────────────────┘
       │ boundary 5: MLS AEAD (end to end, survives both)
```

### Boundary 1: application to SDK

**Enforced by:** reserved prefix refusal on every public send surface; size
caps at the API boundary rather than deeper; refusal to accept a transport peer
identity from the caller.

**Assumption:** the application is not the adversary, but is not trusted to be
careful.

### Boundary 2: bridge to core

**Enforced by:** a fixed FFI surface, opaque event payloads, and, for anything
security-relevant, a **dedicated entry point** rather than message-plane
injection.

The group delivery report is the worked example. It arrives through its own
entry point, which makes it unforgeable by anything that can reach the generic
notification injector. Anything with equivalent authority MUST follow it.

See [Bridge contracts](../bridges/README.md).

### Boundary 3: process to secure storage

**Enforced by:** the platform (Keychain, Keystore, equivalent), reached through
a storage interface the application implements.

**Assumption:** the platform store is confidential and integral. Sealing is not
integrity: an implementation that seals records but permits deletion of a state
key must consider what deleting that key reopens.

### Boundary 4: this device to any peer or relay

**Enforced by:** the control-plane signature gate. See
[Control messages](../spec/control-messages.md#the-control-plane-signature-gate).

The essential property is that verification derives an address from the
presented public key and compares it to the claimed sender. Without that step,
signature verification proves nothing about identity, and the protocol would
need a trust-on-first-use store, which is what this replaced.

Two exemption classes cross this boundary and are documented in full in the spec.
Both are stated here because they are the largest deliberate holes in the
design:

| Exemption | What it means |
|-----------|---------------|
| Data plane (`__MLS_ENC__`, `__GROUP_MSG__`) | Authenticated **later**, by MLS decryption plus the credential comparison |
| Relay answers (six prefixes) | **Not authenticated by this protocol at all** |

### Boundary 5: end to end

**Enforced by:** MLS (RFC 9420), plus the application-side leaf identity binding
that RFC 9420 assigns to the application and that MLS implementations do not
perform.

This is the only boundary that survives a hostile relay.

## Controls, and the attack each answers

| Control | Answers |
|---------|---------|
| Self-certifying addresses | Impersonation by name claim (A2, A3) |
| Canonical signing payload with domain separation and length prefixes | Signature replay across purposes, delimiter-shift forgery (A2) |
| Address derivation from the presented key | Signature-by-any-key forgery (A2, A3) |
| Envelope slot binding | One derivable session identifier aimed at arbitrary peers (A2) |
| Leaf identity binding, three seams | A member speaking as another member (A4) |
| Wire-sender to credential comparison | Relay re-attribution of group messages (A3) |
| Plaintext-naming-an-MLS-group drop | Downgrade of a secured group to plaintext (A2, A3) |
| Sealed rich payload | Relay-visible reply previews, media keys, forward attribution (A3) |
| Withholding acknowledgements on refusal | Liveness confirmation to an injector (A2) |
| Report rate limits | Report flooding by an insider (A4) |
| Bounded tracking maps | Memory growth driven by attacker-chosen keys (A2) |
| Telemetry classification to fixed vocabularies | Third-party identifier disclosure through unscrubbed event text (A2, A3) |

## The acknowledgement channel is a side channel

An acknowledgement confirms to whoever sent a frame that the target is live and
processing. That makes the acknowledgement decision a security decision, not
only a reliability one.

The rule the protocol settled on:

- A frame that **cannot become deliverable** and was refused on security grounds
  gets **no acknowledgement**, and its identifier is unmarked.
- A frame that is **signature-gated** may be acknowledged even when refused,
  because the acknowledgement tells an unauthenticated injector nothing and a
  permanent refusal should not be retransmitted.
- A frame that failed for a **recoverable** reason gets no acknowledgement, so
  the sender's resend is the recovery path.

Consequences for application teams are in
[Delivery and ACKs](../state-machines/delivery-and-acks.md). The headline: **a
missing acknowledgement is not proof of non-delivery.**

## Known residual risks

Stated plainly, because a threat model that lists only what it defeats is
marketing.

### R1. Relay answer forgery

Anything able to inject on the relay ingest path can forge the six relay-answer
prefixes: group registration, membership add and remove, group info, the user's
group list, and error reports.

**Impact:** a forged registration flips the sync gate that group broadcast rides
on; forged membership answers corrupt the members cache (though **not** the MLS
roster, and roster-derived logic never reads that cache).

**Why it stands:** these frames have no signer. Closing it means moving relay
answers onto dedicated entry points.

**Mitigations in place:** the bridge restricts these prefixes to the relay
channel, and the exemption applies only to internet-transport frames carrying no
carrier identity.

### R2. Unauthenticated session desync trigger

Anyone who can inject a frame can drive a 1:1 session to a desync
classification, with no key material, no captured ciphertext, no session, and no
replay.

**Why it stands, and why it cannot be fixed at this layer:** the encrypted
prefix is data-plane and deliberately signature-exempt; MLS validates the
framing header (group identifier, then epoch) **before** any AEAD, sender-data,
or signature check; and a 1:1 slot identifier is a public function of two public
addresses. The MLS credential that would authenticate the sender exists only
once decryption **succeeds**.

**Why it is tolerable:** the mitigation is that acting on the trigger is
**harmless**, not that the trigger is trusted.

- Slot binding means one derivable identifier cannot be aimed at arbitrary
  peers, and the tracking map cannot be grown with attacker-chosen keys.
- A per-peer rate limit bounds the churn.
- The heal destroys nothing: a session reset keeps the outbound pending queue,
  which holds plaintext and is sealed against the rebuilt session at flush time.
- Every re-key emits a security warning, so a sustained rate is visible.

**Residual:** bounded re-key churn on a pair. Delivery delayed, never lost.

**What would close it:** a signed epoch-corroboration exchange before teardown.
A liveness-only probe does **not** work, because a healthy peer answers and the
teardown happens anyway.

### R3. Address collision margin

The 160-bit address hash gives ~2^80 collision resistance. An attacker who finds
a collision holds two signing keys indistinguishable at the address layer, which
is enough to equivocate and enough to defeat the one-identity-one-leaf property
the group binding otherwise inherits from MLS signature-key uniqueness.

Second-preimage resistance, which is what impersonation of an existing peer
requires, remains ~2^160.

**Why it stands:** every mesh frame carries two addresses and the Bluetooth LE
budget is the binding constraint. Widening is a version bump and a migration.

### R4. Divergent administrative views

Opt-in commit enforcement detects an **absent** administrative view, never a
**divergent** one. Two honest members with different role snapshots can reject
each other's commits and partition.

**Why it stands:** the administrative overlay replicates best-effort by design,
and rejecting a commit forks you permanently from everyone who accepted it.

**Mitigation:** enforcement is opt-in and documented as unsuitable for
fleet-wide enablement.

### R5. Unauthenticated plaintext in MLS-free groups

A group with no MLS state accepts unauthenticated plaintext on the relay group
fan-out prefix.

**Why it stands:** identical to pre-gate behaviour, and unreachable in
deployments where every group is MLS-secured.

### R6. Broadcast tracker is memory-only

A process kill inside the delivery-report window loses the re-issue backstop for
that broadcast.

**Impact:** members the relay missed are not re-issued to. The sender's outbox
does not cover it, because the broadcast was one frame.

### R7. Capability negotiation is unsigned

The three capability lists are not bound to the sender's signature. Forging one
onto a legacy peer is a targeted delivery denial of service. It grants nothing
else, and an attacker in that position can already drop packets.

`nostr_pubkey` is the exception and is honoured only from a signed key package,
because it is consumed as a destination key rather than a feature hint.

### R8. Fan-out timing at large group sizes

Past roughly 118 members, per-member fan-out's tail exceeds the acknowledgement
timeout and frames are retransmitted before they were ever written. Duplicates
are absorbed by deduplication, so this is wasted work rather than loss, but it
is a real scaling cliff.

## The telemetry producer rule

Telemetry ships some string fields verbatim by design. The scrubber hashes
identifiers it knows about, but it cannot know that a free-text field contains
one.

The burden therefore sits entirely on producers, and the rule is absolute:

> **An event field never carries text chosen by a remote party, nor a rendered
> error that interpolates one.**

Not shortened, not sanitized in place. **Classified**, to a fixed local
vocabulary, with the remote wording kept bounded in a device log if it is worth
keeping at all.

Two habits follow, and both are structural rather than advisory:

1. **Return a static string type from the classifier**, so interpolating wire
   input is unrepresentable rather than merely discouraged. Push that type all
   the way into the event constructor, so a producer cannot hand it a rendering.
2. **Make the classifier's match exhaustive in the crate that defines the error
   type.** A newly added variant then fails to compile *there*, forcing the
   privacy decision to be made where variants are written.

**Never add a catch-all arm to a classifier.** A catch-all restores the per-site
opt-in that the exhaustive match replaced, and the leaks this rule exists to
stop were all per-site omissions.

When the dropped prose carried real structure, add it back as a **typed field**
the scrubber can hash, not as prose.

### Why this became a rule rather than a set of fixes

The first fix scoped the substitution to the identity refusals, on the premise
that every other join failure was a fault rather than an accusation and named
nobody. That premise was false. Sibling arms at the same sites rendered a
session slot, which is two addresses, one of them possibly a third party's, plus
a sender-chosen string bounded by neither charset validation nor a length cap.

A wider audit then found the same class in relay-answer-fed error reasons, in
control-gate warnings, and in transport send-failure text. The rule generalized
because the exceptions kept turning out not to be exceptions.

## Non-goals

- **Anonymity.** Sender and recipient addresses are visible on every mesh frame.
  The protocol protects content, not the fact of communication.
- **Traffic analysis resistance.** No padding, no cover traffic, no timing
  defence.
- **Defending an application against itself.** See A5.
- **Surviving device compromise.** See A6.
- **Byzantine group consensus.** Membership is MLS's, applied by every member.
  The protocol reports unauthorized changes; it does not achieve agreement on
  who may make them.
