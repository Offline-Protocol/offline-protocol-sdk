# 0017. Nostr is a carrier, never a gateway

**Status:** Accepted

## Context

The [gateway contract](../spec/gateway-contract.md) requires five verbs, of
which Verdict is load-bearing: a per-recipient answer that a frame was forwarded
or that the recipient is unreachable. Verdict is what lets a sender discover
that a carrier which is *up* cannot reach a *particular* recipient, which is the
one fact ordinary transport selection cannot supply.

Nostr is one of this protocol's carriers. It reaches peers over relays that do
their own routing, so it looks like the same class of thing as the internet
relay, and it is natural to expect it to become a gateway once gateways exist.

It cannot. A Nostr relay accepts an event and reports whether it accepted it. It
does not know, and does not report, whether any particular recipient received
it. There is no per-recipient delivery signal to translate into a verdict, and
no amount of implementation work on our side creates one.

## Decision

Nostr remains a carrier and is never a gateway. It advertises no gateway
capability, produces no verdicts, and keeps the peer-blind reachability claim it
has today: while a Nostr transport is available, it counts as a way to reach
every recipient.

Record the resulting gap as permanent rather than outstanding.

## Consequences

A device whose only infrastructure carrier is Nostr never receives an
unreachable verdict. Nothing contradicts the initial "reachable" answer, so no
mesh fallback fires for it on the send path, and its messages settle by
acknowledgement or expiry exactly as they always have.

This is a real limitation and it is worth stating plainly: in a mixed
neighbourhood, a Nostr-only sender standing next to an unreachable recipient
will not hand the frame to its neighbours before the acknowledgement ladder runs
out. The live-mesh-link preference still applies, because that needs no verdict:
a recipient this device holds a link to is reached over that link. What is
missing is only the case where the recipient is reachable through *someone
else's* radio.

Recipient-aware policy is written to make this degradation structural rather
than accidental: a carrier that produces no facts keeps its blanket claim. A
verdict from a different carrier must never silence Nostr, or the residual would
close by accident in some configurations and not others, which is worse than a
consistent limitation.

## What would undo this

A Nostr extension that reports per-recipient delivery, or a deployment placing a
verdict-capable component in front of Nostr relays. Either would make Nostr
gateway-eligible with no change to the contract, since the contract is semantic
and says nothing about wire format.

Absent that, the thing to guard against is someone reading the residual as an
unfinished feature and "fixing" it by inferring verdicts: treating a relay's
`OK` rejection, or a send timeout, as evidence about the recipient. Both are
evidence about the *relay*. Inferring recipient state from them would produce
confident wrong claims, which is worse than the honest silence this decision
accepts.
