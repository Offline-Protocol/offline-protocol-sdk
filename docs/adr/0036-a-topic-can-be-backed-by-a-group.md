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
- `invite_to_group` needs the invitee's key package in hand, sends a Welcome,
  and commits to every member.
- Every group send spends ratchet generations for absent members, and ADR
  0032 bounds that with a budget of 256 per epoch per process.

A topic group that took any of these paths as they are would show up in the
user's group list, tell the relay which devices watch which provider, and
spend a commit and a Welcome on every subscriber that joins or leaves.

## Invariants

Each names the failure it prevents.

1. **A topic group is never a conversation.** It is absent from
   `list_groups`, emits none of the `group_*` events, is never registered with
   the relay, and refuses `send_group_message` from the application. Otherwise
   an application that renders groups renders every topic it watches, and the
   relay learns the subscriber set of every topic.
2. **Only the provider sends into a topic group, and only the provider
   commits.** A consumer accepts an update from the group only when its MLS
   sender is the provider its subscription names (ADR 0035 invariant 2). A
   member that sends is ignored, not relayed. Otherwise any subscriber can
   publish as the provider to every other subscriber.
3. **A consumer joins a topic group only from a Welcome its own subscription
   announced.** The provider's `__SVC_SUB_R__` names the group id; a Welcome
   for a group id no active subscription of that provider announced is
   dropped. Otherwise any device that has a session with you can add you to a
   group of its choosing.
4. **Catch-up stays in the 1:1 session.** Snapshots, replays and checkpoints
   are per subscriber (ADR 0035 recovery), so they go sealed to that
   subscriber alone; only live publishes go to the group. Otherwise a new
   subscriber's catch-up is sent to every member.
5. **Ending a subscription removes the member in the next commit.** That is
   the property the feature exists for; a group-backed topic that kept ended
   subscribers as members until some later cleanup would be strictly worse
   than ADR 0035.
6. **The ratchet-gap budget of ADR 0032 applies to topic groups as it does to
   any group.** A live publish that misses an absent member counts against it,
   and a spent budget sends the next one reliable.

## Decision (proposed)

- **Opt-in per topic, by the provider.** `set_service_topic_group(service_id,
  topic, true)` before or after subscribers exist. A consumer does not choose:
  how a provider fans out is its cost to bear.
- **One group per topic**, created on the first admitted subscriber, carrying
  an MLS group-context extension that marks it a topic group and names the
  provider, service and topic. The mark is what invariant 1 keys on, so it is
  in the authenticated group context, not in local metadata.
- **Joining.** On admission the provider adds the subscriber with the key
  package the 1:1 session exchange already left in `pending_key_packages`, and
  answers `__SVC_SUB_R__` with `group_id`. With no key package in hand the
  subscription runs over the 1:1 session (ADR 0035) and is moved to the group
  at the next renewal that finds one.
- **Updates.** A live publish is one `__SVC_GUPD__` frame in the group:
  `{v, service_id, topic, stream, seq, key?, body}`. There is no `sub_id`:
  the frame is for every member. A consumer maps it to its subscription by
  (provider, service, topic) and applies ADR 0035's ordering and
  superseding rules unchanged. A `latest` value missed by an absent member is
  recovered by the digest at the next renewal, as today.
- **Leaving.** `unsubscribe_service`, a lapsed lease and
  `end_service_subscription` each queue the member's removal; removals and
  additions within one tick are one commit.
- **Pattern subscriptions** follow each matched topic's group separately.
  A consumer in more groups than `MAX_TOPIC_GROUPS` (proposed: 16) stays on
  the 1:1 session for the rest.

## Open questions

These need an answer before the status can move.

- **Commit cost against churn.** A topic whose subscribers come and go every
  minute commits every minute, and each commit is a Welcome or a removal to
  deliver across the mesh. Is there a member count below which the group is
  not worth it, and should the engine choose rather than the application?
- **Membership is visible to members.** Every subscriber of a group-backed
  topic can read the roster, so it learns who else watches the provider.
  ADR 0035 never shows a subscriber anything about other subscribers. That
  may be acceptable for a fleet and not for a medical sensor; it needs the
  threat model's view, and possibly a per-topic warning in the API.
- **Persistence.** MLS group state is persisted, and ADR 0035 persists no
  subscription. A provider that restarts holds a topic group with no
  subscriptions behind it; the proposal is to tear it down at start-up and
  let renewals rebuild it, which costs one commit per subscriber per restart.
- **Relay fan-out.** Invariant 1 keeps topic groups off the relay, which also
  gives up the relay's one-frame broadcast for internet-connected members.

## Consequences

- One encryption per publish for a group-backed topic, whatever the number of
  subscribers, and revocation that holds against a subscriber that keeps its
  copy of the code.
- A commit per membership change, and roster visibility, which ADR 0035 never
  had.
- `group_mesh.rs` learns a second kind of group. That is the bulk of the work
  and the bulk of the risk: every path that lists, announces, relays or
  accepts a group needs the topic-group check of invariant 1, and a path that
  misses it leaks.

## What would undo this

- **Letting the application see or send into a topic group** "for
  debugging". Invariant 1 is what keeps the subscriber set off the relay and
  out of the user's group list.
- **Sending catch-up to the group.** It is per subscriber by construction.
- **Keeping removals for later.** Revocation that waits is not revocation.
