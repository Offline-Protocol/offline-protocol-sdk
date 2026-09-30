# Custody

## What this chapter is for

A frame this device did not originate is held for five seconds. That is the
whole of what the mesh does for a stranger's message today: a forwarder queues
it, tries to put it on a link, and when no neighbour can take it within
`RELAY_QUEUE_MAX_OVERDUE` the frame is abandoned and its identifier released.
The device's own messages get the opposite treatment: sealed on disk, retried
for seven days sliding and twenty-eight absolute. Custody is the contract that
lets a device close that gap for one class of traffic, by holding a frame it
could not forward for hours rather than seconds and delivering it when a path
appears.

This chapter specifies the deposit, the receipt, the hold, redelivery, the
quotas and the erase. It does not change what any existing frame means, and it
adds one control frame and one reserved metadata key to the wire. Nothing in it
is a delivery guarantee; the [invariants](#invariants) say why, and they come
first because every mechanism below is a consequence of one of them.

Custody is off by default and needs no counterpart to be useful: a device that
never enables it, and a device that never meets a custodian, both behave
exactly as they do today.

## Invariants

Seven things hold regardless of implementation, and a change that breaks one is
a protocol break rather than a refactor.

- **Custody is replication, never transfer.** A custodian is an additional
  holder of a frame. The depositor's outbox entry, acknowledgement tracking,
  retry ladder and park state are untouched by a deposit and untouched by a
  receipt. The failure this prevents is the one the sender-holds-custody
  invariant exists for
  ([state machines](../state-machines/README.md#the-one-invariant-that-spans-all-six)):
  a sender that released a message to a carrier which then walked away would
  have lost it silently. Under this chapter that case costs latency and nothing
  else.
- **A receipt settles nothing.** Only the recipient settles a message, and it
  does so with an acknowledgement, never with anything a custodian sends. A
  receipt MUST NOT settle an outbox entry, cancel an acknowledgement timer,
  remove a retry entry or un-park a message. The receipt is a separate control
  frame with its own prefix rather than an acknowledgement with a flag, because
  a receipt that is called an acknowledgement is eventually routed like one,
  and the attribution gate that refuses acknowledgements from anyone but the
  recipient exists precisely because a false confirmation is strictly worse
  than a refusal.
- **The hold is strictly shorter than the outbox lifetime.** A custodian holds
  ciphertext it cannot re-seal. The depositor survives a session re-key because
  it retains the plaintext and re-seals on resend
  ([ADR 0007](../adr/0007-reseal-on-resend.md)); a custodian has neither the
  plaintext nor the keys, so its copy has a validity horizon it cannot observe.
  A hold that outlives the outbox would keep bytes whose sender has already
  reported the message failed, and deliver a frame the recipient may no longer
  be able to open, to settle an entry that no longer exists. The bound is
  validated at configuration, not documented as advice, because the outbox
  lifetime is itself a dial. It is validated on the custodian, against the
  custodian's own outbox dial, and it holds network-wide only where depositor
  and custodian share a configuration, which this chapter cannot enforce.
  Where they do not, the consequence is bounded by the class carried: a Class
  A copy delivered after the depositor's entry expired is absorbed by the
  recipient, and its acknowledgement names nothing in the depositor's outbox
  and settles nothing. A wasted write, never a false settlement.
- **A custodian never blanks the route it is holding.** Accepting a frame into
  custody MUST NOT record its identifier in the forwarding suppression cache,
  and MUST release it there if the forwarding path recorded it. The cache
  retains an identifier for ten minutes against a five-second hold; a custodian
  that both held a frame and suppressed its own forwarding of the depositor's
  retransmissions of that frame would turn a helpful device into a black hole
  on exactly the route now known to be slow.
- **Class A only, by the depositor's word.** A custodian cannot see inside a
  sealed frame, so it cannot classify one. Custody is therefore an explicit
  deposit in which the depositor asserts the class, and this version accepts
  one class: the document replication frames `delta`, `snap`, `vv` and
  `blob_gone`. Everything else is refused by name, and the
  [class table](#the-classes) says why for each.
- **A deposit comes from the depositor itself, over the link that proved it.**
  A custodian accepts a frame into custody only when the frame's `sender` is
  the address the transport proved for the peer it arrived from, and every
  device that forwards a third-party frame MUST strip the deposit request from
  the copy it transmits, whether or not custody is enabled on it. Nothing else
  keeps custody to one hop: the request is a metadata key, the control
  signature covers the sender, identifier, recipient and content and never the
  metadata, and an `__MLS_ENC__` frame carries no control signature at all, so
  the key rides the wire unsigned and survives any forwarder that does not
  remove it. Without both rules, one frame fanned out to three neighbours, each
  of which fans out to three, is held for hours on every device in a zone, and
  an honest two-hop network deposits a frame at a custodian that keys its
  quotas on the forwarder while answering a sender it may hold no key package
  for. With them, the depositor, the tier key, the capability lookup and the
  receipt's recipient are one address, and it is one the transport proved.
- **Custody is opt-in and has its own erase.** A device holds other people's
  traffic only because its operator said so, and stops holding it the moment
  the operator asks. There is no global wipe in this protocol, so custody
  cannot inherit one; the erase is its own verb and the data-layer wipe calls
  it.

## The classes

Custody is defined per payload class, not per message. The classes are the
ones the design fixed, and only Class A is carried in this version.

| Class | Frames | This version | Why |
|-------|--------|--------------|-----|
| **A. Replication state** | `delta`, `snap`, `vv`, `blob_gone`, sealed to a pairwise session | **Carried** | Idempotent, kind by kind. A duplicate `delta` or `snap` is short-circuited as already applied before the CRDT engine is touched, which is a safety verdict on a compacted replica rather than an optimisation. A redelivered `vv` is an offer, answered from the recipient's live state like any offer, so it costs one exchange that converges on nothing the recipient did not already hold. A redelivered `blob_gone` is a floor report the recipient has already applied. All four are unordered, terminal in every outcome, at most 32 KiB of blob per frame, and re-derivable from state if a carried copy dies with an epoch |
| **B. Replication requests and their answers** | `need_blob`, `need_snap`, `chunk` | **Refused** | Not idempotent in cost. Each acted-on copy of a request spends a whole media transfer or a snapshot export, and both endpoints bound those exchanges on the assumption that a request arrives once. `chunk` is the answer to a group blob request, up to 32 pieces of 32 KiB, and a carried copy repeats an answer the requester already had |
| **C. Direct messages** | Any other `__MLS_ENC__` body | Refused | Subject to epoch death for the whole hold, with no way for the custodian to notice. A carried copy that arrives after a re-key raises the recipient's re-key security signal with benign traffic (see [R19](../security/threat-model.md#r19-custody-borne-re-key-pressure)) |
| **D. Media chunks** | `FileChunk` frames | Refused | Media is never persisted, by policy; a custodian that persisted a chunk would break it, and the chunk-zero purpose mark has a refusal path a carrier must not be able to steer |
| **E. Control frames** | Signed frames under the control-plane gate | Refused | Deferred rather than refused on principle: the thirty-day freshness window already bounds any hold, but each kind has its own consequences on late arrival, and none has been examined for a delivery hours late |
| **F. Acknowledgements** | Frames carrying `ack_for` | Refused | The strongest candidate for the next version, because the attribution gate needs no change to accept a relayed acknowledgement, but it widens who can observe delivery timing and is not in this one |

Group frames are Class A by content and are still refused: a frame sealed once
under a group key is carried by the group message path, never offered to the
mesh as the depositor's own direct frame, and a group epoch changes on every
membership commit. The depositor's engine never writes a deposit request on one.

## Reserved names

This chapter reserves the following. Each exists once, and the registry that
publishes it is named.

| Name | Kind | Where it is registered |
|------|------|------------------------|
| `__CUSTODY_RECEIPT__` | Control-frame prefix, signature-gated | [Control messages](control-messages.md#custody) |
| `__custody` | Hop-local metadata key, engine-written, stripped by every forwarder | [Reserved metadata keys](wire-format.md#reserved-metadata-keys) |
| `data_versions` entry 7 | Capability | [Capability negotiation](capability-negotiation.md) |

## The deposit

A deposit is not a frame. It is the depositor's own frame, offered to its
neighbours exactly as it is offered today, with one reserved metadata key that
says what the frame carries.

### What the depositor writes

The depositor MUST write the key `__custody` with the value `data`, and MUST
write it only when all of the following hold:

1. The frame is the depositor's own, sealed to a pairwise session (its outer
   prefix is `__MLS_ENC__`), and the depositor is handing it to neighbours
   because it cannot reach the recipient itself.
2. The sealed body is a Class A replication frame. The depositor knows this
   from the plaintext it retains for re-sealing on resend; a frame whose
   plaintext it no longer holds, for instance after a restart, is offered
   without the key.
3. The frame is being offered over a mesh transport to a neighbour whose
   address the transport has proved. The key is never written on a frame sent
   to the relay, over the internet, or to a gateway, each of which has its own
   store-and-forward.

The depositor is always the frame's `sender`. A deposit is made only by the
device that originated the frame, and only directly: a device never writes the
key on a frame it is forwarding for somebody else, and a custodian never
deposits a held frame onward.

The value is a class token. `data` names Class A and is the only token defined.
A custodian MUST refuse a deposit whose token it does not know, and MUST NOT
infer a class from anything but the token.

### What a forwarder does

A device that transmits a third-party frame MUST remove `__custody` from the
copy it transmits, at the point where it adjusts the frame's hop fields, whether
or not custody is enabled on it and whether or not it understands the key. The
key is not signed and is not inside the sealed body, so nothing else stops it
travelling on. The failure this rule prevents is stated under the invariants:
without it a two-hop network deposits a frame at a custodian that never saw
its sender, and a zone holds every frame everywhere.

An application MUST NOT write `__custody`; it is engine-written, like the two
signature keys. Nothing authenticates the token beyond the transport identity
of the peer that handed the frame over, and nothing needs to: a false assertion
costs the depositor's own recipient exactly what a direct resend of the same
frame would have cost, and a depositor can already send that directly.

### What a custodian does on arrival

A frame carrying a deposit request is, first, an ordinary forward. The
custodian queues it, tries to put it on a link and hands it onward if it can,
under every gate and budget that governs forwarding today. Custody begins only
where forwarding ends.

**A custodian holds only what it could not forward.** Acceptance happens at the
point where a forward has waited past `RELAY_QUEUE_MAX_OVERDUE` without reaching
a link and would be abandoned. At that point the forwarding path has already
released the frame's identifier from the suppression cache, which is what
makes the fourth invariant a consequence of existing code rather than a new
mechanism. A frame that transmitted is never held: holding every copy of every
fan-out would multiply storage by the fan-out at each hop, for frames another
device is already carrying.

A queued forward that the governor marked as a held frame being redelivered
([Redelivery](#redelivery)) is returned to the store at this point, and is not
judged by the table below: it is not a deposit, its refusal would not be a
refusal, and it is not counted as one. The drop point MUST NOT touch the
suppression cache for a marked forward: the held intake never recorded the
identifier there, so releasing it is at best a no-op and at worst releases an
entry the depositor's own retransmission legitimately holds if the ordinary
intake admitted one in the meantime.

At that point the custodian MUST accept the frame into custody when every one
of these holds, and MUST refuse otherwise:

| Condition | Refusal reason when it fails |
|-----------|------------------------------|
| Custody is enabled | `disabled` |
| The frame carries `__custody` | `no_request` |
| The token is `data` | `unknown_class` |
| The outer prefix is `__MLS_ENC__` | `not_sealed` |
| The frame arrived from a peer whose address the transport proved | `unproven_peer` |
| The frame's `sender` is the address the transport proved for that peer | `not_depositor` |
| The frame is not addressed to this device | never fails: a frame for this device is delivered, not held |
| No frame with this identifier is already held | `duplicate` |
| The depositor's tier admits it: a peer with an established session under the session tier, any other proven peer under the stranger tier | `stranger_refused` |
| The depositor's entry and byte budgets have room, or the overflow policy makes room | `depositor_full` |
| The global entry and byte budgets have room, or the overflow policy makes room | `store_full` |
| The battery is above the soft relay floor | `battery` |

The `unproven_peer` and `not_depositor` rows together make the depositor one
address: the peer that handed the frame over, the frame's `sender`, the key the
tiers are looked up under, the peer whose capabilities gate the receipt, and
the receipt's recipient. A frame whose `sender` is not the arrival peer reached
this device through a forwarder, which means the forwarder did not strip the
key; the custodian refuses it and MUST strip the key itself if it transmits the
frame onward.

A refusal is silent. The depositor learns only that no receipt came, which it
cannot distinguish from a lost receipt or an absent custodian. A detailed
refusal would be an oracle for probing a device's quotas and its session list,
so refusals are counted by reason for the operator and never answered on the
wire.

A duplicate deposit, one for an identifier already held, is not stored, not
answered and counted. The first receipt already suppressed re-deposit at a
depositor that received it; a depositor that did not will deposit again on its
next retry, and absorbing that here costs nothing.

### What is held

An accepted frame becomes one sealed record under the custody storage category,
holding:

- the frame exactly as it arrived with the `__custody` key removed, hop fields
  as the forwarding path adjusted them;
- the depositor's address, which is both the frame's `sender` and the peer the
  frame arrived from, so redelivery never hands it back;
- the wall-clock time of acceptance, in milliseconds since the Unix epoch,
  never a monotonic instant: a monotonic clock does not run while the device is
  off, and a custodian powered off for a week would otherwise wake believing
  its hold had just begun;
- the class token.

The record is sealed for the same reason the pending-decrypt queue is sealed:
it is other people's ciphertext plus routing metadata about them, and it
outlives the process. Sealing authenticates presence, not absence, so a
custodian cannot detect that held records were deleted from under it; that is
the walked-away case, which costs latency and nothing else.

## The receipt

A custodian that accepted a frame answers with a receipt, and this is the only
frame custody adds to the wire.

### Shape

The receipt is a control frame with the reserved prefix `__CUSTODY_RECEIPT__`
followed by a JSON body:

```
__CUSTODY_RECEIPT__{"v":1,"id":"<held frame's message id>","hold_ms":<u64>}
```

| Field | Meaning |
|-------|---------|
| `v` | Body version, `1` |
| `id` | The identifier of the frame now held |
| `hold_ms` | How much longer the custodian will hold it, relative to the receipt's own timestamp. Relative rather than absolute so the depositor applies it to its own clock and skew cannot expire a valid receipt |

The custodian's address is the frame's `sender`; the depositor's is its
`recipient`. Unknown fields MUST be ignored. A receiver MUST refuse a body it
cannot parse, or whose `v` it does not know, exactly as it refuses any
malformed control frame: silently, without an acknowledgement.

### Authentication

The receipt is signature-gated like every other control frame in the registry:
signed over the control-plane canonical payload (`offline-ctrl-v2`, or
`offline-ctrl-v1` where that is what the depositor negotiated), verified against
the key its sender's address derives from, and refused outside the freshness
window ([Control messages](control-messages.md#the-control-plane-signature-gate)).
It introduces no signing domain of its own. An unsigned receipt is refused by
the gate before this chapter is consulted.

### Sending rules

A custodian MUST send at most one receipt per accepted frame, at acceptance,
addressed to the depositor and transmitted once over the link the deposit
arrived on, to that peer. It is never placed in the custodian's outbox, never
offered to the mesh, and never sent through a relay or a gateway: it names a
neighbour that was on a link a moment ago, and a receipt that cannot be put on
that link is dropped and counted, because the depositor deposits again on its
next retry and the custodian absorbs the duplicate. The receipt MUST NOT
request an acknowledgement: it is advisory, and putting it on the retry ladder
toward a neighbour that may already have walked away spends a retry budget on
nothing.

A custodian MUST send a receipt only to a depositor whose key package advertised
`data_versions` entry 7. A peer that does not reserve the prefix does not know
the frame as a control frame, so it treats the receipt as an application
message: with encryption on, it refuses it as inbound plaintext from a peer
that has proved it can encrypt, which its threat model classifies as a security
refusal and records as one; on a plaintext-only deployment, it hands the
receipt to its application as a text message. The gate exists so that a
custodian never provokes either. Acceptance of a deposit is not gated on the
entry: a depositor whose capabilities are unknown, which is the ordinary case
for a stranger, is judged by its tier and simply gets no receipt.

### What the depositor does with it

A depositor that receives a valid receipt for an identifier still in its outbox
MUST suppress further deposit requests for that identifier toward that
custodian until `hold_ms` elapses, and MAY surface the fact for observability.
It MUST NOT do anything else with it: the second invariant lists what a receipt
never touches. Suppression state is in memory; losing it at a restart costs one
duplicate deposit, which the custodian absorbs.

A receipt whose `id` names nothing in the depositor's outbox is ignored and
counted. That is the ordinary shape of a receipt arriving after the recipient's
acknowledgement settled the message, and it is also the shape a forged receipt
for a never-sent identifier would take, which is why it is not an error.

## Redelivery

Held frames leave custody by delivery, by expiry or by erase.

**On neighbour discovery.** When a neighbour is discovered on any mesh
transport, the custodian walks its held frames and queues each eligible one
toward that neighbour through a **dedicated intake** of the forwarding
governor, not the ordinary one. The ordinary intake would refuse or mangle a
held frame in three ways: it consults the suppression cache, which would refuse
a second re-origination toward another neighbour within the cache's ten-minute
window; it spends a hop, while a held frame is transmitted with the hop fields
it was stored with, because the custodian is the same forwarder it was when
the frame arrived; and it charges the per-peer rate of the frame's arrival
peer, hours after that peer sent anything. The held-frame intake:

- MUST skip the suppression check and the hop accounting;
- MUST take the queue capacity, the send budget (the forward budget, never the
  own-traffic reserve) and the per-neighbour rate toward the neighbour it
  targets, and MUST obey `allow_relay` and the battery floors;
- MUST NOT target the depositor;
- MUST mark the queued forward by its held identifier, so that when it reaches
  no link within the overdue window the drop point returns it to the store
  rather than judging it as a deposit: not a refusal, not counted as one, no
  new record and no new receipt;
- MUST record the single target neighbour on the marked forward, which is
  transmitted to that neighbour only and never through target selection: a
  fan-out at flush time would defeat at most once per neighbour.

A frame whose recipient is the discovered neighbour is queued toward that
neighbour and released from the store on transmission: the custodian has no
way to know more than that the bytes left, and the recipient's
acknowledgement, if any, travels the ordinary ladder back to the depositor,
never to the custodian. Any other frame is re-originated toward the neighbour
at most once per distinct neighbour during its hold, with the set of
neighbours tried bounded the way tracked neighbours are bounded, and stays in
the store after transmission until its hold ends.

**What the custodian transmits never carries `__custody`.** The key is removed
at acceptance, and the custodian is a forwarder like any other for the rule
that a forwarder strips it.

**Expiry.** A record is expired when its acceptance time is older than the
`hold_ms` in force, judged in wall time at restore and at every sweep against
the configuration of that moment, the way a control frame's freshness is
judged against the window of the verifier that receives it. So a lowered
`hold_ms` applies to records already held, and a custodian that was powered off
for longer than its hold delivers nothing stale when it returns. An expired
record is dropped, counted under `expired`, and nothing is sent to anybody: the
depositor never lost anything, so there is nothing to report, and a
`message_failed` for a frame this device did not originate would be a lie to an
application that never sent it.

**At the recipient.** A redelivered frame is an ordinary inbound forward. The
receiver's deduplication, its acknowledgement decision and its stale-epoch rule
apply unchanged, and nothing about custody changes any of them:

- A copy that arrives after the original is deduplicated when it lands inside
  the receiver's window (two thousand identifiers over twenty-four hours,
  persisted). When it does not, a `delta` or `snap` is absorbed below the CRDT
  engine as already applied, a `vv` is answered from live state as one more
  offer, and a `blob_gone` is a floor report already applied.
- A copy sealed to an epoch the recipient has since left classifies as a
  session desync **when the recipient has crypto recovery enabled, which is
  the default**. The recipient then withholds the acknowledgement and unmarks
  the identifier rather than confirming what it could not read, so the copy
  cannot falsely settle the depositor's entry, and the depositor's own
  re-sealing resend is the recovery. The same branch schedules a rate-limited
  session re-key and emits the re-key security warning; what integrators
  should read from that is stated under
  [R19](../security/threat-model.md#r19-custody-borne-re-key-pressure).
- **With crypto recovery disabled, the same copy is a terminal decrypt
  failure**: dropped and acknowledged, and that acknowledgement settles the
  depositor's outbox entry for a frame the recipient never read. A direct
  resend never reaches this path, because the depositor re-seals it to the
  live epoch; a custody copy is exactly what does. For Class A the loss is
  recoverable and the recovery is the exchange itself: the recipient's version
  vector still lacks the change, so the next version offer between the two
  re-offers it, and the depositor's document state, not its outbox, is the
  source. This is a third reason Class A is the only class carried, and a
  deployment that disables crypto recovery on any device SHOULD leave custody
  off.

## Quotas

The scarce resource is durable writes, not bandwidth: every built-in storage
provider pays a device barrier per stored record and another per delete,
whatever the record's size, so a quota that metered bytes alone would let ten
thousand tiny deposits cost more than one large one. Entries and bytes are
metered together, per depositor and globally, in the shape the pending-decrypt
queue already has.

| Dial | Reference default | Bound, and the failure the bound names |
|------|-------------------|----------------------------------------|
| `enabled` | `false` | The off switch. Refusing to hold other people's traffic is this dial's job, never a zeroed budget's |
| `hold_ms` | 6 hours | `0 < hold_ms < retry.outbox_max_lifetime_ms`, refused at configuration otherwise: a hold that outlives the outbox delivers frames whose sender has already reported them failed. Judged in wall time from each record's acceptance timestamp against the value in force at the sweep, so lowering it expires records already held |
| `max_entries_per_depositor`, `max_bytes_per_depositor` | 64 entries, 2 MiB | Session tier. Both must be positive when `enabled`, or the store is on and holds nothing, which is the silent dial the off switch exists to replace. The byte cap must admit at least one replication frame at its ceiling, or every deposit is refused as `depositor_full` |
| `max_entries`, `max_bytes` | 512 entries, 16 MiB | Global. Each must be at least its per-depositor sibling, or the per-depositor dial can never be reached |
| `stranger_max_entries`, `stranger_max_bytes` | 0, 0 | Stranger tier. Zero means deposits are accepted only from peers with an established session, which is the default and is stated rather than inferred; a deployment that wants strangers held raises both |
| `overflow_policy` | drop oldest | What happens when a budget is full: evict the oldest held frame to admit the new one, or refuse the new one. The per-depositor caps are what keep one depositor's flood from evicting everyone else's frames under drop-oldest |

The defaults are shapes awaiting field data, exactly as the forwarding
governor's were before topology measurements tuned them. The bounds are
normative; the numbers are the reference implementation's.

**Tiers are keyed on the proven arrival peer.** A depositor is under the
session tier when this device holds an established MLS session with the peer
that handed the frame over, and under the stranger tier otherwise. A session
costs a key package exchange, which is what makes the session tier resistant
to an attacker who can mint addresses for free.

**Battery.** Below the soft relay floor a custodian accepts no new deposits.
Held frames are never dropped for battery: dropping them is the one local
optimisation that converts latency into loss, and redelivery simply pauses with
the forwarding gate until the floor is cleared.

**Counters.** A custodian counts, at least: accepted, delivered, re-originated,
expired, duplicates, and each refusal reason in the acceptance table. Counters
are aggregate and never per depositor on any wire, for the oracle reason above.

## Erase

`erase_custody` deletes every held record and resets the counters. It MUST be
callable on its own, and the data-layer wipe MUST call it: a custody store that
survived a logout would hold other people's traffic past the point the user
asked for erasure. A device that starts with custody disabled MUST erase any
held records at restore rather than keep them, because nothing would ever
deliver them.

At restore with custody enabled, a custodian walks its records under a budget,
drops every record whose acceptance time is older than the current `hold_ms`,
and keeps the rest exactly as stored. Restore never sends anything.

## What a custodian learns

A sealed payload is opaque at every hop, but the outer message is not: sender,
recipient, identifier, application id, priority, hop fields, timestamp and size
are readable by whoever holds the frame. Custody extends the retention of that
metadata, for traffic between two other parties, from five seconds in memory to
hours on disk. This is the design's primary privacy cost, it is not mitigable by
sealing because routing needs the recipient, and it is stated as
[R20](../security/threat-model.md#r20-a-custodian-retains-routing-metadata-about-third-parties)
so that an operator enabling custody to be a good citizen knows what they are
also volunteering to store.

## Conformance

The receipt body is the one encoding this chapter adds, and it has no frozen
vector yet: the chapter precedes the codec, and a vector computed from prose
alone pins nothing. The vector lands with the implementation, in the crate that
holds the codec, and [Conformance](conformance.md) lists this chapter among
those that are not yet surfaces until then.

## What this chapter does not specify

- **Custody transfer.** The sender never releases; the first invariant.
- **Any incentive scheme.** Credit needs a ledger and reputation needs gossip,
  and a network whose defining condition is partition has neither. Quotas are
  local state and are enough.
- **Ordering, sequencing or exactly-once.** The replication layer is
  at-least-once and unordered by design, and custody supplies exactly that.
- **Classes B to F.** Named in the table so the refusal is by name, not
  specified for carriage.
- **A custody role, promotion or election.** Every device can hold bytes and
  several custodians are several copies, so custody is a flag and a set of
  quotas, never a state machine.
- **Relay or gateway custody.** The relay has its own store-and-forward and its
  own account model. A gateway is a device like any other here; if the gateway
  contract gains a custody verb, that chapter defines it.
- **Custody-level retry against a data-layer refusal.** Every data-layer
  outcome is terminal, and a held frame is delivered once per path, never
  retried against the recipient's verdict.
