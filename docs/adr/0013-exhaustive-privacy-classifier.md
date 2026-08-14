# 0013. Remote-influenced text is classified to a fixed vocabulary by an exhaustive match

**Status:** Accepted

## Context

Telemetry events carry some free-text fields. The scrubber hashes identifiers it
knows about, but it cannot know that a free-text `reason` contains one.

A series of leaks followed the same shape: an event field rendered an error, the
error interpolated wire input, and the wire input was an identifier belonging to
a **third party**.

The worst example: a session-Welcome refusal rendered a session slot, which is
two addresses, one of them possibly a third party's, plus a group identifier the
sender chose, taken from raw bytes and bounded by neither charset validation nor
a length cap.

The first fix scoped the substitution to the identity refusals, on the premise
that every other join failure was a fault rather than an accusation and named
nobody. **That premise was false**, and sibling arms at the same two sites leaked
the same things. A wider audit found the class again in relay-answer-fed error
reasons, in control-gate warnings, and in transport send-failure text.

The exceptions kept turning out not to be exceptions.

## Decision

Invert the default. Instead of sanitizing sites known to leak, make leaking
unrepresentable.

**The producer rule:** an event field never carries text chosen by a remote
party, nor a rendered error that interpolates one. Not shortened, not sanitized
in place. **Classified**, to a fixed local vocabulary.

Two structural habits enforce it:

1. **The classifier returns a static string type**, so interpolating wire input
   is unrepresentable rather than merely discouraged. Push that type into the
   event constructor, so a producer cannot hand it a rendering.
2. **The classifier's match is exhaustive in the crate that defines the error
   type.** A newly added variant then fails to compile **there**, forcing the
   privacy decision to be made where variants are written.

Keep the remote wording, bounded, in a device log if it is worth keeping.

When the dropped prose carried real structure, add it back as a **typed field**
the scrubber can hash, not as prose.

## Consequences

**Good.** The decision point moves from "the engineer adding an event site
remembers" to "the engineer adding an error variant cannot compile without
deciding".

**Cost.** Event text is less specific. A support engineer reading telemetry gets
a class, not a detail, and must reach for the device log for the rest. That is
the intended trade.

**Cost.** Three classifiers to keep in step, one per error family.

## Never add a catch-all arm

A catch-all restores the per-site opt-in this replaced, and every leak in this
class was a per-site omission. The exhaustiveness **is** the mechanism; without
it the return type alone only stops interpolation, not omission.

Exhaustive matching on a non-exhaustive type is legal within its defining crate,
which is exactly why the classifier lives there.

## Testing note

A test asserting "this variant classifies without its payload" is **vacuous** if
the variant never renders its payload in the first place. Pair it with a premise
guard that asserts the variant really does render the identifier, or the
assertion passes with the classifier deleted.

## What would undo this

Adding a `reason` string parameter to a new event constructor because it is
convenient. Adding a catch-all to a classifier to make a new variant compile.
