# 0032. Ephemeral sends go once, over a live link

**Status:** Accepted. Since 0.31.0 a group member acknowledges and drops a
frame too far behind its ratchet to decrypt, so a late rung settles as
delivered instead of being retried until it fails; the paragraphs below say
where that changes the consequences.

## Context

Every message the engine sends is acknowledged and retried: an outbox entry
written to storage, an acknowledgement registration, a retry on failure, a
hand-off to neighbours when the recipient is out of range, a relay mailbox
when the internet is up. That is right for chat. It is wrong for state that
the next send replaces, which is most of what a game sends: a paddle position
retried after the next one already arrived is noise, and one delivered from a
relay mailbox an hour later is a bug.

It is also expensive where storage is slow. The browser node writes every
record through an OPFS flush, and a reliable group message to one member cost
four flushes on the sender: about forty a second at 10 Hz, measured by the
browser's WebRTC page during the 0.31 work. The core message already carries
`requires_ack`, and the send ladder already skips the outbox and the
acknowledgement registration when it is false; only the retry on failure did
not, and nothing public set it.

## Decision

`SendMessageOptions::ephemeral` and `GroupSendOptions::ephemeral`. An
ephemeral send:

- **goes once, over a live direct link.** The frame carries
  `requires_ack = false`, so the receiver acknowledges nothing, and the sender
  keeps no outbox entry, acknowledgement, retry, park or mesh hand-off. Only a
  carrier with a live link straight to the recipient takes it. The relay and
  Reticulum accept a frame for anyone and keep it, which is the delivery an
  ephemeral update must never get. With no link the call fails at once with
  `PeerNotReachable`, before anything is encrypted.
- **needs a confirmed session.** A direct message is sealed strictly and fails
  with `SessionNotReady` rather than queueing behind the handshake.
- **carries plain content.** Reply threading, rich extras, a content type and
  a forced transport are refused with `InvalidArgument`: each promises how the
  message is rendered or routed later.
- **emits `MessageSent` and nothing after it.** Telemetry counts only sends
  that ask for an acknowledgement into the delivery ratio's denominator, so a
  game does not read as a failing network.

A group send encrypts once and hands the frame to each member that holds a
live link. A member without one misses it, and that is the one hazard: the
frame still advanced this device's sender ratchet for every member, and
OpenMLS refuses a generation more than 8192 past the last one a receiver saw.
A player who stepped out of range for a fifteen-minute match at 10 Hz would
come back unable to read anything this device sends, chat included, until a
commit. So a group send that missed anyone counts against the ratchet-gap
budget the directed replication frames already use (256 generations per
epoch per process, `MAX_ROSTER_INVISIBLE_GROUP_GENERATIONS`), and when the
budget is spent that send goes out reliable instead. Its copy (the rung)
waits in every absent member's outbox and decrypts across the gap, and the
count starts again.

The rung must reach the member before anything newer does. OpenMLS keeps
only 32 generations behind the newest one a receiver has seen
(`SENDER_RATCHET_OUT_OF_ORDER_TOLERANCE`), and a member coming back into
range at 10 Hz is handed that many fresh frames in about three seconds. A
rung that arrives after them can never be decrypted: before 0.31.0 it was
never acknowledged and the sender retried it until it failed, and since then
the member acknowledges and drops it, so it settles without ever being read.
Either way the member never reads it. So ephemeral sends
skip a member whose rung is still in the outbox, and the outbox flush on
that member's reachability edge sends the rung first. Once it is
acknowledged (or has expired or failed), the member takes ephemeral frames
again. A member that held a live link when the rung was sent is never
held: the rung went down that link ahead of any later frame. (When the link
refused it, that is not true; see Consequences.) As with replication, the
count is not persisted, so the first ephemeral send to a group in a process
is reliable.

Measured on the host over the browser node's sealed log store, one member:

| Send | Flushes per group message |
|------|---------------------------|
| Reliable | 4 |
| Ephemeral | 2 |

The two left are the ratchet itself and the delivery bookkeeping a group
encryption writes; neither can be skipped without losing the property that a
restart never reuses a generation.

## Consequences

- An ephemeral update lost in flight is lost. The game's next update is the
  recovery.
- The budget cannot see frames a carrier accepted and then lost when its link
  dropped. Those widen a member's gap past the count. The distance between
  the budget (256) and the limit (8192) is what absorbs them: about 7900
  frames. What bounds them is how long a dead link takes to close, times the
  send rate, not a byte count: the browser's WebRTC carrier closes a link
  past 8 MiB of backlog, but a game's frames are small enough that 8 MiB
  holds far more than 7900 of them. A WebRTC link whose peer stops answering
  closes when ICE consent expires, about 30 seconds (RFC 7675), so the
  margin holds for a game sending fewer than about 260 frames a second.
- A member waiting for a rung misses every ephemeral frame until it is
  acknowledged. That is normally one round trip after the link comes back,
  as the outbox flush sends the rung on that edge. A rung that travels by
  relay broadcast is not held for, because the relay's copy is not in the
  outbox; the browser node has no relay, and a native binding that exposes
  ephemeral sends has to revisit this.
- A member that held a live link when the rung was sent is not held even
  when that link refused the rung. The hold is decided by who was linked,
  not by what each carrier accepted, so a refused rung sits in the outbox
  while the next ephemeral frames go down the same link. If more than 32 of
  them arrive before the rung is retried, it can never be decrypted; the
  member acknowledges and drops it, so the application sees the id it got
  back from `sendGroupMessage` delivered although the member never read it
  (before 0.31.0 it was retried until `MessageFailed`). The member's ratchet is
  still safe, because the rung is the only thing that gap needed. A carrier
  that refuses on a live link is usually closing it, but that does not hold
  the member either: the hold records only members that were unlinked
  before the rung was sent, so this member is skipped, and when its link
  comes back nothing keeps the next ephemeral frames behind the rung the
  flush is resending. It is narrow because rungs are rare: the count resets
  on every send that reaches every member, so a rung goes out only after
  256 sends in a row that missed somebody (at 10 Hz, about every 25 seconds
  while one member stays out of reach), plus the first send in a process.
  Holding linked members at each rung would cost a game one round trip of
  `PeerNotReachable` whenever a member keeps missing frames, which a
  two-player game sees as its only peer dropping out. The fix, if it shows,
  is to hold a member only when its carrier refused the rung, which needs
  the send path to report that per member.
- A rung that can never be decrypted settles as delivered. A held member's
  rung waits in the outbox, and the hold ends only when the rung leaves it.
  If the member received more than 32 newer frames first (the refused-rung
  case above), the frame is too far in the past to decrypt, and since 0.31.0
  the member acknowledges and drops it, which ends the hold. Before, the
  receiver answered nothing, the sender retried it to the end of the retry
  ladder, and the application saw `MessageFailed` for it while that member
  got no ephemeral frames. A frame from a device outside the group is still
  left unanswered.
- Only the browser binding exposes it. The FFI record does not have the field
  yet, so the UDL, the generated bindings and the C dispatcher are unchanged.
  Adding it is an append to `SendMessageOptions` in the UDL and every binding
  regenerated together.

## What would undo this

A message that must arrive belongs on the reliable path. If ephemeral sends
start being used for things the receiver must not miss, the answer is to send
those reliably, not to add a retry to this path: a retried ephemeral send is a
reliable send without its outbox, which loses its messages on a restart.
