# 0032. Ephemeral sends go once, over a live link

**Status:** Accepted

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
budget is spent that send goes out reliable instead. Its copy waits in every
absent member's outbox and decrypts across the gap, and the count starts
again. As with replication, the count is not persisted, so the first
ephemeral send to a group in a process is reliable.

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
  the budget (256) and the limit (8192) is what absorbs them, which holds for
  any carrier that buffers less than about 7900 frames; the browser's WebRTC
  carrier closes a link past 8 MiB of backlog.
- Only the browser binding exposes it. The FFI record does not have the field
  yet, so the UDL, the generated bindings and the C dispatcher are unchanged.
  Adding it is an append to `SendMessageOptions` in the UDL and every binding
  regenerated together.

## What would undo this

A message that must arrive belongs on the reliable path. If ephemeral sends
start being used for things the receiver must not miss, the answer is to send
those reliably, not to add a retry to this path: a retried ephemeral send is a
reliable send without its outbox, which loses its messages on a restart.
