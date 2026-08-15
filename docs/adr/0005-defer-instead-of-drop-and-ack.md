# 0005. A receiver that cannot deliver withholds the acknowledgement

**Status:** Accepted
**Shipped in:** 0.20.1 (extended to every recoverable failure class)

## Context

An encrypted message arriving before the receiver's session or group state
exists was queued for later decryption **and acknowledged as delivered**. The
sender then dropped its outbox entry and retired the retry ladder. If the queued
copy never drained, the message was gone, with both sides believing it
delivered.

The same shape appeared in five further places: epoch desync, ordinary crypto
failure, transport failure, envelope parse failure, and a queued frame that hard
failed on drain.

## Decision

**Acknowledge only what is delivered or permanently refused.**

A receiver that cannot deliver a frame **now**, but where some future event
could make it deliverable, withholds the acknowledgement and unmarks the
identifier so the sender's resend re-enters processing rather than hitting the
duplicate re-acknowledge path.

Terminal failures keep the acknowledgement: **policy** refusals that can never
become decryptable, such as an unauthorized commit, and failures **after** a
successful decrypt where the ratchet generation is spent and a re-seal would
produce the same malformed plaintext.

**Security refusals are the exception, and they are silent.** A frame refused
because its identity does not bind (sender identity mismatch, session identity
mismatch, leaf address mismatch, unsupported sender) or because it names another
pair's session slot is permanent in exactly the same sense, but it gets **no
acknowledgement and its identifier is unmarked**.

That exception is load-bearing rather than incidental. Those shapes would
otherwise classify as ordinary unknown-session failures and inherit this
decision's drop-and-acknowledge disposition, which would hand an injector a
confirmation that the target is live and processing. They are therefore
intercepted **before** classification, on both the text and media paths, and the
interception is deliberately not gated on the crypto-recovery switch: it is
about what the receiver reveals, not about recovery.

## Consequences

**Good.** Custody stays with the sender until a receiver positively confirms.
Silent loss becomes delayed delivery.

**Cost, and application teams must know it.** A sender that exhausts its retry
budget before both the session confirms and an acknowledgement lands may mark a
message undeliverable **though it was delivered locally**. That is strictly
better than the old silent drop, but it means **a missing acknowledgement is not
proof of non-delivery.**

**Cost.** Decryption-failure events now fire per failed **attempt** rather than
per message, bounded by the sender's retry budget. They are advisory; the
terminal signals are the failure events.

## The six pieces are correct only together

Implementing a subset produces a system that looks like it works and loses
messages:

1. A distinct not-ready outcome, so the receive loop has something to branch on.
2. Idempotent enqueue keyed by message identifier, so resends do not stack and
   the time-to-live measures from **first** receipt.
3. Drain on **any successful decrypt**, not on a session-established event. In
   the both-create race the **owner** keeps its local session and never *adopts*
   the Welcome it receives, confirming only once a decrypt succeeds, so a
   Welcome-triggered drain silently skips it.
4. Re-mark the identifier when the drain surfaces the message.
5. A time-to-live long enough to cover mesh session establishment (30 minutes,
   not 2).
6. Acknowledge on drain, on the transport the frame arrived on.

## Two rules the drain path adds

**Do not enqueue a frame that can never become processable.** A desynced
ciphertext is sealed to a dead epoch; a spent generation cannot be re-spent; an
unparseable envelope stays unparseable. Queuing them re-reports failures on
every drain forever.

**A queued frame that hard-fails on drain is dropped, not re-enqueued.** The
drain removes the entry before processing, so a re-enqueue misses the idempotency
check and re-stamps the receipt time, restarting the time-to-live of a frame that
can never decrypt, on every drain.

## What would undo this

Adding an acknowledgement to any deferred arm "so the sender stops retrying".
The retrying is the recovery mechanism.

Removing the security-refusal interception, on the reasoning that those shapes
are permanent refusals and permanent refusals are acknowledged. They are, and
that is exactly why deleting the interception is silent: the frames keep being
refused, the refusal keeps being correct, and the only thing that changes is
that an injector now gets an answer.
