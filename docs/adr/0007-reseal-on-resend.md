# 0007. Resends are re-sealed against the current session, never replayed

**Status:** Accepted

## Context

An outbox entry holds a message awaiting acknowledgement. The obvious
implementation stores the serialized frame and retransmits those bytes.

That is wrong for encrypted traffic. Ciphertext is sealed to an MLS epoch. If
the recipient's session is rebuilt, for any reason including a legitimate heal,
the stored bytes are sealed to a dead epoch and every retransmission fails
identically, forever, until the retry budget is exhausted.

The receiver-side heal alone therefore makes a fork **detectable**, not
**recoverable**.

## Decision

Re-seal each resend against the recipient's **current** session state,
preserving the message identifier so deduplication and acknowledgement matching
still work.

Re-sealing is gated on the session being confirmed. Re-sealing against an
unconfirmed session produces ciphertext the peer cannot open either.

## Consequences

**Good.** A session fork becomes recoverable without message loss. This is the
half that makes the receiver-side heal worth having.

**Cost, and it constrains the implementation.** Re-seal provenance holds
**plaintext**. Two rules follow:

1. It is memory-only and never persisted.
2. Staging is strictly transient: taking a staged re-seal always removes it, and
   removing an outbox entry clears any staged re-seal, so a staged-but-dropped
   send never strands plaintext.

**Cost.** Media has no equivalent. Chunks are re-encoded rather than replayed,
so media recovers through a descriptor-based resend request instead. That
asymmetry is permanent and is why media and text differ in the recovery tables.

## What would undo this

Persisting the re-seal provenance "so resends survive a restart". It holds
plaintext, and that is the whole reason it is memory-only. The cost is real and
should be stated rather than argued away: a restored entry carries no
provenance, so it replays verbatim, and a fork that spans a sender restart
settles as an honest failure instead of being re-sealed. That is the price of
not writing plaintext to disk, not an absence of consequences.

Storing the sealed bytes in the outbox and reusing them, which is the default
shape of every retry queue and is the thing this ADR exists to prevent.
