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
can rewrite it. The one carriage path that reads `known_peers` is service
discovery gossip, which takes its fan-out from that map when it originates a
query and at every hop that forwards one; this decision moves it onto the
transports too ([below](#service-discovery)). So the mesh is one network, and its density grows with the
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
| 2. Modified application B | the SDK source, any radio | sees "an SDK node with address X" and nothing that names or groups A's users, beyond the bounded discovery replay stated [below](#service-discovery); cannot pass as A |
| 3. A modified application holding A's binary | A's publisher public key | recognises an A node one connection at a time; cannot forge membership, but can enrol through A's backend as any user can (below) |

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
   run for it, only once it is **admitted** (below): it has proven membership
   of the same application, or the application or a trusted group vouched for
   it. A missing proof means "not admitted", never "pass" (the shape of
   [ADR 0010](0010-unconditional-leaf-identity-binding.md)).
3. **A member answers no stranger end to end.** A configured node sends no
   end-to-end answer to a peer that is not admitted (below), and the stranger
   observes what it observes for an address nobody holds. An end-to-end
   answer is any frame the engine sends because a frame addressed to this
   node, as its final recipient, arrived. Carriage-level answers, such as a
   `__CUSTODY_RECEIPT__` to a depositor
   ([custody](../spec/custody.md)), are outside this invariant: they reveal
   only that an SDK node is live here, which party 2 may learn. Forwarding
   service discovery gossip is carriage too: passing a `__SVC_DISC_Q__` on,
   or a `__SVC_DISC_R__` toward its originator, is never gated. Only a
   provider's own response to a query is an end-to-end answer.
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
  application sends it. Anyone who can run the application can ask for a
  certificate for an address of their own, so party 3 holding A's binary
  enrols rather than forges. In this mode the boundary is the backend's
  enrolment policy (who it signs for, and whom it stops renewing), not the
  signature, and the integrator guide and the threat model say so.
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
   `tag = HMAC(K_app, "offline-app-member-hello-v1" || nonce_self || addr_self || addr_peer)[0..16]`.
   A node with no publisher key, or with a different one, sends 32 random
   bytes. Every SDK node sends this frame, so sending it reveals nothing.
2. A receiver checks the tag with its own `K_app`, and **always** sends the
   second frame, of one fixed size, whatever the result. A node with no
   publisher key sends it too, as random bytes, so a configured node and an
   unconfigured one cannot be told apart by it. On a match it is
   `AEAD(HKDF(K_app, salt = nonce_lo || nonce_hi, info = ... || addr_sender), cert || sig_identity(transcript))`,
   where `nonce_lo` is the nonce of the side whose address sorts lower, so
   both ends derive the same salt. The sender's address is in the key because
   both sides send a response from the same two nonces, and one key for both
   would seal two plaintexts under one key and one nonce. The exact layouts
   are in the [control-message chapter](../spec/control-messages.md#application-membership). On a mismatch it is random bytes of the
   same length. The transcript signature ties the certificate to this live
   exchange, so a recorded frame does not replay
   ([R16](../security/threat-model.md#r16-the-identity-assertion-is-static-and-replayable-on-every-carrier)
   stays a property of the identity assertion only).
3. A receiver verifies the certificate against `publisher_pk`, its address
   against the frame's sender, `not_after`, and the transcript signature. A
   pass makes the peer **visible**, cached per address until `not_after`.
   The cache is cleared whenever `app_publisher_key` changes, so a rotated
   key pair makes every old member a stranger at once rather than at its
   certificate's expiry.

**Nothing observable depends on the match.** Neither size nor cost: on a
mismatch the receiver still performs one signature and one AEAD seal over
dummy input before emitting the random bytes, so the two paths take the same
work and a timing measurement does not separate them. A receiver that stopped
on a mismatch would be an oracle. The hello is a signed control frame, the
deduplicator forgets its id after an hour while the freshness window admits
it for thirty days ([ADR 0023](0023-a-control-frame-states-when-it-was-made.md)),
and R16 lets party 2 label its own link with X's address. Party 2 could then
replay X's hello to Y and learn, from whether Y answered, whether Y runs X's
application: a grouping of A's users, which the attacker table says party 2
cannot get. Because step 2 is sent unconditionally and at one size, the
replay shows party 2 nothing, and the design does not rest on the
deduplicator or the window for this property.

Party 3 completes steps 1 and 2, since it holds `K_app`, and fails step 3,
since it has no certificate. Party 2 fails step 1 and learns that the peer is
an SDK node, which it knew.

The visible cache is keyed by address, and R16 lets a stranger present a
visible address on a link of its own. That stranger still cannot send
anything as X, because every control frame is signed and every data frame is
sealed, so this is R16's existing presence-spoof exposure, not a new one.

### What the gate covers

The gate covers what the engine does **automatically** because it discovered a
peer: `NeighborDiscovered`, `NeighborLost`, `PeerRssi`, `presence_updated` on
receipt, `ServiceDiscovered` for a provider ([below](#service-discovery)), the key-package push, subscription renewal and the data-sync kick in
`on_neighbor_discovered`. The emission decision moves into the core, and the
FFI choke point (`notify_neighbor_reachable`) and the browser engine's
emission sites ask it. The emission gate and the inbound gate ask **one
predicate, admitted** (below); visibility is one of its sources, not the gate
itself. Gating emission on visibility alone would break the leaf pairing
described next, and would cut presence and neighbour events for every
existing contact the release the key is turned on.

It does not cover an action the application takes toward an address it
supplied: an explicit send, pair call or session request, or a scanned
invitation anchor. Those addresses join a persisted, bounded **named set**,
written only by those calls and never by anything a frame carries. The bound
evicts the least recently named address that holds no session; an address
with a live session is never evicted, so the cap can cost only a contact the
application has not used and is not talking to. Without
this exception, pairing an unprovisioned leaf breaks: a phone may start it by
pushing a key package on discovery
([leaf provisioning](../spec/leaf-provisioning.md)), and a leaf with no
certificate never becomes visible.

### Inbound requests

One predicate, **admitted**, sits in front of every end-to-end answer the
engine sends: every acknowledgement (delivery, group, undecryptable), key
package, Welcome, reset or `no_session` reply, confirmation probe or
acknowledgement, connection answer, service answer (`__SVC_SUB_R__` and the
rest) and data-sync reply. It is stated as covering every reactive send, not
as a list of kinds, and the source guard enumerates the send sites, so a new
reply path is added to the guard, never exempted from it. A sender is
admitted when one of these holds:

- the sender is **visible**;
- the sender is in the **named set**, which is backfilled once, at
  configuration, from every peer that already holds a session with this node,
  so turning membership on does not cut off existing contacts;
- the sender **holds a 1:1 session with this node**. Only an admitted peer can
  open one, since a stranger's key package and Welcome are dropped (below),
  so a session outlives the visible entry that let it form, and a contact met
  as a neighbour stays answerable after its cache entry lapses;
- the sender is on the **roster of a group this node holds**, as the MLS
  group state has it after an authorised commit. A roster comes from the
  group, not from a frame's claim: a member added by another member's commit
  was admitted by someone this node already trusts with the group, and
  without this source every group member who was never a neighbour would
  have its messages go unacknowledged and its sends run to expiry;
- the request carries a valid **end-to-end membership proof** (below).

**What happens to a stranger's frame.** Admission decides what the node
*answers*, never what it *carries*, and it is judged per frame class:

| Frame from a sender not admitted | Processed | Answered |
|---|---|---|
| Key package, 1:1 Welcome, group Welcome (`__GRP_MLS_WELCOME__`), connection request, service subscription | No: dropped before any state changes, so no session, no group, no pending package and no subscription forms | No |
| Data frame under a session or group this node holds | Not applicable: holding the session or being on the roster admits the sender | Not applicable |
| Service discovery query or response with no valid proof from its originator or provider | Forwarded as carriage, unchanged; never matched, never reported | No |
| Anything else addressed to this node | No | No |
| A frame addressed to someone else | Relayed as before | Not applicable |

### The end-to-end membership proof

The exchange is hop 0, so two members several hops apart never see each
other as neighbours. Without a proof they could open a session only if one
typed the other's address. The initiator therefore carries a proof **inside**
the frame that opens the session, as an optional `member_proof` field, so the
proof and the request arrive, retry and expire together and no ordering
between two frames is assumed:

- in the **key package payload**, binding that package;
- in the **1:1 Welcome**, binding that Welcome, for the path where the
  initiator built a session from a package it fetched (a Nostr username
  lookup) and the recipient's first frame from it is a Welcome with no key
  package of the sender's to hash;
- in the **group Welcome** (`__GRP_MLS_WELCOME__`), binding that Welcome, for
  the path where an admin invites an address whose key package it fetched the
  same way. The invitee holds no session with the inviter and, until it
  joins, no roster that names it, so without the proof a group invite to a
  member never met would be dropped;
- in the **service discovery query and response**, binding that frame, for
  the provider several hops away that no other source can admit
  ([below](#service-discovery)).

```
salt  = random(16)
k     = HKDF(ikm = K_app, salt = salt, info = "offline-app-member-e2e-key-v1")
proof = salt || AEAD_k(nonce = 0, cert || sig_identity(
          "offline-app-member-e2e-v1" || addr_recipient || kind || hash(frame_body)))
```

`kind` is one byte naming key package, 1:1 Welcome, group Welcome, discovery
query or discovery response, and `frame_body` is the package, the Welcome or
the discovery payload with the proof field absent. The certificate's address
is checked against the frame's sender for the first three kinds and for a
discovery response, which travels as an ordinary unicast from its provider
and must name that provider in its sealed body too. For a discovery query it
is checked against the payload's `originator`, since a query is forwarded hop
by hop and its sender is the last forwarder. `K_app` is shared by every
install of the application, so the AEAD key is derived per proof from a fresh
salt and is used once; a fixed nonce under a fleet-shared key would repeat
across installs. Binding the recipient, the kind and the body stops a captured
proof from being replayed to another member, onto another package or onto
another Welcome of either kind, or onto another discovery frame. A relay that does not hold `K_app` (party 2) sees random
bytes. A relay that does (party 3) can decrypt every proof it carries and
learns which address is opening a session to which: party 3's recognition
residual, extended from one connection to the proofs it happens to carry.

### Service discovery

Discovery is gossip, so it is carriage, and today it is also cleartext: the
query names the `service_id` it asks for and the response names the provider,
`service_id`, version and capabilities, all readable at every hop
([R9](../security/threat-model.md#r9-service-bodies-are-sealed-only-toward-capable-peers-discovery-is-signed-not-encrypted)).
A configured node runs it under four rules, and an unconfigured one runs it as
today (invariant 5).

- **Fan-out never reads membership.** The targets of a query a node
  originates or forwards are its mesh neighbours
  (`TransportManager::mesh_neighbors`), as relay targets are, and nothing
  else. That list consults only the peer-to-peer carriers; the
  infrastructure carriers report no links, so the one off-mesh set the
  engine holds is `known_peers`, which this decision makes an admitted set.
  Fanning out to it as well would be gossip targets chosen by membership.
  Discovery is therefore a mesh mechanism on a configured node: a member
  reachable only over the internet is not found by gossip, and an
  application that knows a provider's address sends it a service request
  directly, which is an explicit send and joins the named set. A node forwards every query
  within its hop budget, and relays every response toward its originator,
  whether or not the sender is admitted, whether or not it can open the
  body, and whether or not it matched, so forwarding is never an oracle.
  Without this rule a configured node would gossip only to admitted peers
  and drop every stranger's query, and discovery for every application would
  stop at the first node of another.
- **The body is sealed under the application key.** Only the routing fields
  travel in the clear: `query_id`, `originator` and the hop budget. The rest,
  the requested `service_id` in a query and the provider, `service_id`,
  version and capabilities in a response, travels as
  `salt || AEAD_k(body)` with
  `k = HKDF(ikm = K_app, salt = salt, info = "offline-app-svc-v1")` and a
  fresh salt per frame. Party 2 sees that address X issued a query, which
  it may learn, and cannot read what was asked or link two queries for the
  same service. A derived service id in the clear would be deterministic and
  so would group every A user asking for the same service, which is why the
  matching id is sealed rather than hashed. A node that cannot open the body
  forwards it unchanged and answers nothing. The cleartext `service_id`
  field stays in the payload, so an unconfigured node still parses the
  frame, and carries a reserved sentinel that names no service and that
  `register_service` refuses. Left absent, it would read as a wildcard to an
  unconfigured provider, which would answer every configured query with its
  whole service list toward the direct sender.
- **Both ends prove membership.** A query carries a `member_proof` of kind
  discovery query, with an empty `addr_recipient` because a query has no
  single recipient; the body hash binds its `query_id` and `originator`. A
  provider answers only a query whose proof verifies against `originator`. A
  response carries a proof of kind discovery response, bound to the
  originator and verified against the sealed provider, and `ServiceDiscovered`
  is emitted only for a response that passes. A provider is usually several
  hops from the originator, so no other admission source holds, and without
  these proofs discovery between members would never return a result.
- **A provider answers the originator, once, while the query is fresh.** The
  response is addressed to `originator` and routed as an ordinary unicast,
  not handed back to the query's direct sender as an unconfigured node does.
  That rule exists so a spoofed originator cannot draw a provider's service
  list toward an arbitrary peer; the proof binds the originator, so the
  spoof is gone, and answering the direct sender would hand a party 2 that
  replays a recorded query a frame addressed to itself whenever the target
  is a member. Freshness needs a clock the originator set, because the
  control-frame timestamp
  ([ADR 0023](0023-a-control-frame-states-when-it-was-made.md)) does not
  survive the gossip: each forwarder re-serialises the query and signs a new
  frame, so the stamp a provider sees is the last hop's and a replayed
  capture always looks new. The sealed query body therefore carries
  `issued_at`, written by the originator and covered by the proof's body
  hash. A provider answers only while `now - issued_at` is within the
  discovery query lifetime, which is no longer than the query dedup window
  (sixty seconds today), and answers a given `query_id` once inside it, so
  "once" and "fresh" are held by the same window. A provider whose clock
  disagrees with the originator's by more than the lifetime answers
  nothing, which costs a result, never a leak.

What remains is traffic analysis, passive and active. Passively, a carrier
that delivers a query to a provider and then carries the response sees that
the provider sent a frame to the originator. That is the exposure every
unicast between two members already has, since sender and recipient travel
in the clear for routing. Actively, party 2 can capture a query and
re-gossip it elsewhere before `issued_at` lapses: every member providing
that service near the replay answers once toward the originator, and a
carrier next to a provider sees that it answered. So party 2 can learn,
for one lifetime per captured query and once per provider, which nearby
nodes provide some service to the application that issued it, without
learning which application or which service. The lifetime and the
once-per-`query_id` rule are what bound it; cover traffic would close it,
and both are stated as part of party 2's residual rather than closed here.

### Nothing in the clear

- **`app_id` on the wire** is the constant `"offline"` for every frame a
  configured node sends. The real id travels inside the sealed rich payload,
  which gains an optional `app_id` field and is authoritative on receipt
  ([ADR 0008](0008-sealed-rich-payload.md)). Control frames and
  acknowledgements carry only the constant. This is the fix R17 names.
- **The Bluetooth LE app tag** becomes
  `HMAC(K_app, "offline-app-ble-tag-v1" || device_id)[0..8]`, over the Device
  id string exactly as served, so a central computes it before it parses anything:
  different per phone and per identity, so it groups nothing for party 2, and
  computable by a same-application central from that instance's address.
  This changes the selection procedure. Today a central chooses an instance
  by App tag *before* it reads Device id
  ([BLE framing](../spec/ble-framing.md#several-instances-behind-one-link)),
  and the new tag cannot be checked without the address. A configured central
  that finds more than one instance reads Device id and App tag on each,
  takes the first whose tag equals `HMAC(K_app, device_id)[0..8]`, and
  otherwise falls back to the existing rules 2 and 3; it then reads Identity
  and verifies it on the chosen instance only. Device id is unauthenticated
  until that verification, so a match only chooses an instance and grants
  nothing. A central with one instance, or with no publisher key, behaves as
  today.
- **The Android Wi-Fi Direct group** name, passphrase and `app` TXT key, and
  **the browser's nearby room key**, are derived from `K_app`.
- **Service discovery bodies** are sealed under `K_app`
  ([above](#service-discovery)); only the routing fields stay in the clear,
  and none of them names an application.

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
- **Two small frames each way per new neighbour**, sent whatever the result,
  plus one proof field per session started across the mesh and per discovery
  query and response. Cached per address until `not_after`; bounded by the
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
  the sealed rich payload; an optional `member_proof` field in the key package
  payload, the 1:1 Welcome and the group Welcome payload, all in
  `offline-protocol-sealed`, and the group Welcome handler among the receive
  sites the source guard enumerates; a sealed body carrying `issued_at` and
  a `member_proof` in the discovery query and response payloads, a reserved
  sentinel in the cleartext `service_id` that `register_service` refuses,
  discovery fan-out confined to the mesh neighbour list, and a configured
  provider answering the originator rather than the direct sender; a new
  derivation for the app tag and a new instance-selection rule in the BLE
  framing chapter and its three mirrors; R9, R15, R17 and R22 are rewritten,
  and a new threat-model entry states party 3's residual, including what it
  reads from the proofs it carries, backend mode's enrolment boundary,
  embedded mode's forgery, and the traffic analysis left to party 2,
  including the bounded discovery replay.

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
one network per application without anything failing. Discovery fan-out is
such a decision: choosing gossip targets from admitted peers rather than from
the mesh neighbours, adding `known_peers` back as an off-mesh leg of the
fan-out, or forwarding only a query whose body opened, makes discovery one
network per application while every message still flows.

A provider that judges a discovery query fresh by the control-frame
timestamp, or that keeps a discovery lifetime longer than its dedup window.
The first accepts any replay, since every forwarder signs anew; the second
answers one captured query more than once. Either turns the bounded replay
into an open census of the application's providers.

An emission site added for a neighbour, presence or signal event that does
not ask the gate. The peer leaks through that one event. The source guard
enumerating every emission site is what catches it, so a new site is added to
the guard, never exempted from it.

A reply path added to the engine (a new acknowledgement, error or capability
answer) that does not ask the inbound predicate. It confirms to a stranger
that an address is live, which is the last thing invariant 3 protects.

A second exchange frame sent only on a match, sized by the result, or cheaper
to produce on a mismatch. That reopens the replay oracle the unconditional
step 2 closes.

An emission site that asks visibility instead of admission. Leaf pairing and
every existing contact's presence break the release the key is turned on.

Weakening a property this design rests on without naming it: the control-plane
signature gate ([ADR 0004](0004-control-plane-signature-gate.md)), which makes
a frame's sender its signer; the freshness window
([ADR 0023](0023-a-control-frame-states-when-it-was-made.md)); and the
transport-sender check that refuses a hop-0 frame whose sender is not the
link's proved peer. Each is why a stranger holding a visible address on a
link of its own still cannot speak as that address.

A roster source read from anything other than the MLS group state after an
authorised commit. A roster a frame can assert is a named set a frame can
write.

Writing to the named set from anything a frame carries. The set is the one
input that bypasses proof, and it must stay in the application's hands.

Treating an unconfigured peer, or a failed check, as visible "to be safe". That
is the silent fallback ADR 0010 refuses, and here it turns the boundary off
for whoever sends a bad frame.
