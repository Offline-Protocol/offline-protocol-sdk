# 0006. The desync classification gates the re-key, not the acknowledgement

**Status:** Accepted

## Context

Two decrypt failures look similar and are not:

- **Epoch disagreement.** The two sides of an established session disagree about
  the MLS epoch. Recoverable, but only by rebuilding the session.
- **AEAD or ratchet failure.** The ciphertext is corrupt, or the generation was
  already spent. Not recoverable by rebuilding anything.

The first was given a dedicated recoverable class, and that class was
**also** used to decide whether to withhold the acknowledgement. That coupling
was the bug.

## Decision

Separate the two questions.

| Question | Answer |
|----------|--------|
| Withhold the acknowledgement? | **Yes for every recoverable class**, desync and ordinary crypto failure alike |
| Schedule a re-key? | **Only for desync** |

## Consequences

**Good.** Ordinary crypto failures were already recoverable, because sender-side
re-sealing regenerates the ciphertext on every resend (see
[ADR 0007](0007-reseal-on-resend.md)). Drawing the no-acknowledgement boundary at
desync alone meant the receiver kept acknowledging them as delivered while the
recovery mechanism sat unused.

**Good.** Keeping the re-key narrow avoids a re-key-storm vector. Re-keying on
AEAD or corruption failures means anyone who can corrupt a frame can force a
session teardown.

**Cost.** Two separate decisions where a naive reading expects one, and the code
must keep them separate at every site.

## The classification boundary in detail

| Failure | Ack | Enqueue | Re-key |
|---------|-----|---------|--------|
| Session not ready | no | **yes** | no |
| Epoch desync | no | no | **yes** |
| AEAD / corrupt / spent generation | no | no | no |
| Transport failure | no | no | no |
| Envelope parse failure | no | no | no |
| Policy refusal that can never become decryptable | **yes** | no | no |
| Security refusal (identity mismatch, foreign session slot) | **no**, identifier unmarked | no | no |
| Failure after a successful decrypt | **yes** | no | no |

The two refusal rows are opposite on purpose, and the security row is
intercepted before this classification runs so it cannot inherit the policy
row's acknowledgement. See
[ADR 0005](0005-defer-instead-of-drop-and-ack.md).

**Four rows describe the table with crypto recovery enabled**: epoch desync,
AEAD / corrupt / spent generation, transport failure, and envelope parse
failure. Enabled is the default; it is off only when an application sets it off
explicitly, or when encryption is disabled outright. With it disabled, those
four collapse into drop-and-acknowledge with no re-key: the receiver treats an
undecryptable frame as permanently undeliverable rather than paying for the
heal.

The other four rows do not move with the switch, and for two of them that is a
rule rather than an accident. The security refusal must never become gated:
turning off a recovery optimization must not turn off an interception whose
purpose is to withhold a liveness signal from an attacker. Session-not-ready
must not either, because it is the deferred atom's own fix rather than a
recovery heuristic, and acknowledging a frame that is known to become
deliverable once the session arrives is exactly the silent loss
[ADR 0005](0005-defer-instead-of-drop-and-ack.md) closes. The remaining two, the
policy refusal and the post-decrypt failure, already acknowledge in both
configurations, so there is nothing for the switch to change.

## A subtlety worth pinning in a test

**Malformed** input never reaches MLS framing validation and correctly stays out
of the desync class.

A **well-formed** frame carrying a forged epoch **does** classify as desync. That
is not a classification bug; it is the unauthenticated trigger described in
[ADR 0004](0004-control-plane-signature-gate.md)'s data-plane exemption and in
the threat model. A test asserting "corrupt ciphertext is not desync" covers only
the malformed case and must not be read as covering the forged one.

## What would undo this

Broadening the re-key trigger to cover ordinary decryption failure, on the
reasoning that "a failed decrypt might mean a fork". It might, and acting on that
guess hands an attacker a teardown per corrupted frame.

Narrowing the withheld acknowledgement back to desync only, on the reasoning
that "the others are not recoverable". They are, by re-sealing.
