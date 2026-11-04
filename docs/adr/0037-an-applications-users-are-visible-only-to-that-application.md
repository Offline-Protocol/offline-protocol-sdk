# 0037. An application's users are visible only to that application

**Status:** Proposed

## Context

Every application built on the SDK already carries every other application's
traffic. Relay targets come from the transports
(`TransportManager::mesh_neighbors`), never from `known_peers`, and no
receive, relay or security path reads `app_id`: the wire format says a
receiver MUST NOT drop or authorize a frame by it
([wire format](../spec/wire-format.md)), and
[R17](../security/threat-model.md#r17-the-application-id-on-every-frame-is-cleartext-and-unsigned)
says why it could not anyway, since the id is cleartext and anyone on the path
can rewrite it. So the mesh is one network, and its density grows with the
installs of every integrator together. That is worth keeping.

What is not worth keeping is that the network is also one *audience*. Today a
phone running application B can learn, from inside B, that a nearby node or a
known address belongs to application A, and A's users see B's users exactly
as they see their own:

- the wrappers emit `NeighborDiscovered` for every neighbour, whatever
  application it runs, and `on_neighbor_discovered` writes it into
  `known_peers` and pushes it a key package;
- `presence_updated`, `PeerRssi` and `NeighborLost` follow the same rule;
- four values name the application in the clear: `app_id` on every frame
  (R17), the Bluetooth LE app tag
  ([R15](../security/threat-model.md#r15-the-bluetooth-le-app-tag-is-unsigned-and-readable)),
  the Android Wi-Fi Direct group name, passphrase and `app` TXT key
  ([R22](../security/threat-model.md#r22-an-android-wi-fi-direct-group-admits-anyone-who-knows-the-application-id)),
  and the browser's nearby room key.

An integrator whose users must not be enumerable by a competitor's
application, or by anyone who repackages one, cannot ship on a shared network
with those properties. The requirement is a security boundary, not a display
filter: B, *modified*, must still learn nothing about A.

The attacker is one of three parties:

| Party | Holds | What it can do after this decision |
|---|---|---|
| 1. Honest application B | the SDK | sees nothing of A |
| 2. Modified application B | the SDK source, any radio | sees "an SDK node with address X" and nothing that names or groups A's users; cannot pass as A |
| 3. A modified application holding A's binary | A's publisher public key | recognises an A node one connection at a time; cannot forge membership |

Party 3's recognition is inherent: whoever holds what A's users use to
recognise each other can recognise them too. Forgery is what this decision
removes.

## Decision

### The invariants

1. **Carriage is never gated.** A node relays, stores and forwards frames for
   every application exactly as before. Nothing in this decision is read on a
   relay path, and no relay decision may ever read membership.
2. **Visibility requires proof.** With a publisher key configured, a peer is
   reported to the application, and the application's automatic side effects
   run for it, only once it has proven membership of the same application. A
   missing proof means "not visible", never "pass" (the shape of
   [ADR 0010](0010-unconditional-leaf-identity-binding.md)).
3. **A member answers no stranger.** A configured node sends nothing back to a
   peer that is neither visible nor named by its own application, and the
   stranger observes what it observes for an address nobody holds.
4. **Nothing names the application in the clear.** Every value a carrier or a
   connecting party can read is either constant across applications or
   derived from a key that party 2 does not hold.
5. **No publisher key, no change.** An application that configures none sees
   every peer, as today.

### Keys and certificates

The integrator generates an Ed25519 publisher key pair offline. The
application ships the public key (`ProtocolConfig.app_publisher_key`,
optional), and the engine derives one secret from it:

```
K_app = HKDF(ikm = publisher_pk, info = "offline-protocol-app-v1" || app_id)
```

A member holds a certificate binding its address:

```
cert = address || not_after || publisher_sig("offline-app-member-v1" || address || not_after)
```

The engine never generates or stores the publisher private key. Two issuing
modes produce the same certificate:

- **Backend** (recommended): the integrator's server signs the address the
  application sends it.
- **Embedded**: the application ships the private key and calls a pure helper,
  `sign_app_membership(private_key, address, not_after)`, which keeps no state
  and never logs, stores or transmits the key. This hands party 3 forgery for
  that application; party 2 is unaffected. The integrator guide and the threat
  model say so in those words.

### The membership exchange

A new reserved control frame, `__MEMBER__`, runs once per new neighbour on any
carrier. It is hop 0 and never relayed, fixed in size, and signed through the
ordinary [control-plane signature gate](0004-control-plane-signature-gate.md)
like every other control frame. The signature proves only the sender address,
which the frame already carries, so no new exemption class is needed.

1. Each side sends `nonce(16) || tag(16)`, where
   `tag = HMAC(K_app, "hello" || nonce_self || addr_self || addr_peer)[0..16]`.
   A node with no publisher key, or with a different one, sends 32 random
   bytes. Every SDK node sends this frame, so sending it reveals nothing.
2. A receiver checks the tag with its own `K_app`. On a mismatch it stops,
   silently. On a match it sends
   `AEAD(HKDF(K_app, nonce_a || nonce_b), cert || sig_identity(transcript))`.
   The transcript signature ties the certificate to this live exchange, so a
   recorded frame does not replay
   ([R16](../security/threat-model.md#r16-the-identity-assertion-is-static-and-replayable-on-every-carrier)
   stays a property of the identity assertion only).
3. A receiver verifies the certificate against `publisher_pk`, its address
   against the frame's sender, `not_after`, and the transcript signature. A
   pass makes the peer **visible**, cached per address until `not_after`.

Party 3 completes steps 1 and 2, since it holds `K_app`, and fails step 3,
since it has no certificate. Party 2 fails step 1 and learns that the peer is
an SDK node, which it knew.

### What the gate covers

The gate covers what the engine does **automatically** because it discovered a
peer: `NeighborDiscovered`, `NeighborLost`, `PeerRssi`, `presence_updated` on
receipt, the key-package push, subscription renewal and the data-sync kick in
`on_neighbor_discovered`. The emission decision moves into the core, and the
FFI choke point (`notify_neighbor_reachable`) and the browser engine's
emission sites ask it.

It does not cover an action the application takes toward an address it
supplied: an explicit send, pair call or session request, or a scanned
invitation anchor. Those addresses join a persisted, bounded **named set**,
written only by those calls and never by anything a frame carries. Without
this exception, pairing an unprovisioned leaf breaks: a phone may start it by
pushing a key package on discovery
([leaf provisioning](../spec/leaf-provisioning.md)), and a leaf with no
certificate never becomes visible.

### Inbound requests

A configured node answers a sender, with an acknowledgement of any kind, a
key package, a Welcome or a reset, only when one of these holds:

- the sender is **visible**;
- the sender is in the **named set**, which is backfilled once, at
  configuration, from every peer that already holds a session with this node,
  so turning membership on does not cut off existing contacts;
- the request carries an **end-to-end membership proof**, sent ahead of the
  key package:
  `AEAD_{K_app}(cert || sig_identity("offline-app-member-e2e-v1" || addr_recipient || key_package_hash))`.

The third case exists because the exchange is hop 0. Two members several hops
apart never see each other as neighbours, and without the proof they could
open a session only if one typed the other's address. Binding the recipient
and the key package stops a captured proof from being replayed to another
member or with another package. A relay sees random bytes.

### Nothing in the clear

- **`app_id` on the wire** is the constant `"offline"` for every frame a
  configured node sends. The real id travels inside the sealed rich payload,
  which gains an optional `app_id` field and is authoritative on receipt
  ([ADR 0008](0008-sealed-rich-payload.md)). Control frames and
  acknowledgements carry only the constant. This is the fix R17 names.
- **The Bluetooth LE app tag** becomes `HMAC(K_app, instance_address)[0..8]`:
  different per phone and per identity, so it groups nothing for party 2, and
  computable by a same-application central from the address it has just read.
- **The Android Wi-Fi Direct group** name, passphrase and `app` TXT key, and
  **the browser's nearby room key**, are derived from `K_app`.

### Leaves

A leaf receives its certificate and the publisher public key after pairing,
inside the paired session, so only its paired phone can provision it. It
stores both durably before acknowledging, then runs the same exchange and the
same inbound gate. The codecs live in `offline-protocol-sealed`
([ADR 0022](0022-one-sealed-layer-shared-with-the-leaf.md)), so the leaf takes
no engine dependency. A leaf reads no clock: it checks `not_after` against the
latest time it was supplied, which bounds freshness after a long gap and is
not authentication, the same stance the leaf takes for key packages. A leaf
with no certificate behaves as today.

## Consequences

- **The shared network keeps its density.** Every node still carries every
  application's traffic, and B's users relay A's messages without ever
  learning A is there.
- **Two small frames per new neighbour**, plus one proof per session started
  across the mesh. Cached per address until `not_after`; bounded by the
  per-link relay bucket.
- **A stranger's message to a member goes unanswered** and runs its retry
  queue to expiry, as for an absent peer. That is the intended observable,
  and it means an application cannot reach a member of another application
  by address alone.
- **Rollout within one application:** a configured build and an unconfigured
  build of the same application see each other as strangers. The integrator
  turns the key on in one release.
- **Older SDK builds** do not reserve `__MEMBER__`, so one that receives the
  frame delivers it to its application as text. The release that adds the
  prefix is the floor for sharing a radio neighbourhood cleanly. This is
  accepted because no fleet is deployed yet, and it is stated in the
  changelog.
- **Losing the publisher private key** stops new certificates. Existing ones
  carry `not_after`, so it is an outage, not a breach. A leaked private key is
  cut off by rotating the key pair, which makes every old member a stranger.
- **The contracts move:** a UDL change (one config field, three functions),
  so every binding is regenerated; a new reserved prefix; an optional field in
  the sealed rich payload; a new derivation for the app tag in three mirrors;
  R15, R17 and R22 are rewritten, and a new threat-model entry states party
  3's residual and embedded mode's forgery.

## Alternatives considered

**Put the certificate in the identity assertion.** Its `signed_data` has no
domain separator, so giving it meaning is a wire break; the assertion is
static and replayable (R16); and any connecting party reads it, which is a
passive census of A's users.

**Send the certificate in the clear and verify it before showing the peer.**
One frame and simple, and party 2 reads every A certificate it meets: the
census again.

**Run an MLS session with every neighbour and send the certificate inside.**
The strongest option, and a full handshake with every node met, including
every node that only carries traffic. The cost scales with the size of the
crowd, which is the case the shared network exists for.

**Filter by `app_id`, or give each application a relay quota.** Both key on a
field anyone on the path can rewrite (R17), so the filter is not a boundary
and the quota is a field an attacker sets. Fairness stays per arrival link,
with the own-traffic reserve.

**Share one engine across applications on a device.** The local API serves one
identity per engine, so applications from different developers sharing one
would read each other's inbound. It is a design for one operator's
applications, never for several developers'.

## What would undo this

A relay, forwarding or custody decision that reads membership. It turns a
visibility boundary into a carriage boundary, and the shared network becomes
one network per application without anything failing.

An emission site added for a neighbour, presence or signal event that does
not ask the gate. The peer leaks through that one event. The source guard
enumerating every emission site is what catches it, so a new site is added to
the guard, never exempted from it.

A reply path added to the engine (a new acknowledgement, error or capability
answer) that does not ask the inbound predicate. It confirms to a stranger
that an address is live, which is the last thing invariant 3 protects.

Writing to the named set from anything a frame carries. The set is the one
input that bypasses proof, and it must stay in the application's hands.

Treating an unconfigured peer, or a failed check, as visible "to be safe". That
is the silent fallback ADR 0010 refuses, and here it turns the boundary off
for whoever sends a bad frame.
