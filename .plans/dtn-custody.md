# Third-party bundle custody (Workstream C1)

**Design record** · Offline Protocol SDK · Written 2026-09-01 · Status: design pass, implementation not scheduled

| | |
|---|---|
| **Program item** | C1, the Layer 3 deliverable of `offline-internet-program.md` §6 |
| **Question** | What does it mean for this device to hold and carry a bundle that is not its own? |
| **Deliverable** | This record. No code, no scheduled implementation. |
| **Prerequisite that is now met** | Custody records key on addresses, and addressing shipped (`off1…`, PRs #327 to #331). |

## Problem

Layer 3 of the program is delivery across partitions measured in hours. The
program doc claims the existing machinery is "already a proto-bundle layer for
*own* messages" and that the work is extending custody to third parties. That
claim is correct, and this record starts by measuring exactly how large the
extension is.

The SDK today runs two disjoint holders, and the gap between them is the whole
feature:

| | This device's own messages | Another device's frame |
|---|---|---|
| Holder | `OfflineProtocol::outbox` (`protocol/mod.rs:132`) | `MeshRelayGovernor::pending` (`protocol/mesh_relay.rs:310`) |
| Durable | yes, sealed under `StateCategory::Outbox` | no, and `PendingRelay` holds an `Instant`, so it is not even serializable |
| Lifetime | 7 days sliding, 28 days absolute (`constants.rs:34`) | **5 seconds** (`RELAY_QUEUE_MAX_OVERDUE`, `protocol/mesh_relay.rs:139`) |
| Retry | full ACK ladder, parking, escalating probes | none: one fan-out, then requeue or abandon |
| Survives restart | yes (`restore_outbox`, `storage.rs:3706`) | no |

So a stranger's frame is held for five seconds in RAM while this device's own
message is held for four weeks on disk. Custody is the work of closing that
asymmetry without breaking the reasons it exists.

Everything else custody needs is already built and needs joining rather than
inventing: durable sealed storage with a category chokepoint, token-bucket
governance with a refusal-dedup discipline, receiver deduplication, an opt-in
config convention with a validation-audit pattern, and a mesh offer path that is
already a one-hop custody handoff.

## Core insight

**Custody in this protocol is replication, never transfer, and it can never be
the correctness path.**

The first half is forced by three independent existing commitments (see
Invariants). The second half is forced by a fact that is easy to miss and
reshapes the whole design: **a custodian holds ciphertext it cannot reseal.**

The sender's own retry path survives a rekey because the sender retains the
plaintext and re-seals on resend (`stage_outbox_reseal`, `data_sync.rs:777-789`,
ADR 0007 Tier 2). A custodian has no plaintext and no keys. It holds bytes
sealed to an epoch it cannot observe, and a 1:1 session rekey is
application-driven and can happen at any moment (`schedule_session_rekey`,
`session.rs:689`; `REKEY_INTERVAL_SECS` is a rate-limit floor, not a cadence,
`types.rs:30`; ADR 0021:214-219 makes `rekey_session` an application call). A
custody copy therefore has an unpredictable validity horizon and no way to
detect its own death.

That sounds like a defect and is actually the property that makes custody cheap
and safe. Because the sender never releases custody, every custody failure
degrades to today's behaviour:

- custodian walks away: sender's ladder still runs, cost is latency
- custodian's copy dies with an epoch: the recipient withholds the
  acknowledgement rather than confirming what it could not read, so nothing
  settles falsely and the sender's resealing re-drive is the recovery (C6)
- custodian never existed: exactly today

Custody is a **latency and partition-tolerance optimization**, not a delivery
guarantee. Designing it as anything stronger requires an authenticated
settlement channel this protocol deliberately does not have.

## Invariants

These are existing commitments. A custody design that breaks one is wrong, not
innovative.

**I1. Custody of an undelivered message stays with the sender until a receiver
positively confirms it.** The single invariant spanning all six state machines
(`docs/state-machines/README.md:28`), restated in ADR 0005:48.

**I2. Only the recipient settles, and a custody receipt is not an
acknowledgement.** `ack_is_from_the_message_recipient` (`send.rs:4591`) already
refuses exactly the frame a custodian would naively send. Its rationale
(`send.rs:4435-4469`) names the failure: without it, any third party that merely
saw the frame settles it, and *"refusing delivery is something those parties
could always do; a false confirmation is the strictly worse one."*

**I3. A refused frame is never recorded as seen, and a frame dropped without
transmitting releases its id.** Four enforcement sites (`mesh_relay.rs:752-760`,
`:983`, `:1212`, and the overdue abandon in `take_due`). The failure it prevents:
one node recording a frame it did not carry blanks every path through that
device for the full 600 s suppression window.

**I4. The offer and the settle arm are one change; neither is correct alone.**
`docs/state-machines/outbox-and-retries.md:159-162` adds: *"Any future path that
hands a parked message to another carrier inherits this obligation."* That
sentence was written for this feature.

**I5. Never hand the CRDT engine bytes that did not come out of a sealed
record.** Only `DataDoc::import_remote` (`doc.rs:350-366`) takes network bytes,
and only after `inspect()`. ADR 0018 §2, ADR 0019. Under `minisize` the SDK is
`panic = "abort"`, so the AEAD tag is the real containment.

**I6. A data frame's space is derived from the authenticated sender or the group
key that opened the ciphertext, never declared in the frame** (ADR 0019 §1,
`data_sync.rs:844-884`). Custody metadata must never name a space, document,
group, or peer outside the AEAD boundary.

**I7. Every data-layer outcome is terminal** (invariant D3,
`docs/spec/data-sync.md:531-542`). There is no custody-level "retry until the
data layer accepts it" to build.

**I8. No ordering, sequence numbers, or exactly-once.** Explicitly rejected
(`docs/spec/data-sync.md:28-32`, ADR 0018:186-189). At-least-once and unordered
is *exactly enough*, and adding more builds machinery this layer does not use.

**I9. Declining to participate is an explicit off switch, never a zeroed dial.**
The recurring principle in `ProtocolConfig::validate` (`config.rs:981`, `:1050`):
a zeroed budget is a silent drop, and refusing to help is `allow_relay`'s job
because it is visible.

## Verified facts

### Code, verified against `main` at 413ae083, 2026-09-01

**The holders.** `OutboxEntry` at `types.rs:1974-1987`; its `reseal` field is
`#[serde(skip)]`, so the retained plaintext is never persisted. `RetryQueue` is
a binary heap (`retry_queue.rs:142`) whose `remove` is O(n) (`:243-252`), which a
custody store with thousands of entries would feel. `MAX_OUTBOX_ENTRIES = 500`,
media 100 (`constants.rs:22,77`). Capacity eviction is terminal (`send.rs:2818-2862`).

**Forwarding budgets already in place** (`protocol/mesh_relay.rs:64-180`, in the
engine crate rather than the transport crate): hop budget 8
sparse / 5 dense, absolute ceiling 16, fan-out 3, node rate 10/s burst 30 with a
5-token own-traffic reserve, per-neighbour 5/s burst 15, pending queue 256,
tracked neighbours 256, suppression cache 8192 ids for 600 s
(`relay_seen.rs:39,42`). Two meters, not one: `take_send_token` for forwards
against `take_own_send_token` for own traffic (`mesh_relay.rs:912-937`).

**Gates.** `try_relay_message` is gated on `relay.allow_relay` and battery
(`receive.rs:441`, `:537-547`; soft floor 30, hard floor
`CRITICAL_RELAY_BATTERY_LEVEL = 15`, `router/src/relay.rs:14`). `offer_to_mesh`
is deliberately *not* gated on `allow_relay` (`send.rs:3395-3402`) because it is
this device's own frame. Custody sits on the accepting side and inherits the
gated path.

**Storage seams.** `ProtocolStateStorage` (`protocol_state_storage.rs:128`).
Core record cap 4 MiB (`types.rs:538`), provider transfer ceiling 8 MiB
(`protocol_state_storage.rs:113`), relationship pinned by two tests including one
that greps the Swift, Kotlin and Python provider sources. Sealing is decided at
one exhaustive match, `StateCategory::requires_sealing` (`storage.rs:255-277`),
through one chokepoint `write_state_record` (`storage.rs:902`) that fails closed
on an unknown category. **Sealing authenticates presence, not absence**
(`storage.rs:251-254`): a seal cannot detect deletion or rollback.

**Write cost is the meter, not bytes.** *"Every built-in provider pays two
device barriers per `store` (the record's own flush plus its directory's) and one
per `delete`, regardless of record size"* (`types.rs:504-507`). Persistence is
deliberately kept off the retry hot path (`send.rs:2934-2945`).

**Size caps.** App content 256 KiB (`types.rs:484`), rich extras 32 KiB
(`types.rs:655`), data-sync frame body 32 KiB (`data_sync.rs:95`), transport
frame 1 MiB (`transport/src/constants.rs:246`). The 32 KiB sync figure is sized
to the mesh, not the record store: unnegotiated BLE fragments into at most 512
pieces of 139 usable bytes, about 69 KiB (`data_sync.rs:87-94`).

**Media is never persisted, by policy.** `media_outbox` entries are not written
(`storage.rs:3605`, `types.rs:1652-1655`); an interrupted transfer surfaces as
`MediaResendRequired`.

**Receiver deduplication is one hour and one thousand ids.**
`DeduplicatorConfig::default` (`deduplicator.rs:41-50`): exact-match mode
(`use_bloom_filter: false`, chosen because bloom drops about 1% of legitimate
messages), `max_tracked_messages: 1000`, `retention_time_secs: 3600`. Both bounds
matter: in a busy zone the count rotates faster than the hour.

**Control-frame freshness bounds any long hold.** A signed control frame is
refused if stamped more than 30 days ago, a window chosen to clear the 28-day
absolute outbox cap (`docs/spec/control-messages.md`, restated
`docs/message-delivery.md:245`).

**Acknowledgements.** An ACK is a `Message` with empty content and metadata keys
(`constants.rs:12-18`, built `send.rs:4417-4425`). `route_ack` (`send.rs:4343`)
is a three-rung ladder: inbound link, then mesh if the frame arrived over mesh,
then DORS. The attribution gate **fails open** when neither outbox holds the id
(`send.rs:4585-4590`), which is what lets a device relay an acknowledgement for
a message that was never its own. There are no hop acknowledgements; a
forwarding node emits a local `MessageRelayed` event and takes on no obligation.

**The parked-DM offer is already a one-hop custody handoff.**
`park_unreachable_dm` (`send.rs:3903-3972`) drops the pending ACK and retry
entry, escalates a per-recipient counter, calls `offer_to_mesh`, and re-enqueues
at `15 << (parks-1).min(6)` seconds capped at 600. `settle_parked_dm_from_ack`
(`send.rs:4652`) is the settle arm I4 names.

**Nostr can never produce an unreachable verdict** (`docs/mesh.md:575`): a
broadcast relay reports no per-recipient delivery, so a Nostr-only device gets no
parking and no mesh fallback. Permanent by construction, and custody does not
change it.

**`AckOptimizer` is dead code.** 447 lines exported at
`reliability/src/lib.rs:20` with zero call sites in the engine or the FFI. Do not
plan custody against it.

**Data-layer carriage properties.** Frames are `__DATA_V1__`, an inner prefix
only (`prefixes.rs:92`, `docs/spec/data-sync.md:42-45`). A duplicate delta is
absorbed at three layers, and the decisive one is `BlobMeta::already_applied`
producing `RemoteImport::AlreadyHave` **before the engine is touched**
(`doc.rs:305`, `:352-354`). An orphan delta is `Parked`: accepted, answers `Ok`,
and is **invisible to every read** until its predecessor arrives
(`doc.rs:266-272`, pinned at `offline-protocol-data/src/tests.rs:512-551`). A
change forked below a compacted replica's trim point is refused because letting
it reach the engine is a `SIGABRT` (ADR 0019 §3, `tests.rs:821-858`). The
in-flight blob digest is written to a sealed record **before** the engine call
and cleared on return (`data_sync.rs:1370-1379`), keyed on blob bytes rather
than carrier or message id.

### External, verified 2026-09-01

**There is no standards-track custody transfer to implement.** RFC 9171 (Bundle
Protocol version 7) carries no custody mechanism; the RFC 5050 custody procedures
were not brought forward. The only successor work,
[`draft-ietf-dtn-bibect`](https://datatracker.ietf.org/doc/draft-ietf-dtn-bibect/),
reintroduces custody by adapting RFC 5050's procedures over bundle-in-bundle
encapsulation, and it **expired at revision 05 (2025-03-15) with no replacement**.

So this record owes RFC 9171 its *vocabulary* (bundle, lifetime, hop limit,
bundle age, the status-report taxonomy) and owes it no mechanism. Custody
semantics here are defined locally and deliberately.

**Incentive literature.** The DTN cooperation literature splits into credit or
currency schemes, reputation schemes, and quota schemes (surveyed in the
Pi / SIS / reward-mechanism line of work). Credit schemes need a ledger and a
settlement authority; reputation schemes need persistent identity plus gossiped
observations. Neither assumption holds in a network whose defining condition is
partition. Quota schemes need only local state, which is why C5 below is a quota
design.

## Decisions

### C1. Custody is replication; the sender never releases

A custodian becomes an *additional* durable holder of a bundle. The depositor's
outbox entry, ACK tracking, retry ladder and park state are untouched by
deposit.

**Rejected: custody transfer** (BPv6 and RFC 5050 style, where the sender
releases once a custodian accepts). It fails three ways here. It violates I1
directly. It requires an authenticated acceptance signal, and I2 exists because
this protocol's acknowledgements are unsigned and a false confirmation is worse
than a refusal. And it converts the walked-away-custodian case from a latency
cost into silent loss, which is precisely the failure ADR 0016 refuses to
introduce one layer down.

The cost of replication is duplicate delivery, which is cheap here (C6) and
which the data layer absorbs for free (C4).

### C2. The custody receipt is a new signed control frame, and it settles nothing

A custodian that accepts a bundle answers with a receipt. The receipt:

- is a **signed control frame** with its own reserved prefix, not an
  acknowledgement, and not routed through the acknowledgement path
- names the bundle id, the custodian's address, and the custody expiry
- **suppresses re-deposit** of that bundle to that custodian, and feeds
  observability
- **never** settles an outbox entry, cancels an ACK timer, or removes a retry
  entry

Naming matters, because a receipt that is called an acknowledgement will
eventually be routed like one. The prefix joins the reserved-prefix registry that
drives injection prevention (`docs/spec/control-messages.md`), and being
signed puts it under the derivation check rather than the message plane.

**Rejected: reusing the ACK frame with a custody flag.** The attribution gate
(I2) would refuse it, and relaxing that gate to admit it re-opens the exact hole
the gate documents.

### C3. Custody lifetime is bounded by the sealing horizon, not by the outbox lifetime

This follows from the core insight. A custodian holds unresealable ciphertext
whose validity ends, unobservably, at the next rekey of the pair (or the next
epoch change of the group). Holding a bundle for seven days therefore mostly
means storing dead bytes: paying storage, battery, and two device write barriers
per record for a copy whose delivery probability decays toward zero.

Custody lifetime should be **hours, not days**, matching the partition scale the
program actually claims, and it must be strictly shorter than the outbox
lifetime so that the sender's resealing ladder always outlives the custodial
copy. A bundle whose custody expires is dropped silently by the custodian; there
is nothing to report, because the depositor never lost anything.

The related bound to respect: for carried **signed control frames**, the 30-day
freshness window is already the ceiling, and it comfortably clears any custody
lifetime in this range.

### C4. Payload classes, and why a delta bundle is the ideal one

Custody is defined per payload class, not per message. The classes:

| Class | Payloads | Carry? | Why |
|---|---|---|---|
| **A. Data-layer state frames** | `delta`, `snap`, `vv`, `blob_gone` | **Yes, first class** | Idempotent, commutative, unordered, terminal in every outcome, bounded to 32 KiB. |
| **B. Data-layer requests** | `need_blob`, `need_snap` | **Never** | Not idempotent *in cost*. Each acted-on copy spends a whole media transfer or a snapshot export, and both endpoints are required to bound it (`docs/spec/data-sync.md:379-388`). A carrier that duplicates one defeats both bounds. |
| **C. Direct messages** | `__MLS_ENC__` | Yes, with the C3 caveat | Subject to epoch death; duplicates outside the dedup window surface as an advisory decrypt failure. |
| **D. Media chunks** | `FileChunk` | **Never in v1** | Media is never persisted, by policy (`storage.rs:3605`). Custody would break that policy, and the chunk-0 purpose mark adds a contradiction-refusal path (`receive.rs:868-897`) a carrier should not be able to influence. |
| **E. Signed control frames** | `__CONN_REQ__` and siblings | Yes, bounded | The 30-day freshness window is the bound and it already clears custody lifetimes. |
| **F. Acknowledgements** | ACK frames | **Yes, and this is half the value** | See C6. |

**Why the delta bundle is the ideal custody payload**, as the program input
asserts and this pass confirms:

1. **Idempotent below the engine.** A redelivered delta is short-circuited by
   `already_applied` into `AlreadyHave` before the CRDT engine is touched
   (`doc.rs:305`, `:352-354`), which is a *safety* verdict on a compacted
   replica, not an optimization.
2. **Commutative and unordered.** The layer's stated contract is that
   at-least-once, unordered, partition-tolerant delivery is exactly enough
   (`docs/spec/data-sync.md:28-32`). Custody supplies precisely that and nothing
   more.
3. **No per-message delivery semantics to preserve.** Nothing in the data layer
   depends on which carrier a delta arrived by, or in what order.
4. **Bounded and small.** 32 KiB ceiling, and a 10,000-operation document
   compacts to under a kilobyte (`doc.rs:456-460`), so steady-state deltas are
   hundreds of bytes.
5. **Recovery exists above every refusal.** Parked imports are refilled by
   anti-entropy, and trimmed-history refusals ask for a snapshot.

**The one caveat that must be named**, because it argues against unbounded
custody lifetime: latency raises the probability that a carried delta crosses a
compaction boundary at the receiver. A stale *delta* refused on trim has a
recovery rung (`need_snap`), but a stale *snapshot* refused on trim has **no rung
above it**, and the replicas simply stay apart. ADR 0019 is explicit that this is
a real limit rather than a corner: *"It takes a partition that outlives a
compaction, which is ordinary, so this is a known gap in 1:1 replication and not
a corner"*, and the honest guarantee is *"replicas that stay in contact converge,
and replicas separated across a compaction may not."* **Custody is a machine for
making partitions outlive things.** It directly increases the frequency of the
one data-layer gap that has no recovery rung, which is a second and independent
argument for C3's short lifetime.

**Recommendation: a v1 implementation should carry Class A only.** It is the
class with no epoch-death exposure worth worrying about (a dead delta is
re-derivable from state, unlike a lost message), no duplicate cost, the smallest
payloads, and the clearest recovery story. Class C is where the emotional appeal
of DTN lives, and it is also where C3 bites hardest. Sequencing Class A first
buys the whole mechanism (store, quotas, receipts, offers) against the easy
payload, and leaves Class C as a policy flip once the machinery has field
evidence.

### C5. Quotas and spam economics

**The scarce resource is durable writes, not bandwidth.** Every `store` costs
two device barriers regardless of record size (`types.rs:504-507`). A quota that
meters only bytes lets a depositor of ten thousand tiny bundles cost more than
one that deposits a megabyte. Meter entries and bytes together, and rate-limit
deposits on the existing token-bucket pattern.

**Deposit rights are tiered by relationship, keyed on `off1…` addresses:**

- **Peers with an established MLS session** get the ordinary budget. A session
  is not free: it costs a key-package exchange, which is what makes this tier
  Sybil-resistant enough to be useful.
- **Strangers** get a much smaller budget, or none, depending on deployment.
  Because addresses are self-certifying, a stranger is cheap to *name* but a
  stranger who has completed a session is not.

**Bounds a custody store needs**, mirroring shapes that already exist:

- per-depositor entry count and byte budget (the pending-queue pattern,
  `types.rs:516,521`)
- a global custody byte ceiling, and a per-record cap under the 4 MiB core cap
- eviction oldest-first with a priority tiebreak, as the outbox does
- refusal is silent to the depositor beyond the absence of a receipt, because a
  detailed refusal is a probing oracle

**Rejected: credit or currency incentives.** They need a ledger and a settlement
authority; a network defined by partition has neither. **Rejected: reputation
scoring.** It needs persistent per-peer observation and gossip, which is the
cooperative-routing layer this codebase deleted in Workstream B3 for having no
production callers and misleading readers about how the mesh works.

Per I9, custody gets an explicit off switch. A zeroed quota must be a
configuration error, not a way to turn custody off.

### C6. Hour-scale acknowledgement semantics, and what "delivered" means

**Only the recipient's acknowledgement settles a message**, and it may arrive by
any path. That is unchanged, and it is the answer to "what does delivered mean
when the carrier walked away": nothing the carrier does or fails to do changes
settlement.

**Carrying acknowledgements back is half the value of custody.** An ACK is
small, idempotent, and terminal, and in a partitioned network the return leg is
as likely to be broken as the forward one. The attribution gate fails open when
neither outbox holds the id (`send.rs:4585-4590`), which is exactly what allows a
third party to relay an acknowledgement it has no stake in. Custody of Class F
therefore needs no new gate, only the storage and the offer discipline.

**Duplicates will arrive outside the deduplication window, by construction.**
The receiver's filter holds 1000 ids for one hour (`deduplicator.rs:41-50`) and
custody latency is hours. The outcomes:

- **Class A**: absorbed as `AlreadyHave` below the engine. No app-visible effect.
- **Class C**: the ciphertext either decrypts (the recipient sees a duplicate
  message, deduplicated by content at the app layer if it cares) or, far more
  likely after hours, fails against a dead epoch.

**The dead-epoch path is safe, and the mechanism is worth stating because it is
what makes C1's "degrades to today's behaviour" concrete rather than hopeful.**
A stale-epoch ciphertext classifies as `SessionDesync` (OpenMLS `WrongEpoch`,
`mls/src/group.rs:393-400`), and the receiver **withholds the acknowledgement
and unmarks the id** rather than, in the code's own words, *"lying 'delivered'
for a chunk we dropped"* (`receive.rs:1473-1499`). So a custody-delivered frame
that cannot decrypt **cannot falsely settle the depositor's outbox entry**. That
closes the one path by which custody could have caused silent loss.

**It has a cost that is not obvious: a custody-delivered stale frame triggers a
re-key on the recipient.** The same branch calls `schedule_session_rekey`. The
blast radius is bounded, because that floor already exists for exactly this
input: `REKEY_INTERVAL_SECS` is *"enforced unconditionally"* and bounds *"a peer
replaying stale-epoch ciphertext (or an injected wrong-epoch frame) to at most
one re-key per this window rather than a storm"* (`types.rs:22-30`). But a
re-key emits a `SESSION_REKEY_TRIGGERED` security warning, and `config.rs:208`
says an integrator is expected to read a sustained rate of it as an attack
signal. Custody raises that rate with entirely benign traffic.

That is the sharper half of the noise problem: the decrypt-failure event is
advisory, but this one is a *security* warning. An implementation must either
distinguish custody-borne frames in that classification or document the new
floor loudly. This is a second, independent argument for C4's Class-A-first
recommendation, since Class A never touches this path.

**Rejected: extending deduplication retention to cover custody latency.** It
would require durable per-peer id sets with their own reconciliation after a
crash, which is the ordering-and-exactly-once machinery I8 refuses. The cost of
not extending it is a bounded amount of decrypt-failure noise, which is
observable and harmless. This is a deliberate trade and integrators watching
`message_decryption_failed` rates need to be told that enabling custody raises
the floor.

### C7. Custody acceptance must not blank the forwarding route

This is the one interaction that needs a mechanism rather than a policy, and it
is the sharpest edge in the design.

`RelaySeenCache` retains a forwarded id for 600 s (`relay_seen.rs:39,42`). That
retention was reasoned about against a 5-second hold. If custody acceptance
records the bundle in the same cache, a custodian that accepts a bundle and then
cannot deliver it **also suppresses its own forwarding of the depositor's
retransmissions of that same frame**, for ten minutes, on a route that is now
known to be slow precisely because the custodian is holding it.

**Name the failure: a node that both holds a bundle and blanks the route to it
converts a helpful device into a black hole.**

Custody and forwarding are different obligations and must not share the
suppression cache. Either custody acceptance does not `observe()` the id in the
forwarding cache at all, or it releases it (`seen.forget()`) at the moment the
bundle moves from the forwarding queue to the custody store. Both are consistent
with I3, which already requires that a frame not transmitted releases its id.

### C8. Opt-in surface, battery, and lifecycle

**`ProtocolConfig::custody`, default off.** The doc comment follows the
`edge_driven_unreachable_dm` model (`retry_queue.rs:33-51`), which is the
codebase's best example of an opt-in: it states the default, what the default
preserves, what enabling trades away, which deployments it is correct for, and a
`MUST NOT` clause. Custody's version of that clause: it must not be enabled on a
device whose storage budget is not the app's to spend.

**Battery.** Reuse the relay floors (soft `min_battery_for_relay` 30, hard
`CRITICAL_RELAY_BATTERY_LEVEL` 15, `router/src/relay.rs:14`). Below the floor a
device **stops accepting new custody but keeps delivering what it already
holds.** Dropping held bundles to save battery is the one local optimization
that converts latency into loss, and it is forbidden.

**Config conventions to honour**: fields optional on the write side so an
omitted field keeps the core default, every field required on the read side so
no binding writes a fallback literal (`docs/mesh.md:553-556`), and construction-
only if the implementation snapshots into buckets, as `mesh_relay` does. Any
value a binding restates needs a drift guard, as
`rn_bridge_retry_fallbacks_match_rust_defaults` does today.

**Validation must name the silent failure for each dial**, per the `mesh_relay`
block (`config.rs:977-1112`), not merely assert positivity.

**Lifecycle**: `wipe_all` (`uniffi/src/lib.rs:7285`) must clear the custody
store. A custody store that survives a logout wipe holds other people's traffic
past the point the user asked for erasure.

**Events**: accepted, delivered, expired, refused. Refusals are aggregate
counters rather than per-depositor events, for the oracle reason in C5.

### C9. Custodians are role-blind and opt-in inside a zone; gateways are a quota class

ADR 0016 refuses emergent gateway promotion because two properties fail for
bridging: most devices structurally cannot bridge, and there is no in-zone
redundancy covering a bridge that leaves. **Both properties hold for custody**,
which is why the forwarding-bias shape transfers here even though it does not
transfer to gateways:

1. Every device can hold bytes. The candidate population is everyone.
2. Redundancy is cheap because custody is replication (C1). Several custodians
   are several copies, and a wrong pick costs storage, never delivery.

So custody needs no role, no state machine, and no promotion: it is an opt-in
flag plus quotas, exactly like forwarding.

Provisioned gateways (the item 32 daemon) are a **higher quota class**, not a
different mechanism: powered, stationary, already provisioned, which is the
shape ADR 0016 endorses. Whether that is normative before the daemon exists is
an open question below.

### C10. Non-goals

- **No custody transfer.** C1.
- **No currency, credit, or reputation incentives.** C5.
- **No ordering, sequence numbers, or exactly-once.** I8.
- **No media custody in v1.** C4 class D.
- **No custody of `need_blob` or `need_snap`.** C4 class B.
- **No emergent custody role or promotion state machine.** C9.
- **No relay-server custody.** The relay has its own store-and-forward and its
  own account model; custody is for the mesh, where no infrastructure exists.
- **No custody-level retry against a data-layer refusal.** I7.

## Security and privacy

**What a custodian learns, and for how long.** A sealed payload is opaque at
every hop (C4 establishes there is no plaintext surface), but the outer
`Message` is not: sender, recipient, message id, app id, priority, TTL, hop
count, timestamp and size are all visible (`core/src/message.rs:506-546`).
Custody extends the retention of that metadata from five seconds to hours, on
disk, for traffic between two other parties. **This is the design's primary
privacy cost and it is not mitigable by sealing**, because routing needs the
recipient. It must be stated plainly in the opt-in documentation, in the same
register the Nostr work used for its routing-tag residual: name the exposure,
name who is exposed to whom, and say what it is not mitigated by.

A custody store is sealed at rest under a new `StateCategory` (the exhaustive
match at `storage.rs:255-277` forces this to be a deliberate choice, and the
chokepoint fails closed on an unknown category). Sealing authenticates presence,
not absence (`storage.rs:251-254`), so a custody store cannot detect that
bundles were deleted from underneath it. That is acceptable: deletion of a
replicated copy is exactly the walked-away case, which costs latency.

**Threats and their answers:**

| Threat | Answer |
|---|---|
| Storage exhaustion by a depositor | Per-depositor entry and byte quotas, global ceiling, oldest-first eviction (C5) |
| Sybil deposit flood | Tiering on established sessions (C5); an address is cheap to mint but a session is not |
| Forged custody receipt | Receipts are signed control frames under the derivation check (C2); an unsigned receipt is refused like any other unsigned control frame |
| A custodian that accepts and never delivers | Bounded by C1: the depositor's ladder still runs. Latency, not loss |
| A custodian that blanks the route it is holding | C7, the one case where the above is not automatically true |
| A custodian falsely settling a message by delivering an undecryptable copy | Not possible: the receiver withholds the acknowledgement on a dead epoch (`receive.rs:1473-1499`), so custody cannot convert epoch death into silent loss |
| Deliberate late delivery to force re-keys | Bounded to one re-key per `REKEY_INTERVAL_SECS` by a floor that is enforced unconditionally and was written for this exact input (`types.rs:22-30`). The residual is signal noise, not work (C6) |
| Redelivery of a blob that aborts the CRDT engine | The per-space quarantine is keyed on **blob bytes**, not carrier or message id (`data_sync.rs:1304-1313`), so a custody-borne redelivery of a killer blob hits the same refusal |
| Custody as a traffic-analysis position | Real and unmitigated; an attacker who volunteers as a custodian collects metadata. Quotas bound volume, not observation. Opt-in framing must say so |

**Draft residual-risk entries** for `docs/security/threat-model.md`, to be added
by the implementation rather than by this record:

- *Custody metadata retention*: a custodian retains outer routing metadata for
  third-party traffic for hours rather than seconds.
- *Custody as a volunteer observation post*: enabling custody in a deployment
  invites nodes whose motive for volunteering is collection.
- *Custody-borne re-key pressure*: late delivery of stale-epoch ciphertext
  raises the `SESSION_REKEY_TRIGGERED` rate with benign traffic, degrading a
  signal integrators are told to treat as adversarial. Related to R2, which
  covers the unauthenticated desync trigger itself.

The acknowledgement channel is already documented as a side channel
(`threat-model.md:158`); Class F custody widens who can observe delivery timing,
and that section needs a sentence rather than a new entry.

## Spec and negotiation impact

Nothing here changes the wire today. An implementation would need:

1. **A capability entry** so a peer advertises custody support. The list is
   append-only and entries are read independently
   (`docs/spec/capability-negotiation.md:72-90`).
2. **A reserved control prefix** for the receipt, added to the registry that
   drives injection prevention (`docs/spec/control-messages.md`).
3. **A spec chapter** covering the deposit frame, the receipt, custody lifetime,
   and the per-class carriage table from C4.
4. **Conformance vectors**, per the E2 precedent: a spec without vectors was
   found unimplementable once already.
5. **Documentation updates**: `docs/message-delivery.md` gains a custody section
   next to parking, and `docs/mesh.md`'s mixed-neighbourhood discussion gains
   the durable-hold case.

## Follow-up work

**None scheduled, by design.** The program defers implementation until this
design pass lands. When it is scheduled, the natural staging is:

1. Custody store, quotas, receipts, and the C7 cache separation, carrying
   **Class A only**.
2. Class F (acknowledgement return), which needs no new gate.
3. Class C behind its own flag, once field data exists on how often epoch death
   wastes a carried copy.
4. Gateway quota class, once the item 32 daemon exists.

## Risks

**The epoch-death rate could make Class C custody nearly worthless.** If
deployments rekey often, most carried direct messages die before delivery and
custody costs storage for nothing. This is the largest unknown in the design and
the reason C4 recommends starting with Class A. It is measurable before it is
expensive: instrument the fraction of carried bundles that fail to decrypt.

**Storage pressure on mobile.** Custody competes with the app's own storage
budget, and the SDK has no visibility into what else the device is holding. The
mitigation is that custody is off by default and quota-bounded.

**Metadata retention is the real cost and is easy to under-communicate.** An
integrator who enables custody to be a good network citizen is also volunteering
to store other people's routing metadata.

**Custody raises a security alarm floor with benign traffic.** Per C6, a
custody-delivered stale-epoch frame triggers a rate-limited re-key and its
`SESSION_REKEY_TRIGGERED` warning, which integrators are told to read as an
attack signal. The rate limit bounds the work, not the signal. If this floor is
not documented and ideally distinguished, custody will be blamed for an attack
that is not happening, or worse, will mask one that is.

**A quota design without field data is a guess.** The numbers in C5 are shapes,
not values. The mesh governor's numbers had the same problem and were tuned
against topology simulations; custody deserves the equivalent before defaults are
frozen.

## Open questions

1. **Is Class C (direct messages) in a v1 at all?** C4 recommends no, on the
   evidence of C3. This is the decision that most changes the feature's size and
   its perceived value, and it deserves an explicit answer rather than being
   settled by implementation order.
2. **What is the custody lifetime default?** C3 argues hours and strictly less
   than the outbox lifetime. The specific number wants field data (see Risks).
3. **Is gateway custody (C9) normative before the item 32 daemon exists?**
   Writing it into the spec now risks specifying a counterpart that does not
   exist, which is the exact failure `docs/reticulum.md` already committed and
   item 34 had to correct.
4. **Does the stranger tier default to zero?** A deployment-shaped question:
   zero is safest and makes custody useless for the disaster-response case where
   nobody has met anybody yet.
5. **Should custody deposit reuse `offer_to_mesh` or be a distinct verb?**
   Reusing it inherits the I4 settle-arm obligation automatically, which is an
   argument for reuse; a distinct verb makes the quota accounting clearer.
