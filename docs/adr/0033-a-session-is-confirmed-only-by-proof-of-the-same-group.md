# 0033. A session is confirmed only by proof of the same group

**Status:** Accepted
**See also:** [ADR 0006](0006-desync-classification-gates-rekey.md), [Session lifecycle](../state-machines/session-lifecycle.md#split-sessions)

## Context

A 1:1 session is an MLS group named by its slot, and two sides can hold two
different groups for one slot. Both creating at once is the common way in:
the tiebreak says the greater address adopts the other's Welcome, and when
the lesser side's Welcome is lost, the greater side keeps a group the other
side never joined.

Nothing noticed. A 1:1 session never commits after it is created, so both
groups sit at the same epoch, every frame between them passes MLS framing
validation, and each fails as an ordinary decryption failure. ADR 0006 keeps
the re-key off that class on purpose, because anyone who can corrupt a frame
reaches it. The epoch check was the only thing that could see a split, and a
split is invisible to it.

What made it worse was confirmation. A plaintext probe or acknowledgement
said only "I hold a session", and the adopter confirmed its own group on the
owner's probe and flushed its queue under it. The owner already refused to
confirm that way, but only because it knew it had raced; the adopter, whose
copy of the race was the lost frame, did not.

## Decision

A probe and an acknowledgement carry an MLS exporter tag over a nonce, which
only a member of the same group at the same epoch computes. A probe's nonce is
a challenge, repeated by every probe until it is answered; an acknowledgement
echoes it. A match is group-aware proof, as good as a decrypt.

A mismatch at the same epoch is a split, stated under the peer's identity
signature. Only an acknowledgement answering a challenge issued under the
current session is acted on, and only by the greater side, which gives up its
own group with a `no_session` key package (below), so the peer resends its
Welcome and keeps its group. Any other mismatching frame may be stale or
replayed, and only prompts a challenge. An empty body is an older peer, held
to the plaintext rules.

Confirmation probes stop once a session confirms, so an ordinary decrypt
failure on a 1:1 session sends one too, at most once per peer per re-key
interval. That finds a split between two sides that both confirmed. The
failure only asks; the tag in the answer decides.

The other split is one side holding a session and the other none, which a
lost reset leaves. A key package sent while the sender holds no session
carries `no_session`. The holder resends its Welcome when that Welcome names
the package, since the sender never processed it, and otherwise drops its
session and rebuilds. Repeating the reset flag instead would tear down a
replacement whose Welcome is in flight; the package comparison is what tells
that case apart. The teardown is refused when the flag is unstamped or older
than the frame that built the session, and a Welcome given up in the
both-create tiebreak is discarded, so neither a stale flag nor a dead Welcome
reaches the comparison.

## Consequences

**Good.** The split cannot form through confirmation, and one that exists is
healed by the first probe or acknowledgement in either direction. The owner
can confirm from a probe, so a passive owner no longer waits on a message to
decrypt.

**Good.** ADR 0006 holds. Nothing destructive hangs off an ordinary decrypt
failure; the trigger is a signed statement, not a corrupted frame.

**Cost.** One exporter call per probe and acknowledgement, and about 160
bytes on each.

**Cost.** A split found by a probe heals one round trip later than it would
if probes were acted on, because the greater side first issues its own
challenge.

**Residual.** None from stale or replayed corroborations: only a fresh
answer to a live challenge drives a heal, and a successful decrypt retires
the challenge, so an answer the peer sealed under a group it has since left
finds nothing live. The mark is clamped to five minutes past this device's
clock, so a peer whose clock was set back is locked out of resetting for at
most that long, and on a clock of ours more than five minutes slow a
recording made just before the session is refused only as a reset always
was. A `no_session` teardown is bounded
like a reset, by the spent mark, which the frame that built the session now
moves. A session the send path builds later from a cached package records no
stamp, so a recording older than it is bounded only as a reset always was.
With freshness enforcement switched off no frame carries a checked stamp, so
a `no_session` teardown is never honoured; a split whose lesser side cannot
resend a Welcome naming the offered package then churns at the re-key floor
instead of healing, as it would have stayed split before this decision.

## What would undo this

Confirming on a probe whose tag cannot be compared, on the reasoning that
the peer clearly holds a session. It does, and possibly a different one.

Repeating `session_reset` on a re-push instead of `no_session`, on the
reasoning that a reset already means "drop yours". It does, unconditionally,
including the replacement session a Welcome is still carrying.

Healing on a mismatching probe, or on any acknowledgement, on the reasoning
that the frame is signed. It is, and it may be from before the last heal.

Healing with `session_reset` because the heal is a re-key. The peer then
rebuilds on the package its own delayed Welcome was built on, and the
Welcome splits the pair again when it lands.

Letting both sides heal a mismatch. Both would tear down at once, each would
rebuild from the other's package, and the pair would race again.
