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
   without either side persisting anything.
4. **A consumer never delivers an older value after a newer one** for the
   same subscription and key.
5. **At most one `latest` value per subscription and key waits in the
   outbox.**
6. **A provider at a cap refuses the newcomer; it never evicts.**
7. **A provider holds one subscription per (subscriber, service, topic)**;
   the newest replaces the one before it.
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
   +-- (a renewal the provider no longer holds goes through its policy
        again; the consumer stays ACTIVE or PAUSED meanwhile)
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
its return, not a hundred and twenty.

**Ordering.** In `latest` an update whose seq is not above the one held for
its key is dropped. In `log` a duplicate is dropped and one that skips ahead
is delivered at once and remembered (at most 256 above the position); the
reliable ladder fills the gap, and the next renewal replays what it did not.
A checkpoint (a `log` update with `snapshot`) moves the position to its seq.

**A new stream** on any frame resets what is held and emits
`stream_reset` before the new state is delivered: the provider restarted or
cleared the topic, and keys or positions from before mean nothing now.

**Resync** forgets the position, so that the state that comes back is
delivered even where it equals what was delivered before, and asks with
`resync`.

## Provider

| State | Holds | Leaves on |
|-------|-------|-----------|
| awaiting the application | Subscriber, lease, the subscriber's `resume` | `accept_service_subscription` (admitted), `reject_service_subscription` or `approval_timeout_ms` (gone, `rejected` sent) |
| admitted | Lease, last send time, keys held back by the interval | Lease expiry (gone, no frame), `__SVC_UNSUB__` from the subscriber, `end_service_subscription` (`__SVC_UNSUB__` sent), the service unregistered, or replacement by a newer subscription to the same topic |

**Admission** is the service's policy: `restricted` (the default) asks the
application, `accept_all` admits, and `rules` admits listed peers and members
of listed groups and asks or refuses the rest.

**Per topic** the provider holds the mode, a random stream id, the next seq,
and either the current value of every key (`latest`) or a buffer of recent
updates and an optional checkpoint (`log`). It holds them whether or not
anyone is subscribed, so the first subscriber gets state without the
application publishing it again.

**An expired lease is dropped without a frame.** A subscriber that is still
there renews, and the renewal subscribes it again through the policy; one that
is gone would never read the frame.

## An update, from publish to delivery

```
publish_service_update(service, topic, key?, body)
  |
  |-- latest: table[key] = (seq, body); log: buffer.push(seq, body)
  |
  +-- for each admitted subscriber on the topic:
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
| `log` resync | The checkpoint and what follows it; with no checkpoint, what follows the position |

## Restart

Nothing is persisted on either side. A consumer that restarts holds no
subscriptions and subscribes again; the provider replaces the old one
(invariant 7). A provider that restarts holds no subscriptions and no topic
state, and mints new streams: the next renewal from each consumer is admitted
through the policy again (`renewal: true` on the event), and the consumer
reports `stream_reset`. An application whose `latest` state matters after a
restart publishes it again at start-up.
