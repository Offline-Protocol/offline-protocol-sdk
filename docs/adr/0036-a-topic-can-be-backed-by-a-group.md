# 0036. A topic can be backed by a group

**Status:** Proposed. Not implemented. Builds on [ADR 0035](0035-a-service-can-be-subscribed-to.md),
whose third phase it is the remaining part of.

## Context

ADR 0035 seals every subscription frame in the 1:1 session between consumer
and provider. That is right for a room of devices and wrong at two edges:

- **Fan-out cost.** A publish is one encryption and one frame per subscriber.
  A topic with a hundred subscribers costs a hundred of each, on a phone, for
  every update. MLS already has the answer: one group, one encryption, one
  frame that every member can read.
- **Revocation.** `end_service_subscription` stops sending, but the
  subscriber keeps its session, so ending a subscription is a promise and not
  cryptography. A removed group member cannot read the next epoch.

A group-backed topic answers both, and it is not a subscription change. It
changes what a group is in this SDK, because today every MLS group is a
conversation the application created and the user sees:

- `create_group` emits `group_created`, appears in `list_groups`, registers
  with the relay when there is one, and is admin-checked like any group.
- `invite_to_group` needs the invitee's key package in hand, sends a Welcome
  as `__GRP_MLS_WELCOME__`, and commits to every member as
  `__GRP_MLS_COMMIT__`. The receiving handlers join or apply what arrives
  and emit `group_member_added` and the rest; a Welcome is joined from any
  peer, with no check that it was asked for.
- Any member may commit: the epoch-fork check has the lexicographically first
  member issue a key update, and a removal that leaves no admin promotes one.
- Every group send spends ratchet generations for absent members, and ADR
  0032 bounds that with a budget of 256 per epoch per process.

A topic group that took any of these paths as they are would show up in the
user's group list, tell the relay which devices watch which provider, let a
subscriber commit, and spend a commit and a Welcome on every subscriber that
joins or leaves.

Two facts about key packages shape the join. A key package is single-use
(RFC 9420), and the 1:1 session spends the one the provider held: building
the session consumes the consumer's package and deletes it from memory and
storage. Nothing pushes another to a peer whose session is confirmed. So when
a subscription is admitted, which ADR 0035 does only over a confirmed session,
the provider normally holds no key package for the subscriber.

## Invariants

Each names the failure it prevents.

1. **A topic group is never a conversation.** It is absent from
   `list_groups`, emits none of the `group_*` events, is never registered with
   the relay, and refuses `send_group_message` from the application. Its
   Welcome and its commits never travel as `__GRP_MLS_WELCOME__` or
   `__GRP_MLS_COMMIT__`, and the conversation Welcome path refuses a group
   whose context carries the topic mark before it caches the group or emits
   anything. Otherwise an application that renders groups renders every topic
   it watches, and the relay learns the subscriber set of every topic.
2. **Only the provider sends into a topic group, and only the provider
   commits.** A consumer accepts an update from the group only when its MLS
   sender is the provider its subscription names (ADR 0035 invariant 2), and
   rejects a commit from any other member. Topic groups are excluded from the
   epoch-fork check and from admin promotion on removal. Otherwise any
   subscriber can publish as the provider to every other subscriber, or the
   fork leader (often not the provider) commits into the group, or a provider
   that is removed hands the topic to a subscriber.
3. **A consumer joins a topic group only from a Welcome that names one of its
   own subscriptions to the sender.** The Welcome is a subscription frame
   sealed in the 1:1 session and carries the `sub_id`; the consumer joins only
   when it holds that `sub_id` for a subscription to the frame's
   MLS-authenticated sender, and after joining checks that the group context
   names the same provider, service and topic, leaving the group if it does
   not. It matches on the subscription and not on a group id announced in an
   earlier frame, because the reliable ladder gives no order between two
   outbox entries (ADR 0035, Order): a Welcome that outran its announcement
   would be dropped, leaving a committed add that nobody joined and nothing
   retries. Otherwise any device that has a session with you can add you to a
   group of its choosing.
4. **Catch-up stays in the 1:1 session.** Snapshots, replays and checkpoints
   are per subscriber (ADR 0035 recovery), so they go sealed to that
   subscriber alone; only live publishes go to the group. Otherwise a new
   subscriber's catch-up is sent to every member.
5. **Ending a subscription removes the member in the next commit.** That
   includes a subscription replaced under ADR 0035 invariant 7, whose leaf is
   removed in the same commit that adds the replacement. That is the property
   the feature exists for; a group-backed topic that kept ended subscribers
   as members until some later cleanup would be strictly worse than ADR 0035.
6. **The ratchet-gap budget of ADR 0032 applies to topic groups as it does to
   any group, and the rung is still a subscription frame.** A live publish
   that misses an absent member counts against the budget, and a spent budget
   sends the next one reliable. That reliable copy is marked as a
   subscription frame when it is queued, so `persist_outbox_entry` never
   writes it (ADR 0035 invariant 3). Otherwise a rung restored after a restart
   names a group and a stream the provider no longer has.
7. **A key filter is never widened by a group.** A subscription with a key
   filter stays on the 1:1 session. A group frame goes to every member, so
   filtering it on the consumer would send each filtered subscriber every key
   of the topic, which is what ADR 0035's provider-side filter exists to
   prevent.

## Decision (proposed)

- **Opt-in per topic, by the provider.** `set_service_topic_group(service_id,
  topic, true)` before or after subscribers exist. A consumer does not choose
  whether the provider offers a group, since how a provider fans out is its
  cost to bear; it may decline the offer only at its own cap (below).
- **One group per topic**, created on the first subscriber that sends a key
  package, carrying an MLS group-context extension that marks it a topic
  group and names the provider, service and topic. The mark is what
  invariants 1 and 3 key on, so it is in the authenticated group context, not
  in local metadata. The provider is the group's only admin and its only
  committer.
- **Getting a key package.** The provider cannot reuse the one the 1:1
  session spent, so it asks. An `__SVC_SUB_R__` that admits a subscription to
  a group-backed topic carries `group_offer: true`. The consumer answers at
  once with an `__SVC_SUB__` for the same `sub_id` carrying `key_package`: one
  it mints for this offer, never one from its pending list. The provider takes
  a `key_package` only on a frame for a subscription it holds with an offer
  outstanding, and validates it as `add_group_member` validates any package
  (credential identity, address binding, lifetime cap). This costs one round
  trip before the first commit; until the package arrives, and whenever an
  offer goes unanswered, the subscription runs over the 1:1 session (ADR 0035)
  and the next renewal's reply offers again.
- **Joining.** With the package the provider adds the subscriber, and sends
  the Welcome to it alone as `__SVC_GWEL__`: `{v, sub_id, service_id, topic,
  welcome}`, sealed in the 1:1 session like every subscription frame, on the
  reliable ladder, marked as a subscription frame (invariant 3). It never
  passes through the conversation Welcome handler, so no group event exists
  to suppress.
- **Commits.** A commit goes to each remaining member as `__SVC_GCOM__`:
  `{v, service_id, topic, commit}`, sealed in that member's 1:1 session on the
  reliable ladder and marked as a subscription frame. It is handled by the
  topic-group path, which rejects a committer other than the provider, and
  never by the conversation commit handler. A member's own removal reaches it
  as ADR 0035's `__SVC_UNSUB__`, not as a commit.
- **Updates.** A live publish is one `__SVC_GUPD__` frame in the group:
  `{v, service_id, topic, stream, seq, key?, body}`. There is no `sub_id`:
  the frame is for every member. A consumer maps it to its subscription by
  (provider, service, topic) and applies ADR 0035's ordering and
  superseding rules unchanged. A plaintext decrypted from a topic group that
  is not `__SVC_GUPD__` is dropped, and an `__SVC_` frame decrypted from a
  conversation group is dropped. A `latest` value missed by an absent member
  is recovered by the digest at the next renewal, as today.
- **Leaving.** `unsubscribe_service`, a lapsed lease, a replacement and
  `end_service_subscription` each queue the member's removal; removals and
  additions within one tick are one commit.
- **Restart, on both sides.** Neither side persists a subscription (ADR 0035
  invariant 3), but MLS group state is persisted. At start-up each side
  deletes every topic group it holds: the provider's has no subscriptions
  behind it, and the consumer's names subscriptions it no longer has.
  Renewals and fresh subscriptions rebuild the groups, which costs the
  provider one commit per subscriber per restart.
- **Pattern subscriptions** follow each matched topic's group separately.
  A consumer already in `MAX_TOPIC_GROUPS` (proposed: 16) topic groups
  answers a further offer with `group_declined: true` and no key package, and
  stays on the 1:1 session for that topic.
- **The provider can see who is revocable.** `SubscriberInfo` carries
  `in_group`, true only for a subscriber that is a member of the topic's
  group. A subscriber on the 1:1 session (no package yet, a key filter, or a
  declined offer) is revoked by promise, as in ADR 0035, and the provider can
  tell which kind it holds.
- **Wire.** `__SVC_GWEL__`, `__SVC_GCOM__` and `__SVC_GUPD__` join the
  reserved prefix registry (`INTERNAL_PREFIXES` and the control messages
  chapter); none is a prefix of another or of ADR 0035's four. `group_offer`,
  `group_declined` and `key_package` are new fields on existing frames, so
  they raise `v` under ADR 0035's version rule, and a consumer at an older `v`
  never answers an offer and stays on the 1:1 session. Routing the three
  frames is a new `svc_versions` entry (`SVC_SUB_GROUP_V1`); a provider offers
  a group only to a peer that advertises it.

## Open questions

These need an answer before the status can move.

- **Commit cost against churn.** A topic whose subscribers come and go every
  minute commits every minute, and each commit is a Welcome or a removal to
  deliver across the mesh. Is there a member count below which the group is
  not worth it, and should the engine choose rather than the application?
  Commits interact with ADR 0032 in the provider's favour: each one starts a
  new epoch, so a churning topic rarely spends its ratchet budget, while a
  stable topic at 10 Hz with one member out of reach sends a rung about every
  25 seconds.
- **Membership is visible to members.** Every subscriber of a group-backed
  topic can read the roster, so it learns who else watches the provider.
  ADR 0035 never shows a subscriber anything about other subscribers. That
  may be acceptable for a fleet and not for a medical sensor; it needs an
  amendment to the threat model's R32, and possibly a per-topic warning in
  the API.
- **Members never update their own keys.** Only the provider commits, so a
  subscriber's leaf keeps the key it joined with until it is removed and
  added again. Whether a topic needs periodic re-adds for post-compromise
  security of members is open.
- **Relay fan-out.** Invariant 1 keeps topic groups off the relay, which also
  gives up the relay's one-frame broadcast for internet-connected members.
  It keeps the roster off the relay, not the traffic pattern: rungs, catch-up
  and the per-member Welcome and commit frames still reach a member's relay
  mailbox addressed to that member, which R32 already describes.

## Consequences

- One encryption per publish for a group-backed topic, whatever the number of
  subscribers, and revocation that holds against a subscriber that keeps its
  copy of the code, for every subscriber whose `in_group` is true.
- A key package round trip before a subscriber joins, a commit per membership
  change, and roster visibility, none of which ADR 0035 had.
- Three new frames and a new capability entry. The topic group's Welcome and
  commits ride the sealed 1:1 path rather than the group frames, which costs
  one sealed frame per member per commit and buys invariants 1 and 3 by
  construction: the conversation handlers never see a topic group.
- `group_mesh.rs` learns a second kind of group. That is the bulk of the work
  and the bulk of the risk: every path that lists, announces, relays,
  accepts, fork-checks or promotes in a group needs the topic-group check of
  invariants 1 and 2, and a path that misses it leaks.

## What would undo this

- **Letting the application see or send into a topic group** "for
  debugging". Invariant 1 is what keeps the subscriber set off the relay and
  out of the user's group list.
- **Sending the topic Welcome or commit as a group frame** to save the 1:1
  seal. The conversation handlers would join and announce it, and the
  Welcome would lose the `sub_id` that invariant 3 matches on.
- **Gating the Welcome on a group id from the reply.** The reply and the
  Welcome are not ordered, and a dropped Welcome is an add nobody joins.
- **Reusing a key package from the pending list** to skip the round trip.
  The 1:1 session spent it.
- **Persisting the rung with the rest of the outbox.** It names a group the
  provider deletes at start-up.
- **Sending catch-up to the group.** It is per subscriber by construction.
- **Putting filtered subscribers in the group** and filtering on the
  consumer. The keys the filter withheld would leave the provider.
- **Keeping removals for later.** Revocation that waits is not revocation.
