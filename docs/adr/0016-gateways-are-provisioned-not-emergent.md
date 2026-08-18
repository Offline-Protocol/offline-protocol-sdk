# 0016. Gateways are provisioned, never emergent

**Status:** Accepted

## Context

A gateway bridges a zone to the internet or to a wide-area backbone. The
question is how a zone acquires one: does a person install it, or does a capable
device promote itself when it notices it could bridge?

Self-promotion is the obvious design, and this codebase already does something
that looks like it. Mesh forwarding is role-blind: every device may forward, and
a device that is charging and well-connected wins forwarding races through a
continuous scoring bias rather than through a role it claims. That worked, and
it removed a whole class of role-state bugs. Applying the same shape one layer
up is a natural instinct.

## Decision

A gateway is a deployment: a powered, stationary box someone installs and
configures, attached to at least one zone and at least one wide-area carrier. It
is never a runtime state a device reaches on its own.

Inside its zone a gateway is an ordinary peer, with no role and no state
machine, exactly as before.

## Consequences

### Why the forwarding-bias shape does not transfer

Relay bias is safe inside the zone because of two properties that do not hold
for bridging:

1. **Every device can forward.** The bias picks among a population that is
   entirely capable, so a wrong pick costs a little redundancy.
2. **Suppression makes redundancy cheap.** A neighbour that hears someone else
   carry a frame stands down, so several devices trying is nearly as cheap as
   one.

Backbone attachment has neither. Most devices structurally *cannot* bridge: no
Reticulum interface, no fixed power, no stable address. So "everyone can, the
capable win" has almost no population to draw from. And there is no in-zone
redundancy covering a bridge that leaves or misjudges, because the other devices
were never candidates.

The failure that produces is a **silent partition**: inter-zone traffic stops,
and nothing reports it, because from inside the zone an absent gateway and a
gateway that decided it was not needed look identical.

### What provisioning buys

A zone has exactly the gateways someone installed. "Zero gateways" becomes a
visible operational fact rather than an emergent condition, and it degrades to
the behaviour a zone with no gateway should have anyway: absent facts mean
today's behaviour, so a zone with no gateway simply routes as it always did.

### What it costs

Someone has to install a box, and a zone whose only gateway is switched off has
no bridge until a person acts. That is the intended trade: an outage a human can
see beats a partition no one can.

## What would undo this

An "auto-promote to gateway" heuristic: a device noticing it has a Reticulum
interface and a charger and enrolling itself. It would look like a natural
extension of the forwarding bias and would reintroduce exactly the silent
partition above.

Provisioned-ness is also what makes the operational advice ("install two")
meaningful. If gateways were emergent there would be nothing to install twice.
