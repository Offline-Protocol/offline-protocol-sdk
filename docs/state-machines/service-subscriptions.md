# Service subscriptions

A consumer holds a lease on one topic of one provider's service, and the
provider sends it updates as they happen. The decision and its reasons are
[ADR 0035](../adr/0035-a-service-can-be-subscribed-to.md); the frames are in
[control messages](../spec/control-messages.md#subscription-frames). This
document is the runtime behaviour: what each side holds, what moves it, and
what each shape prevents.

## Invariants

1. **Every subscription frame is sealed in the 1:1 session, or not sent.** A
   frame that cannot be sealed is not sent, and one that arrives as plaintext
   is dropped.
2. **An update is accepted only from the subscription's provider**, by
   MLS-authenticated sender.
3. **A lease that is not renewed ends the subscription**, on both sides,
   without either side persisting anything: a subscription frame waiting in
   the outbox is never written to its store.
4. **A consumer never delivers an older value after a newer one** for the
   same subscription and key.
5. **At most one `latest` value per subscription and key waits in the
   outbox.**
6. **A provider at a cap refuses the newcomer; it never evicts.** A
   subscription awaiting the application counts against the caps.
7. **A provider holds one subscription per (subscriber, service, topic)**;
   a first subscribe replaces the one before it, a renewal never replaces,
   and a replaced or ended one never returns. The consumer refuses a second
   subscription to a triple it already holds.
8. **A consumer that missed updates gets the state back, or an event saying
   it cannot.**

Invariant 5 is the one place this state machine gives up custody of an
undelivered message before the receiver confirms it, which the
[one invariant that spans all seven](README.md#the-one-invariant-that-spans-all-seven)
otherwise forbids. It does so only for a value that a newer value for the same
key replaces, and the newer value takes custody in its place.

## Consumer

```
              subscribe_service
                     |
                     v
   +-----------> PENDING ----- pending_timeout ------------> ENDED
   |                 |   \---- rejected / not_found / ... --> ENDED
   |   reply accepted|
   |                 v
   |              ACTIVE <---------- reply accepted ---------+
   |                 |                                        |
   |   3 renewals    |                                        |
   |   unanswered    v                                        |
   |              PAUSED --- provider seen: renew now --------+
   |                 |
   |                 +-------- max_pause --------------------> ENDED
   |
   +-- (every renewal goes through the provider's policy again, held or
        not; one it now refuses ends with `policy`. The consumer stays
        ACTIVE or PAUSED while a renewal it no longer holds is decided)
```

| State | Holds | Leaves on |
|-------|-------|-----------|
| `pending` | The subscribe frame, sent or waiting for the session | A reply (`active`, or `ended` with its status), or `pending_timeout_ms`: `session_timeout` if the frame never left, `no_reply` if it did |
| `active` | Stream, per-key seqs (`latest`) or contiguous position and seqs seen ahead (`log`), granted lease, next renewal | Three renewals in a row unanswered (`paused`), an `__SVC_UNSUB__` from the provider, or `unsubscribe_service` (`ended`) |
| `paused` | The same | A reply (`active`, `resumed`), or `max_pause_ms` (`ended`, `provider_unreachable`) |

**Waiting for the session.** `subscribe_service` with no confirmed session
starts one, the way a direct message does, and leaves the subscription
`pending` with its frame unsent. Session confirmation sends it, from the same
transition that starts document replication, because every confirmation path
reaches that transition and only it.

**Renewal** is sent at half the granted lease and carries `resume`: the
stream, and for `latest` the highest seq held and a digest of the (key, seq)
pairs, for `log` the contiguous position. It supersedes the previous renewal
still queued, so a provider out of range for an hour is sent one renewal on
its return, not a hundred and twenty. A renewal that supersedes a resync not
yet answered carries `resync` itself, so the request the application made is
never withdrawn by one the engine made.

**Ordering.** In `latest` an update whose seq is not above the one held for
its key is dropped. In `log` a duplicate is dropped and one that skips ahead
is delivered at once and remembered (at most 256 above the position); the
reliable ladder fills the gap, and the next renewal replays what it did not.
A `log` topic is therefore delivered in arrival order, not seq order, and an
application that needs order sorts by `seq`. A checkpoint (a `log` update
with `snapshot`) moves the position to its seq.

**The position is never taken from an update.** A `log` topic's position
starts at 0 (seqs start at 1), and a reply's baseline only ever raises it.
The reliable ladder keeps no order between entries, so a reply that was
resent can land after the catch-up frames that followed it; those frames are
held above the position, and the baseline then advances through them. Were
the first update to set the position, everything between the baseline and
that update would be dropped as already seen.

**A new stream** on any frame resets what is held and emits
`stream_reset` before the new state is delivered: the provider restarted or
cleared the topic, and keys or positions from before mean nothing now. The
stream left behind is **retired** (the last 4 per topic), and a frame naming
a retired stream is dropped. A frame from before a provider restart can still
arrive from a relay mailbox or a custodian; without retirement it would reset
the consumer to the dead stream, and the live stream's next frame would reset
it back.

**A full pattern.** When the provider stops following new topics for a
pattern subscription at 64, its reply carries `pattern_full: true`, and the
consumer emits `service_subscription_changed` (`active`, `pattern_full`) once.

**Resync** forgets the position, so that the state that comes back is
delivered even where it equals what was delivered before, and asks with
`resync`. Every frame for the subscription carries `resync`, and the
position it had when the resync was asked for, until a reply arrives that is
not a notice (`notice: true`, sent by the provider on its own). A renewal that
read the forgotten position would claim position 0, and a `log` topic would
be replayed from its start, or reported `history_lost` once its buffer had
moved on.

**A pattern reply naming a retired stream** for one topic drops that topic's
position only; the rest of the reply applies. An exact subscription's reply
naming a retired stream is dropped whole.

**A second subscribe to the same triple** (provider, service, topic) while
the first is not `ended` fails at the call with `InvalidArgument`, naming the
existing `sub_id`. Sent, it would replace the first on the provider, whose
renewals would then be dropped until it ended as `provider_unreachable`.

## Provider

| State | Holds | Leaves on |
|-------|-------|-----------|
| awaiting the application | Subscriber, lease, the subscriber's `resume`; counts against both caps | `accept_service_subscription` (admitted), `reject_service_subscription` or `approval_timeout_ms` (gone, `rejected` sent) |
| admitted | Lease, last send time, keys held back by the interval | Lease expiry (gone, no frame), `__SVC_UNSUB__` from the subscriber, `end_service_subscription` (`__SVC_UNSUB__` sent), the service unregistered, or replacement by a first subscribe (no `resume`) to the same topic under a new `sub_id` (retired). A renewal (`resume`) naming another `sub_id` is dropped with no reply and replaces nothing |
| retired | Only its `sub_id`, the last 4 per (subscriber, service, topic). Entered on replacement and on every end either side is told of (unsubscribed, ended, refused, `mode_mismatch`, service unregistered); never on lease expiry | Forgotten when a fifth is retired. A frame naming it is dropped with no reply, so a renewal sent before the subscriber restarted cannot replace the subscription it made since, and one sent before an end cannot subscribe again |

**Admission** is the service's policy: `restricted` (the default) asks the
application, `accept_all` admits, and `rules` admits listed peers and members
of listed groups and asks or refuses the rest. It is applied to every
renewal, not only the first subscribe: a subscriber renews every half lease,
so a policy checked once would keep streaming to a peer it now refuses, such
as one removed from an allowed group, for as long as that peer renews. A
renewal the policy refuses ends the subscription with `policy`. One the policy
admitted on its own and now asks about (`rules`, `fallback: ask`) goes back to
awaiting the application, with `renewal: true` and nothing sent until it
decides; one the application admitted is not asked again. A renewal whose
service, topic or mode differs from the subscription it names is dropped: the
policy read for the frame's service would let a peer refused on one service
renew under another that admits it. A renewal for a
triple the provider holds nothing for is asked about with `renewal: true`
(one for a triple it holds under another `sub_id` is dropped), which says only
that the frame carried `resume`: any peer can set it, so it words a prompt
and never admits.

**Per topic** the provider holds the mode, a random stream id, the next seq,
and either the current value of every key (`latest`) or a buffer of recent
updates and an optional checkpoint (`log`). It holds them whether or not
anyone is subscribed, so the first subscriber gets state without the
application publishing it again. Only the application creates a topic, by
publishing or setting a checkpoint; a subscriber that could would fix the
mode the application then fails to publish in, and leave a topic behind for
every name it tried. A subscription to a topic not yet published is accepted
with no stream and waits for it, and one in the other mode ends with
`mode_mismatch` when the topic is created.

**A pattern subscription** (`prefix*`) follows every topic in its mode whose
name starts with the prefix, up to 64, each with its own stream and
position: the table below applies to each topic separately, and a topic the
subscriber's renewal does not name is caught up as a renewal at position
zero would be (the buffer replayed, or the checkpoint). A topic created after
the subscription is followed from its first publish. **A key filter** narrows
a `latest` subscription to the keys it lists, in the snapshot, live, and in
the digest a renewal is compared against.

**An expired lease is dropped without a frame.** A subscriber that is still
there renews, and the renewal subscribes it again through the policy; one that
is gone would never read the frame.

## An update, from publish to delivery

```
publish_service_update(service, topic, key?, body)
  |
  |-- latest: table[key] = (seq, body); log: buffer.push(seq, body)
  |
  +-- for each admitted subscriber on the topic, or on a pattern that
  |   follows it (or now starts to, while it follows fewer than 64), whose
  |   key filter passes the key:
        latest, sent less than min_update_interval ago -> key marked dirty;
                the newest value goes on the first tick after the interval
        latest, otherwise:
            live direct link and confirmed session -> sealed, ephemeral, once;
                                                      withdraws what is queued
                                                      under the key
            otherwise -> sealed, reliable, withdrawing what is queued under
                         the key first (one value per key waits)
        log -> sealed, reliable, every one
```

What reaches the consumer in each case:

| Case | Catch-up |
|------|----------|
| New `latest` subscriber | Every key, `snapshot` |
| `latest` renewal, same stream, same digest | Nothing |
| `latest` renewal, other stream or digest | Every key, `snapshot` |
| `latest` resync | Every key, `snapshot` |
| New `log` subscriber | The checkpoint and what follows it, if there is a checkpoint; otherwise nothing |
| `log` renewal, position in the buffer | What follows the position |
| `log` renewal, position out of the buffer | The checkpoint and what follows it; with no checkpoint covering it, `history_lost` and the position moved to the present |
| `log` consumer holding more than 256 updates above a gap | Gives the gap up: the position moves past it and `history_lost` is reported for the topic, since no renewal asks for it again |
| `log` resync | The checkpoint and what follows it; with no checkpoint, what follows the position |

## Restart

Nothing is persisted on either side, and that includes subscription frames
waiting to be sent. The outbox is persisted and replays a restored entry
verbatim ([outbox and retries](outbox-and-retries.md), S2 and S3), so a
subscription frame is marked when it is queued and never written to the
outbox's store, as a confirmation probe is not: it is retried in memory for
the life of the process, and a restart has nothing of it to restore. The
mark is set at queueing because a restored entry cannot be recognised: it is
stored as its `__MLS_ENC__` envelope, and the `__SVC_` plaintext was only
ever in memory. A frame that had already left, to a relay mailbox or a
custodian, still arrives; retired streams on the consumer, and retired
`sub_id`s and the rule that a renewal never replaces on the provider, drop
it.

A consumer that restarts holds no subscriptions and subscribes again; the
provider replaces the old one and retires it (invariant 7). A provider that
restarts holds no subscriptions, no retired `sub_id`s and no topic
state, and mints new streams: the next renewal from each consumer is admitted
through the policy again (`renewal: true` on the event), and the consumer
reports `stream_reset`. An application whose `latest` state matters after a
restart publishes it again at start-up.
