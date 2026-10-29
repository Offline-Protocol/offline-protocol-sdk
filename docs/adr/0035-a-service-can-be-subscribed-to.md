# 0035. A service can be subscribed to

**Status:** Proposed

## Context

`MeshServices` answers questions. A consumer sends `__SVC_REQ__`, the
provider answers once with `__SVC_RESP__`, and the exchange is over: one body
of at most 64 KiB each way, no state kept on either side after the answer.

Device coordination needs the other shape: one device tells the devices
interested in it what changed, as it changes, without being asked each time.
A battery level, a position, a leader election, a sensor reading, a door that
opened. Built on request/response this is polling, which is wrong on both
axes: a poll interval short enough to feel live is mostly wasted frames, and
one long enough to be cheap misses the change by the interval. Built on direct
messages it is a fan-out loop every application writes again, with its own
subscriber list, expiry, sequence numbers and answer to "what does a device
that was away get".

The pieces it needs already exist and are each settled:

- **The 1:1 MLS session** seals a service body and authenticates its sender
  (`service_seal.rs`, `svc_versions` entry 1).
- **Ephemeral sends** ([ADR 0032](0032-ephemeral-sends-go-once-over-a-live-link.md))
  go once over a live link, sealed strictly, with no outbox, acknowledgement
  or retry.
- **The reliable ladder** (outbox, acknowledgement, retry, re-seal on resend
  per [ADR 0007](0007-reseal-on-resend.md)) carries a frame across the mesh,
  the relay and custody.
- **`forget_outbound_message`** withdraws a queued message from the retry
  queue, the acknowledgement tracker and the outbox together, for a message
  that was superseded rather than failed.

What is missing is the contract that ties them together: who is subscribed,
until when, to what, in which delivery, and how a device that missed updates
gets the state back.

### What this must not inherit

Service requests have a stated exception to "a downgrade loses the feature,
never the confidentiality"
([capability negotiation](../spec/capability-negotiation.md#the-exception-svc_versions)):
the first request to a provider this device has never met travels as signed
plaintext, because that form predates sealing and is the floor older peers
depend on. A subscription has no such floor. It is new, every peer that
understands it also seals, and it lives long enough that waiting for a
session costs nothing. Copying the exception would put a stream of live device
state in the clear for every carrier. **Subscription frames are sealed or not
sent.**

## Invariants

Each names the failure it prevents.

1. **Every subscription frame travels inside `__MLS_ENC__`, in the 1:1 session
   between consumer and provider.** One found in plaintext is dropped, never
   routed. Otherwise any hop could inject updates into a consumer, or read a
   provider's live state.
2. **An update is accepted only from the provider its subscription names**, by
   MLS-authenticated sender. A `sub_id` is an identifier that appears in
   events and logs, not a credential.
3. **A subscription ends unless it is renewed, and nothing of it outlives the
   process that made it.** Both sides hold a lease, neither persists the
   subscription, and no subscription frame queued in the (persisted) outbox
   survives a restart. Otherwise a provider that crashed, or a consumer that
   walked away, leaves the other side streaming to or waiting on a ghost, and
   durable subscription state is state that is wrong after a crash. A queued
   frame is durable subscription state too: replayed after a restart, it
   names a stream or a `sub_id` that no longer exists (see
   [Restart](#restart)).
4. **A consumer never surfaces an older value after a newer one** for the
   same subscription and key. Reliable resends, custody and ephemeral frames
   can cross; without this a retried position from a minute ago overwrites
   the current one.
5. **At most one `latest` value per subscription and key waits in the
   outbox.** A newer value withdraws the older one. Otherwise a device out of
   range for an hour comes back to an hour of positions, delivered in
   whatever order the ladder manages, each costing a frame.
6. **At capacity, a new subscription is refused, never made room for by
   evicting an existing one.** Eviction lets a flood of fresh identities (one
   keygen each) kick every legitimate subscriber, the same reasoning as
   `record_svc_sealed_peer`. A subscription awaiting the application's answer
   counts against the caps like an admitted one; otherwise the queue of
   undecided subscriptions under `restricted` would be the unbounded one.
7. **One subscription per (consumer, service, topic).** A new `sub_id` for the
   same triple replaces the old one on the provider, and the replaced one
   never comes back. Otherwise a consumer that restarted and subscribed again
   is streamed to twice until the first lease runs out, or, when a frame
   naming the old `sub_id` arrives late, the two replace each other in turn.
8. **A consumer that missed updates can always get the state back, or is told
   in an event that it cannot.** Never a silent gap.

## Decision

### Shape

A subscription is a lease held by a consumer on one **topic** of one
registered service of one provider. A topic has one of two **modes**, fixed by
its first use:

- **`latest`** is for state. Each update carries a **key**, and only the
  newest value per key matters. The provider's engine keeps the current value
  of every key on the topic, so a consumer can always be sent current state.
- **`log`** is for events. Every update matters. The provider's engine keeps a
  bounded buffer of recent updates and, optionally, a **checkpoint** the
  application sets: a body that summarises everything up to a sequence number.

A publish with a key creates a `latest` topic, one without a key a `log`
topic; a subscription names the mode it expects and is answered
`mode_mismatch` if the topic already has the other one. A subscription never
creates a topic. One to a topic not yet published waits for it, and ends
with `mode_mismatch` if the topic is created in the other mode. Were a
subscription to create the topic, any peer with a session could fix its mode
before the application's first publish (which would then fail) and leave a
topic behind for every name it tried, even when refused.

```
Consumer A                                   Provider B
    |                                            |  register_service("fleet.v1")
    |-- subscribe_service(B, "fleet.v1",         |
    |      "battery", latest) -> sub_id          |
    |   [no confirmed session: start one,        |
    |    state pending]                          |
    |=========== MLS session confirmed ==========|
    |-- __SVC_SUB__ (sealed) ------------------->|  policy: accept_all, restricted, or rules
    |                                            |  [restricted: service_subscribe_received,
    |                                            |   app calls accept_service_subscription]
    |<-- __SVC_SUB_R__ accepted, stream, lease --|
    |<-- __SVC_UPD__ snapshot, one per key ------|
    |   service_subscription_changed(active)     |
    |                                            |  publish_service_update(.., "pct", "81")
    |<-- __SVC_UPD__ seq 7 key "pct" ------------|  live link: ephemeral; else one superseding
    |   service_update_received                  |  outbox entry per key, across the mesh
    |-- __SVC_SUB__ renew (stream, seq, digest)->|  at half the lease
    |<-- __SVC_SUB_R__ accepted (+ snapshot if   |
    |      the digest says A is missing state) --|
    |-- resync_service_subscription(sub_id) ---->|  on demand: always a full snapshot
    |-- unsubscribe_service(sub_id) ------------>|  __SVC_UNSUB__
```

### Topic patterns and key filters

Two narrowings of what a subscription is sent, both applied by the provider,
so that what the consumer did not ask for never leaves the provider's device.

**A topic pattern** is a topic prefix followed by one `*` (`rooms/*`), or `*`
alone for every topic of the service. One subscription then follows every
topic in its mode whose name starts with the prefix, including a topic first
published after the subscription was made, up to 64 topics
(`MAX_PATTERN_TOPICS`). A `*` anywhere but the end is refused, so that a later
reading of one as a mid-topic wildcard is never taken by this version as a
literal. Each followed topic keeps its own stream, seqs, digest and
checkpoint: a pattern subscription is caught up and recovered topic by topic,
exactly as an exact subscription to each would be, and an update names its
topic. A topic in the other mode is not followed rather than refused, since a
pattern does not name it.

A pattern that follows 64 topics follows no more, and both sides are told
(invariant 8). The provider's application gets `pattern_full` once. The
consumer gets `pattern_full: true` on the reply, either the one that answers
its subscribe or renewal or, when the 64th topic is reached later, a reply
the provider sends for the same `sub_id` at that moment. The consumer emits
`service_subscription_changed` with state `active` and reason `pattern_full`
once per subscription. Without it, the consumer's view of "every topic under
`rooms/`" silently stops growing.

A topic the consumer has never heard of, missing from its renewal, is one it
holds nothing of: the provider catches it up from position zero on that
topic's stream, which replays the buffer or sends the checkpoint. Without
that, the first update of a topic created while the consumer was out of range
would be lost for good, because the consumer cannot ask for a topic it does
not know exists.

**A key filter** lists the `latest` keys to be sent, each exact or a prefix
ending in `*` (at most 32 entries). The provider sends only those keys, in the
snapshot and live, and computes the renewal digest over them only; a digest
over every key would differ from what a filtered consumer holds at every
renewal and resend the snapshot each time. The consumer drops a key outside
its filter even from its own provider. A `log` subscription takes no filter:
its consumer tracks a contiguous position, and an update withheld by a filter
would be a gap that never fills.

`params` stays the application's. It is handed to the provider's application
untouched, as before; the SDK does not parse it, and filtering is a separate,
declarative field because the provider's engine must apply it per update
without a call into the application, which a binding cannot answer
synchronously.

### Admission policy

Each registered service has a subscription policy, set with
`set_service_subscription_policy`:

| Policy | A new subscription is |
|--------|----------------------|
| `restricted` (the default) | Held, and the application is asked with `service_subscribe_received`. It calls `accept_service_subscription` or `reject_service_subscription`. Unanswered after `approval_timeout_ms`, it is rejected with `approval_timeout` |
| `accept_all` | Accepted by the engine. Any peer with a confirmed session can watch the service |
| `rules` | Accepted when the subscriber is in `allow_peers` or a member of one of `allow_groups`; otherwise either asked (`fallback: ask`) or rejected (`fallback: reject`) |

The default is `restricted` because "who can watch this device" should be a
decision, not a default. A subscription to a service the provider has not
registered is answered `not_found` with no event, as a request is.

A renewal the provider no longer recognises (it restarted, or the lease
lapsed) goes through the policy again, with `renewal: true` on the event.
**`renewal` is the subscriber's claim, not the provider's knowledge:** after a
restart the provider holds nothing to check it against, so it means only that
the frame carried `resume`, which any peer can include. An application may use
it to word its prompt ("this device was watching before"), never to admit
without the decision it would otherwise make; an application that wants
returning subscribers admitted without asking puts them in `allow_peers`.
A renewal of a subscription the provider holds goes through it too, and one
the policy now refuses ends with `policy`. Otherwise a peer removed from an
allowed group, which renews every half lease and so never lapses, would be
streamed to for as long as it kept renewing.

### Getting the state back

A consumer misses updates when an ephemeral frame is lost on a link that
dropped, when the provider restarted, or when a `log` subscriber was away
longer than the buffer holds. There are three ways back, and the first two
need nothing from the application:

1. **Every renewal checks.** In `latest` mode the renewal carries a digest of
   the (key, seq) pairs the consumer holds. The provider compares it with its
   own table and sends a full snapshot when they differ. In `log` mode the
   renewal carries the highest contiguous seq, and the provider replays what
   follows from its buffer.
2. **The stream id.** The provider mints a random `stream` per topic per
   process. A provider that restarted, or whose application cleared the topic
   with `clear_service_topic`, has a new stream, and a consumer that sees one
   resets and reports it (`stream_reset`) before taking the new state. A
   stream the consumer already left is never taken again (retired streams,
   under [Consumer side](#consumer-side)).
3. **`resync_service_subscription(sub_id)`** asks for it now. In `latest` mode
   the answer is always a full snapshot. In `log` mode it is a replay from the
   consumer's position, or the checkpoint and what follows it when the buffer
   no longer reaches back that far.

When a `log` consumer's position is out of the buffer and there is no
checkpoint covering it, the reply says `history_lost`, the consumer moves its
position to the present and emits `service_subscription_changed` with that
reason (invariant 8). An application that cannot afford that sets a
checkpoint (`set_service_topic_checkpoint`) whenever its state is worth
summarising.

### Public API (engine, `OfflineProtocol`)

Consumer:

```rust
pub fn subscribe_service(&mut self, provider: &str, service_id: &str, topic: &str,
                         mode: SubscriptionMode, params: &str) -> Result<String>;   // sub_id
pub fn subscribe_service_with(&mut self, provider: &str, service_id: &str, topic: &str,
                              options: SubscribeOptions) -> Result<String>;     // + keys
pub fn unsubscribe_service(&mut self, sub_id: &str) -> Result<bool>;
pub fn resync_service_subscription(&mut self, sub_id: &str) -> Result<()>;
pub fn service_subscriptions(&self) -> Vec<SubscriptionInfo>;
```

Provider:

```rust
pub fn set_service_subscription_policy(&mut self, service_id: &str,
                                       policy: SubscriptionPolicy) -> Result<()>;
pub fn accept_service_subscription(&mut self, sub_id: &str) -> Result<()>;
pub fn reject_service_subscription(&mut self, sub_id: &str, reason: &str) -> Result<()>;
pub fn end_service_subscription(&mut self, sub_id: &str, reason: &str) -> Result<()>;
pub fn publish_service_update(&mut self, service_id: &str, topic: &str,
                              key: Option<&str>, body: &str) -> Result<PublishReport>;
pub fn set_service_topic_checkpoint(&mut self, service_id: &str, topic: &str,
                                    body: &str) -> Result<()>;
pub fn clear_service_topic(&mut self, service_id: &str, topic: &str) -> Result<()>;
pub fn service_subscribers(&self, service_id: &str) -> Vec<SubscriberInfo>;
```

`publish_service_update` publishes to the topic, not to a subscriber: the
engine fans out to every active subscription on it and keeps the topic's table
or buffer whether or not anyone is subscribed, so the first subscriber gets
state without the application republishing it.

Events (JSON through `EventCallback`, like every service event):

| Event | Side | Fields |
|-------|------|--------|
| `service_subscribe_received` | provider | `sub_id`, `service_id`, `topic`, `mode`, `params`, `sender`, `renewal` |
| `service_subscription_changed` | both | `sub_id`, `role`, `service_id`, `topic`, `peer`, `state` (`pending`, `active`, `paused`, `ended`), `reason` |
| `service_update_received` | consumer | `sub_id`, `service_id`, `topic`, `key`, `seq`, `body`, `snapshot`, `provider_peer_id` |

None carries `encrypted`, unlike request and response: every subscription
frame was sealed (invariant 1).

### Wire

Four frames in the reserved `__SVC_` namespace, JSON bodies like the existing
service frames, each only ever inside `__MLS_ENC__`:

| Prefix | Direction | Body |
|--------|-----------|------|
| `__SVC_SUB__` | consumer to provider | `{v, sub_id, service_id, topic, mode, params, keys?, lease_ms, resume?: {stream, seq, digest, topics?}, resync}` |
| `__SVC_SUB_R__` | provider to consumer | `{v, sub_id, status, reason?, lease_ms, stream, seq, history_lost, topics?, pattern_full?}` |
| `__SVC_UPD__` | provider to consumer | `{v, sub_id, topic?, stream, seq, key?, body, snapshot}` |
| `__SVC_UNSUB__` | either | `{v, sub_id, reason}` |

`status` is `accepted`, `rejected`, `not_found`, `mode_mismatch` or `full`.
For a pattern subscription, `resume.topics` and the reply's `topics` carry
`{topic, stream, seq, digest}` and `{topic, stream, seq, history_lost}` per
topic in place of the single position, and `topic` on an update names the
topic it was published to. `__SVC_SUB__` is also the renewal and the resync
request: the same `sub_id` again, idempotent. `seq` in the reply is the
baseline the consumer may treat as caught up to before the catch-up frames
that follow it.

`seq` is per topic, starts at 1, and is strictly increasing within a stream;
position 0 is "nothing of this stream yet". A `latest` update carries the seq
at which its key took that value; a checkpoint carries the seq it summarises
up to. The digest is 64-bit FNV-1a over, for each (key, seq) pair the
consumer holds in byte order of the key, the key's UTF-8 bytes, one zero
byte, and the seq as eight big-endian bytes. A key cannot contain a zero
byte, so no two tables encode alike, and both sides compute the same value
from the same pairs. A collision makes a renewal miss a difference until the
next one differs, which costs freshness, never order (invariant 4 still holds
per key).

**Versions.** `v` is 1. No earlier form of these frames was ever released,
so there is nothing below it to be compatible with. A receiver reads a frame
whose `v` is above its own by the fields it knows and ignores the rest. A
later version that adds a field which narrows what the provider sends (as the
key filter and the topic pattern do in this one) raises `v`, and a consumer
that used such a field and is answered at a `v` that predates it ends the
subscription with `unsupported` and tells the provider, rather than receive
what it did not ask for. A change to what an existing field means is not a
new `v`: it takes a new `svc_versions` entry.

**Order.** The reliable ladder gives no order between two outbox entries: a
reply lost on its first transmission is resent after backoff, while the
catch-up frames that followed it go out at once. The consumer therefore
never takes a `log` position from an update. A topic's position starts at 0,
a reply's baseline only ever raises it, and an update above the position is
remembered above it like any skip-ahead update (see
[Consumer side](#consumer-side)), so a catch-up frame that overtakes its reply
is counted rather than mistaken for the baseline. `latest` needs nothing more than its per-key seq check.

The frames are added to the prefix registry (`INTERNAL_PREFIXES` and the
[control messages](../spec/control-messages.md#service-discovery) chapter).
None of the four is a prefix of another.

**Capability.** `svc_versions` entry 2 (`SVC_SUB_V1` in
`offline-protocol-sealed`) means "routes the four subscription frames from a
session slot". It is advertised from a signed key package when subscriptions
are enabled. A peer whose signed key package omits it fails
`subscribe_service` at the call; a peer whose key package this device has not
seen is tried, and an older receiver drops a decrypted `__SVC_` frame it does
not know, so the subscription ends at its pending timeout. A peer that has
sent this device a sealed subscription frame stays capable through a replayed
older key package.

### Delivery

| Frame | Delivery |
|-------|----------|
| `latest` update | Live direct link and a confirmed session: sealed ephemeral send (the ADR 0032 path). Otherwise: a sealed reliable send that **supersedes** the previous queued update for the same subscription and key (invariant 5) |
| `log` update | Sealed reliable send, every one |
| `__SVC_SUB__` (subscribe, renew, resync) | Sealed reliable send, superseding the previous one for the same subscription |
| `__SVC_SUB_R__` | Sealed reliable send, superseding the previous one for the same subscription |
| `__SVC_UNSUB__` | Sealed reliable send |

Superseding is what makes `latest` live across hops: the reliable ladder
already reaches a provider several hops away, through the relay or through
custody, and withdrawing the previous entry before queuing the new one keeps
what waits for an absent subscriber at one value per key. An ephemeral send
withdraws a queued entry for the same key too, since it is newer. A
superseded frame that had already left (handed to a relay mailbox or a
custodian) can still arrive, late; the consumer's per-key seq check drops it
(invariant 4).

**Rate.** A `latest` subscription is sent at most one update per
`min_update_interval_ms` (default 50). A faster publish marks the key dirty
and the newest value is sent on the next tick after the interval, so the
backpressure is coalescing, not a queue. `log` updates are not coalesced; the
outbox's per-peer window is their bound.

**Ratchet.** These are 1:1 sends, so ADR 0032's group budget does not apply.
An ephemeral frame is encrypted only when a live link exists; frames lost on a
link that dropped mid-send advance the sender ratchet by at most 20 a second
for the length of the outage, inside the 8192-generation window. A superseded
reliable frame is never delivered, which leaves a gap the receiver skips like
any lost frame.

### Consumer side

The consumer keeps, per subscription: state, provider, service, topic, mode,
`stream`, the highest seq per key (`latest`) or the contiguous position and
the seqs seen above it (`log`), the granted lease and the renewal schedule.

- **Ordering (invariant 4).** In `latest` mode an update whose seq is not
  above the one held for its key is dropped. In `log` mode a duplicate is
  dropped and one that skips ahead is delivered at once; the ladder fills the
  gap, and the next renewal replays it if it does not. **A `log` topic is
  therefore delivered in arrival order, not seq order**, and an application
  that needs order sorts by `seq`. Holding a skip-ahead update back until the
  gap filled would stall every later event behind one lost frame until the
  next renewal, half a lease later.
- **Retired streams.** A consumer that moves to a new stream for a topic
  remembers the one it left (the last 4 per topic) and drops any frame that
  names one of them. Otherwise a frame from before a provider restart, still
  in a relay mailbox or with a custodian, resets the consumer back to the dead
  stream, and the next frame from the live one resets it again.
- **Renewal** is sent at half the granted lease. Three renewals in a row
  without an answer move the subscription to `paused`; rediscovery of the
  provider renews at once. Paused for longer than `max_pause_ms` (default 10
  minutes), it ends with `provider_unreachable`.
- **Pending.** A subscription with no reply after `pending_timeout_ms`
  (default 120 s) ends with `no_reply`, or `session_timeout` if the session
  never confirmed.

### Provider side

The provider keeps, per topic: mode, `stream`, next seq, the `latest` table or
the `log` buffer and checkpoint; and per subscription: subscriber, lease,
admission state, last send time and dirty keys.

- **Accept** sends `__SVC_SUB_R__` and then the catch-up: every key of a
  `latest` topic; for a `log` topic, the checkpoint and what follows it if one
  exists, otherwise nothing.
- **Lease sweep** runs on the engine tick. An expired subscription is dropped
  with no frame; the consumer, if it is alive, renews, and the renewal goes
  through the policy again.
- **Revocation** (`end_service_subscription`) sends `__SVC_UNSUB__` and stops
  sending. It is a promise, not cryptography: the subscriber still holds a 1:1
  session with the provider.
- **Unregistering a service** ends every subscription on it.
- **Replacement (invariant 7).** A subscription that a newer `sub_id` for the
  same (consumer, service, topic) replaced is retired: the provider remembers
  the last 4 retired `sub_id`s per triple and drops a frame naming one,
  without a reply. Otherwise a renewal under the old `sub_id`, sent before the
  consumer restarted and still in a mailbox or with a custodian, replaces the
  live subscription, and the live one's next renewal replaces it back.

### Restart

Nothing of a subscription survives a restart on either side, and that
includes its frames waiting to be sent. The outbox is persisted and replays a
restored entry verbatim
([outbox and retries](../state-machines/outbox-and-retries.md), S2 and S3),
so without a rule a provider that restarted would send the updates and
replies of streams it no longer has, and a consumer that restarted would send
renewals for `sub_id`s it no longer holds.

- **At start-up the engine withdraws every queued `__SVC_SUB__`,
  `__SVC_SUB_R__`, `__SVC_UPD__` and `__SVC_UNSUB__`** from the outbox, the
  retry queue and the acknowledgement tracker together, as
  `forget_outbound_message` does for one, before the first flush. The outbox
  holds the prefixed plaintext, so this is a prefix test on each restored
  entry, and it settles them with no event: the application never sent them,
  the engine did.
- **What already left cannot be withdrawn.** A frame handed to a relay
  mailbox or a custodian before the restart still arrives. The retired
  streams (consumer) and retired `sub_id`s (provider) above are what make
  such a frame harmless rather than a reset.

After a restart the provider's tables are empty and every stream is new; an
application whose `latest` state matters republishes it at start-up.

### Limits

Constants in `payloads.rs` beside the existing service limits; each is
reported rather than silent.

| Limit | Value | At the limit |
|-------|-------|--------------|
| Update or checkpoint body | 64 KiB (`MAX_SERVICE_BODY_SIZE`) | `PayloadTooLarge` at the call; dropped on receipt |
| Topic | 1 to 128 bytes, `A-Z a-z 0-9 . _ - /`; a subscription may end it in `*` | `InvalidArgument` |
| Topics one pattern subscription follows | 64 | Further matching topics are not followed |
| Key filter | 32 entries, `latest` only | `InvalidArgument`; `invalid_subscription` on the provider |
| Key | 1 to 256 bytes of UTF-8, no zero byte (the digest's separator) | `InvalidArgument`; dropped on receipt |
| `params` | 4 KiB | `PayloadTooLarge` |
| Subscriptions a provider holds, admitted or awaiting the application | 256 | `full`, never eviction (invariant 6) |
| Subscriptions one peer holds on a provider, admitted or awaiting the application | 16 | `full` |
| Retired streams a consumer remembers per topic; retired `sub_id`s a provider remembers per triple | 4 | The oldest is forgotten |
| Subscriptions a consumer holds | 64 | `InvalidArgument` at `subscribe_service` |
| `latest` table per topic | 256 keys, 256 KiB | `PayloadTooLarge`; `clear_service_topic` starts over |
| `log` buffer per topic | 256 updates or 512 KiB, oldest dropped first | Resume past it: checkpoint, or `history_lost` |
| Lease | 30 s to 600 s, default 120 s | Clamped |

### Where the code goes

`offline-protocol-services` stays I/O-free: it returns frames and events, as
`MeshServices` does.

- `services/src/subscriptions/` (new): the payloads' handling, `Provider` and
  `Consumer` state machines driven by frames, calls and `tick(now)`.
- `services/src/payloads.rs`: the four payloads, prefixes and limits.
- `services/src/events.rs`: the three events.
- `offline-protocol-sealed`: `SVC_SUB_V1 = 2`.
- `offline-protocol/src/protocol/service_subscriptions.rs` (new): the public
  methods, policy evaluation, the delivery choice and superseding, and the
  hooks (session confirmed, neighbour rediscovered, `process()` tick, and the
  start-up withdrawal of queued subscription frames).
- `message_dispatch.rs`: sealed routing for the four prefixes; plaintext ones
  dropped.
- `send.rs`: advertise entry 2; record it on receipt.
- `config.rs`: `ServiceSubscriptionConfig` under `ProtocolConfig`.

## Phasing

1. **Core, in Rust.** Everything above: frames, capability, the sealed-only
   path, both state machines, admission policies, leases, the three recovery
   paths, topic patterns and key filters, `latest` over ephemeral and
   superseding sends (so across hops), `log` over the reliable ladder with
   checkpoints, and the restart rules. Tests with engines over links.
2. **Bindings and an example.** The UDL surface, every binding regenerated
   together, the browser worker, and a device coordination example
   (`examples/web-coordination`). `ServiceSubscriptionConfig` is not on the
   bindings' config record yet, so a binding runs the defaults.
3. **Fan-out.** Group-backed topics (one MLS group per topic: cryptographic
   revocation and one encryption for many subscribers, at the cost of a
   commit per join and ADR 0032's ratchet-gap budget) change how groups
   behave rather than how subscriptions do, and are their own decision,
   proposed in an ADR of its own.

## Consequences

- An application gets a live, sealed, self-healing stream in a handful of
  calls, and stops writing a fan-out loop with its own expiry and gap handling.
- Fan-out costs one encryption per subscriber. Fine for a room of devices; a
  topic with hundreds of subscribers wants the group-backed form.
- Neither side persists anything. A restart costs a re-subscribe (consumer)
  or a new stream (provider), both reported.
- A renewal that races an update in flight can trigger a snapshot that was
  not needed. That is the price of a digest check with no per-subscriber
  acknowledgement state, and it is bounded by the table size once per lease
  half.

## What would undo this

- **Giving subscription frames the plaintext floor** "for compatibility".
  There is nothing older to be compatible with.
- **Persisting subscriptions** to survive restarts. It recreates the ghost
  subscriber the lease exists to prevent.
- **Dropping the start-up withdrawal** because "the lease handles it". The
  lease ends a subscription nobody renews; it does nothing about a restored
  frame that names a dead stream or a replaced `sub_id`, which resets or
  replaces the live one on arrival.
- **Dropping the retired streams or retired `sub_id`s** because the start-up
  withdrawal "already clears the queue". It clears this device's queue, not a
  relay mailbox or a custodian.
- **Admitting on `renewal: true`.** Any peer can set it.
- **Sending `latest` updates through the reliable ladder without
  superseding.** An absent subscriber's outbox fills with values nobody will
  read, and the per-peer window holds back the one that matters.
- **Dropping the consumer's per-key seq check** because superseding "already
  orders things". A superseded frame already in a relay mailbox or with a
  custodian still arrives.
- **Evicting the oldest subscriber at capacity** instead of refusing the
  newest.
- **Making `accept_all` the default.**
