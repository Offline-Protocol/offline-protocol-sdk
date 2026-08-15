# State machines

Five state machines govern the protocol's runtime behaviour. Each is documented
with its states, its transitions, the invariants that hold across them, and the
failure modes that the current shape exists to prevent.

| Document | Governs |
|----------|---------|
| [Delivery and acknowledgements](delivery-and-acks.md) | What happens to an inbound frame, and when a receiver acknowledges |
| [Outbox and retries](outbox-and-retries.md) | What happens to an outbound message from send to terminal state |
| [Session lifecycle](session-lifecycle.md) | 1:1 MLS session establishment, confirmation, desync, and heal |
| [Group message lifecycle](group-message-lifecycle.md) | A group message from send through fan-out, buffering, and drain |
| [Transport lifecycle](transport-lifecycle.md) | Transport availability, scoring, switching, and escalation |

## How to read these

Each document states its invariants first. The invariants are the part that
survives refactoring; the state names are not.

Where a transition exists to prevent a specific failure, the failure is named.
Several of these shapes look over-engineered until you know which bug they close,
and the surrounding prose is there so a future change does not undo one by
accident.

## The one invariant that spans all five

**Custody of an undelivered message stays with the sender until a receiver
positively confirms it.**

Every acknowledgement decision, every buffering decision, and every deduplication
decision in these documents is downstream of that. When a receiver cannot yet
deliver a frame, the correct move is to withhold the acknowledgement and let the
sender keep custody, not to acknowledge and hope.

The corollary matters to application teams: because acknowledgements are
withheld on recoverable failures and re-sent on recovery, **a missing
acknowledgement is not proof of non-delivery.**
