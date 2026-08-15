# 0014. Security-relevant relay answers arrive through dedicated entry points

**Status:** Accepted

## Context

Relay answers reach the core by the bridge synthesizing a message frame with a
reserved prefix and injecting it into the ordinary receive path.

That is convenient: one injection function serves every relay answer, and the
core needs no new surface per answer type.

It is also a trust hole. The generic injector is reachable by anything that can
call the bridge, including push-notification handling. An answer injected that
way is indistinguishable from one the relay actually sent, and the six
relay-answer prefixes are already exempt from the signature gate because they
have no signer.

The group delivery report made the cost concrete. It drives **re-sending** to
members the relay says it missed. A forged report can suppress the re-issue
entirely, or drive an arbitrary fan-out.

## Decision

Anything whose contents drive a security-relevant or delivery-relevant decision
gets a **dedicated entry point**, not message-plane injection.

The group delivery report is the reference implementation of this: it arrives
through its own function, so the notification injector cannot forge it. Bridges
still pass the raw frame through as an opaque server message for observability,
but that path drives nothing.

## Consequences

**Good.** The report is unforgeable by anything that can reach the generic
injector.

**Good.** The entry point has a typed signature, so the bridge cannot pass a
malformed shape that the core then parses defensively.

**Cost.** A new function on the FFI surface per such answer, mirrored across four
bindings. That is the real reason the generic injector existed, and it is a cost
worth paying only for answers that drive decisions.

**Cost.** Two paths now exist for the same relay answer, and the difference
between "drives a decision" and "is observability" has to be maintained
deliberately.

## The rule for new answers

Ask: **if an attacker could forge this, what would happen?**

| Answer | Forged consequence | Path |
|--------|-------------------|------|
| Group delivery report | Suppressed re-issue, or arbitrary fan-out | **Dedicated entry point** |
| Group registration confirmation | Flips the sync gate broadcast rides on, but only inside the outstanding-registration window | Message plane, and this is residual risk R1 |
| Group info, member lists | Corrupts the members cache | Message plane |
| Relay error report | Surfaces a false error | Message plane |

The second row is the honest answer about where the boundary currently sits.
Registration confirmation is a decision-driving answer on the message plane, and
moving it is the follow-up that would close residual risk R1 in the
[threat model](../security/threat-model.md#r1-relay-answer-forgery).

## Related constraint

The relay capability set must be injected **before** the internet-available
transition, so the flush that transition triggers already sees it, and cleared
when internet drops. Ordering constraints of that kind are invisible in the
bridge's own tests and belong in the [bridge contract](../bridges/README.md).
