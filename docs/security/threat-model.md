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
| **A7. Hostile gateway** | Operates or has compromised a gateway a device attached to | Everything A3 has, at a zone's bridge to the wider network: originates verdicts and presence answers, and observes attach sessions |

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

- A frame refused on **security** grounds gets **no acknowledgement**, and its
  identifier is unmarked. That covers the signature gate, inbound plaintext
  refused by encryption policy, and all four identity bindings (sender identity,
  session slot, leaf address, unsupported sender), which are intercepted
  *before* classification precisely so they cannot inherit the policy
  disposition below. The session slot binding runs before any AEAD, since every
  failure the MLS library raises below it happens before it authenticates
  anything; the credential comparisons run once decryption has succeeded.
  Acknowledging would confirm to an attacker that this device is online and
  processing, and unmarking is what stops an exact replay from reaching the
  duplicate re-acknowledgement path and leaking the same fact anyway.
- A frame refused on **policy** grounds keeps its acknowledgement **when the
  refusal happens on the arrival path**. A membership commit refused by opt-in
  enforcement is the case in point: the refusal is permanent, so a resend could
  only waste work. A frame already deferred into the group buffer that later
  resolves as a policy refusal is never acknowledged, because the arrival path
  withheld the acknowledgement already.
- A frame that failed for a **recoverable** reason gets no acknowledgement, so
  the sender's resend is the recovery path.

Both refusals are permanent, so what separates them is neither authentication
nor recoverability: a **policy** refusal is a statement about a *frame*, while a
**security** refusal is a statement about an *attacker*, and answering the second
at all is the leak. Being authenticated does not move a frame into the policy
row. A group message is signature-gated, so its wire sender is proven, and a
leaf credential that fails to bind still refuses it silently.

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
on, but only inside an armed window: the flag is set only for a frame that
arrives on the internet transport, names a group this device already tracks, and
correlates with a registration this device has outstanding. Forged membership
answers corrupt the members cache, which is **not** the MLS roster and is never
read by roster-derived logic, but is read verbatim as the group fan-out send
cache: an accepted forgery therefore makes this device address every subsequent
group ciphertext to an attacker-chosen identifier, or stop addressing a real
member. The same list feeds the sealed rich payload gate, which requires every
non-self member to be known rich-capable, so an unknown spliced identifier
closes it: reply context and forward attribution move from inside the MLS AEAD
to hop-visible cleartext and media secrets are dropped, until the next commit
refreshes the cache. That defeats a control this document lists by name, reached
by the adversary that control names. Adds are gated on internet arrival and
removes on an administrator check, which is what bounds all of this.

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
- The heal destroys nothing: the **desync re-key** reset keeps the outbound
  pending queue, which holds plaintext and is sealed against the rebuilt session
  at flush time. (The post-unblock reset is the other kind and deliberately
  drops that queue, failing each entry terminally. See
  [Session lifecycle](../state-machines/session-lifecycle.md).)
- Every re-key emits a security warning, so a sustained rate is visible.
- Each resend of an encrypted direct message is re-sealed against a live
  generation, **for entries whose re-seal provenance survives in memory**. That
  provenance is deliberately not persisted, so a restart drops it.

**Residual:** bounded re-key churn on a pair. Delivery delayed, never *silently*
lost: a fork that spans a sender restart settles as an honest failure rather
than a false delivery.

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

Opt-in commit enforcement acts only on a **present** administrative set that
positively excludes a principal; absent knowledge of that set fails open. It
cannot detect a **divergent** view: two honest members with different role
snapshots each hold a non-empty set, so they reject each other's commits and
partition.

**Why it stands:** the administrative overlay replicates best-effort by design,
and rejecting a commit forks you permanently from everyone who accepted it.

**Mitigation:** enforcement is opt-in, and is documented as suitable only for a
closed deployment that controls role distribution, never for part of a fleet:
a member with it off applies the commit a member with it on refuses.

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

### R9. Service discovery and service bodies are signed, not encrypted

The service prefix family (`__SVC_DISC_Q__`, `__SVC_DISC_R__`, `__SVC_REQ__`,
`__SVC_RESP__`, and the generic service message) is control-plane: every frame
is signature-gated, so its sender is proven, and every frame is **exempt from
the encryption requirement**. Discovery gossip and the application-supplied
request and response bodies therefore travel in cleartext.

**Impact:** A1 and A3 read service bodies and the full discovery pattern. This
is the one application-supplied payload that boundary 5 does **not** cover, so
"MLS protects application content" does not hold here.

**Why it stands:** discovery is a broadcast to peers with whom no session
necessarily exists, so there is no established group to encrypt to; encrypting
request and response bodies is not implemented.

**What application teams must do today:** treat a service body as public, and
encrypt anything sensitive above the SDK before handing it over.

### R10. Gateways are unauthenticated for delivery, and see who is in the zone

A [gateway](../spec/gateway-contract.md) bridges a zone to the internet or to a
wide-area backbone. Attaching to one binds the session to the device's address
with a domain-separated proof under `offline-gateway-addr-v1`, which stops a
gateway from attaching a session under an address the device does not control,
and stops a proof harvested by one gateway being replayed against the relay. It
does **not** make the gateway trusted for delivery, and nothing about the design
tries to.

**Impact:** an A7 gateway can do four things.

*Blackhole.* Accept frames and forward none. Costs latency and battery.

*Lie "unreachable".* Trigger parking for a recipient who is reachable. Retries
quiet down and the sender's probes stretch out, so delivery is delayed. Mesh
offers and probes continue, and the outbox still holds the message, so it
remains deliverable by every other path.

*Lie "reachable".* Attract traffic to a path that goes nowhere, bounded exactly
as the blackhole is.

*Observe.* Attach sessions and presence queries reveal which addresses are in
the zone, and when they are active, to the gateway operator. This is the same
exposure relay attach already gives the relay operator, at zone granularity.

**Why it stands:** verdicts are not authenticated and cannot be. They are claims
about a third party's connectivity, and no signature makes a claim true. The
design answers this at the policy layer instead: a verdict MAY open a path and
economise retries, and MUST NOT close one; a "reachable" claim never suppresses
the acknowledgement ladder. Delivery settles only on the recipient's end-to-end
acknowledgement or terminal outbox expiry, so no gateway answer settles
anything.

**Mitigations in place today**, carrying the internet relay as the one shipped
gateway: the settlement invariant, verdicts-never-close-a-path, MLS end to end
so a gateway sees ciphertext plus routing metadata, and the recipient-aware
decay that reverts every gateway claim to "no opinion" on a TTL (ten minutes for
a verdict, five for a presence answer) rather than letting it stand
indefinitely. These hold against a hostile relay right now, which is why an A7
gateway inherits a bounded blast radius rather than a new one.

### R11. A space member can abort the process with a crafted document blob

Replicated documents merge changes that arrive from peers, and merging means
handing bytes to a CRDT engine. The engine has open defects where a malformed
or causally impossible change panics rather than returning an error, and one of
them poisons the document's lock so the retry panics too. The mobile artifact
ships with `panic = "abort"`, so there is no unwinding to catch.

MLS does not help here, and the reason is worth being precise about:
authentication establishes who sent the bytes, and the question the engine is
about to ask is what shape they are. A peer can be exactly who they claim and
still send a blob that ends the process.

**Impact:** a member of a space can abort the application on a device it
replicates with. Not silent, not remote-code-execution, and not available to
anyone outside the space: it requires someone the user has already accepted
into a shared document, and it is attributable to them.

**Why it stands:** the defects are upstream and the engine is not ours to fix
on our own schedule. Re-implementing its decoder to predict what it will accept
would mean maintaining a second parser that has to agree with the first one
forever.

**Mitigations in place today:** blobs are judged before the engine sees them,
which refuses every shape we have been able to reproduce, whether it arrives as
a run of changes or as a whole document; frames are bounded at 32 KiB before
decoding; a space accepts at most 1024 documents on a peer's say-so, so an
offer cannot spend unbounded storage; and the digest of a blob about to be
imported is written to disk first, so a blob that does end the process is
refused when the sender retries it. That last one is what bounds the damage to a single abort
rather than a crash loop driven by the delivery ladder faithfully doing its
job. See
[ADR 0019](../adr/0019-remote-document-imports-are-contained-not-trusted.md).

This shrinks when the engine's import hardening reaches its Rust release.

**Mitigations specified but not yet implemented**, and therefore not yet
protecting anyone: address-bound attach under `offline-gateway-addr-v1` (the
domain is [reserved, not emitted](../spec/username-discovery.md#signing-domains)),
and per-device and per-peer token-bucket budgets at the gateway against
exhaustion. They are listed apart from the others on purpose: a threat model
that reads as protection when the protection is still prose is the failure this
document exists to prevent.

**What would close it:** nothing closes the lying-gateway case, because the lie
is about someone else's state. Provisioning is the real control: gateways are
installed by a person ([ADR 0016](../adr/0016-gateways-are-provisioned-not-emergent.md)),
so "which gateways can my device attach to" is an operational decision rather
than an emergent one. Multiple gateways per zone reduce the blast radius of any
single hostile or broken one. Zone-membership exposure would need a design that
does not name recipients to the bridge at all, which no addressing scheme here
provides; it is the same open problem as relay-side metadata.

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

**Never add a catch-all arm to a classifier that matches on an enum.** A
catch-all restores the per-site opt-in that the exhaustive match replaced, and
the leaks this rule exists to stop were all per-site omissions.

A classifier over an open wire **string** is the one exception, because the
input set is unbounded and a final arm is unavoidable. It conforms on one
condition: **the fallback returns a fixed token and never the input.** A
fallback that echoes what it did not recognize is this leak wearing a default's
clothing. See
[ADR 0013](../adr/0013-exhaustive-privacy-classifier.md).

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
