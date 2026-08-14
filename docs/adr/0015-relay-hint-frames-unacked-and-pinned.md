# 0015. Relay hint frames are unacknowledged and transport-pinned

**Status:** Accepted

## Context

Two frames ask the local bridge to do something rather than asking a peer:
register a group roster with the relay, and broadcast a group message through
it. They are addressed to **this device**, recognized by the bridge, and
**replaced** with relay-native frames before anything reaches the wire.

Treating them as ordinary messages is the obvious implementation and it fails in
two independent ways.

## Decision

Send both with acknowledgement disabled, and pin both to the internet transport
rather than routing them through the transport selector.

Put their retry policy at the application layer instead: bounded, explicit
trackers with their own timeouts and downgrade paths.

## Consequences

### Why acknowledgement must be disabled

The frame is replaced, so no acknowledgement can ever return.

On the ordinary ladder that means 10 retransmissions over roughly 800 seconds.
Each resend is another **full relay fan-out**, under a fresh relay-minted
identifier that receiver deduplication does not catch. It ends in a delivery
failure for an identifier the application never saw, plus a transport-selector
penalty against a transport that did nothing wrong.

So: no outbox entry, no pending acknowledgement, no retry entry.

### Why the transport must be pinned

The transport selector demotes the internet transport by design, so a
self-addressed frame under ordinary routing goes to a mesh transport.

Mesh transports enqueue unconditionally and return success. The caller therefore
believes the broadcast succeeded, skips its per-member fallback, and delivers to
**nobody**.

Pinning also means transport errors propagate to the caller rather than being
deferred, which is what lets the fallback trigger correctly.

### Cost

Two frames that do not behave like any other frame, and a retry policy that has
to be written twice by hand rather than inherited.

## Application-layer retry policy

| Frame | Policy |
|-------|--------|
| Registration | 30 s timeout, 3 attempts |
| Broadcast | 60 s timeout, 3 attempts, report-settled, then downgrade to per-member fan-out |

## What would undo this

Routing them through the selector "for consistency". Enabling acknowledgement
"so we know it worked". Neither can work: nothing acknowledges a frame that was
replaced, and any mesh transport will happily swallow it.
