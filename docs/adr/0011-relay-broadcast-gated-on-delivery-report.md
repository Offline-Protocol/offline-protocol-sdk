# 0011. Relay broadcast defaults on, gated on a capability that guarantees a settled report

**Status:** Accepted
**Shipped in:** 0.21.0 (the v3 capability gate)

## Context

Sending a group message as one directed message per member is O(N) frames and
inherits the whole direct-message delivery ladder. Asking the relay to fan out
server-side is one frame.

The first attempt at server-side fan-out was fire-and-forget: no presence check,
no push, no persistence, and "sent" reported before delivery was known. Because
MLS application messages do not advance the epoch, a missed message was
**undetectable**. It stayed off by default, correctly.

## Decision

Default the broadcast **on**, gated on all four of:

1. broadcast enabled in configuration,
2. the roster registered with the relay,
3. the relay advertising `group_delivery_v3`,
4. a live internet check.

What makes it safe to default on is the **delivery report contract**, not the
broadcast itself:

1. The sender mints a **logical message identifier** and carries it in the
   frame. The relay stamps it onto its fan-out verbatim.
2. The sender arms a pending tracker keyed by it.
3. The relay returns a **settled** report of delivered, pushed, and missed
   members.
4. The sender re-sends per-member copies to
   `roster − delivered − pushed − self`.

Step 4 covers reported misses **and members the relay never knew**, because the
relay's registered roster can be a strict subset of the MLS roster.

## Consequences

**Good.** One frame instead of N in the common case, with a backstop that
converges on the same delivery guarantee.

**Good.** Re-broadcast retries reuse the **same** logical identifier, so the
relay echoes it and both receiver deduplication and push deduplication hold
across attempts.

**Cost, known gap.** The tracker is memory-only. A process kill inside the report
window loses the backstop for that broadcast.

**Cost.** Failure handling is three separate paths: lost report (re-broadcast,
bounded to 3 attempts, then downgrade), internet drop (downgrade immediately),
tracker overflow (downgrade the oldest).

## Why the capability token had to be v3 and not v2

This is the part worth remembering, because the surface reason ("a version
bumped") hides the real one.

v3 is v2's settled-report contract **plus an address-aware relay group path**. A
v2 relay must fail the gate **closed**, because its username-keyed path and
address identity cannot compose:

- it cannot route to address-registered members,
- its report names members in a namespace that never intersects the MLS roster,
  so the set difference re-issues to **everyone** after every broadcast,
- any copy it does deliver arrives attributed by username, which fails the
  wire-sender to credential match **after** the decrypt already spent the
  ciphertext's ratchet generation.

That last one is why the **gate** is the fix rather than any receiver-side
cleanup. The generation burn is unrecoverable on the client: MLS implementations
persist message secrets through the storage provider before the identity check
runs, and skipping the group save does not undo it.

## The receiver-side rules invert between paths

Handling the logical identifier correctly requires **opposite** rules on the mesh
and relay paths. Getting either backwards causes silent loss.

**Mesh path: mark only after a successful decrypt**, so a failed decrypt cannot
poison the identifier.

**Relay path: mark at arrival, pre-decrypt**, because there the relay-supplied
identifier **is** the logical identifier and marking it early is the
replay-amplification defence.

The relay path pays for that inversion with an obligation: **every arm that ends
with the frame neither delivered, nor buffered, nor consumed by MLS must unmark
before returning.** Otherwise a rejected copy reads as delivered, the per-member
re-issue is absorbed as a duplicate and acknowledged, and the message is
delivered nowhere while the sender is told it arrived.

The obligation extends to the buffered drain, and that half is not optional: a
relay copy can outrun its Welcome, so it buffers before any decrypt and its
misattribution is judged on the drain instead of at arrival, an ordering a
hostile relay picks for free.

## What would undo this

Adding an "already delivered elsewhere?" check to the drain's plaintext branch.
An MLS decrypt consumes the generation, so reaching that branch **proves** first
delivery. The check is unreachable when true and, because the relay path marks
at arrival, suppresses the only decryptable copy when false.

Accepting the report through generic message-plane injection rather than a
dedicated entry point. See [ADR 0014](0014-dedicated-ffi-entry-points.md).
