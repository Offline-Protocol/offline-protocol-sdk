# 0019. Remote document imports are contained, not trusted

**Status:** Accepted
**Shipped in:** unreleased (F3, 1:1 replication)

## Context

[ADR 0018](0018-data-layer-engine-and-storage-seams.md) established that the
document engine only ever imports bytes that came back out of a sealed
protocol-state record, and closed by naming the thing it could not answer:
replication "cannot simply hand remote deltas to `import`: that is a different
threat model, and it needs its own answer."

This is that answer.

The difficulty is that the obvious argument does not work. Sync frames arrive
inside MLS, so every blob is attributable to an authenticated member of the
space, and it is tempting to stop there. But authentication answers who sent
the bytes, and the question the engine is about to ask is what shape they are.
Those are unrelated. A peer can be exactly who they claim, running exactly our
software, and still send a blob that ends the process.

Two things make that more than theoretical:

- The engine has open defects (loro #793, #1068) where a malformed or
  causally impossible import panics rather than returning an error, and #1068
  poisons the document's lock on the way out, so the retry panics too.
- The mobile profile ships `panic = "abort"`. There is no unwinding, so the
  `catch_unwind` that contains this at rest is not merely a second line
  there, it is absent.

We reproduced the panic rather than taking the issue tracker's word for it. A
change forked below a compacted replica's trim point panics in
`pending_changes.rs` on loro 1.13.9 and leaves the document unusable for the
process. The test that pins the refusal fails, loudly and in exactly that way,
when the refusal is removed.

The uncomfortable part is that the shape is not rare and not hostile. A
replica compacts, which trims history; a peer that was partitioned at the
time sends a change built on what was trimmed. That is ordinary
partition-and-reconnect traffic, which is the entire point of this layer.

## Decision

Remote blobs are contained in layers, each of which stops something specific.
None of them is "the peer is authenticated, so this is fine".

### 1. MLS scopes the surface; the space is the sender

A blob reaches the import path only from an authenticated member of the space,
and the space is derived from the wire sender rather than read off the frame.
A peer therefore cannot name a space, which means they cannot reach a document
shared with somebody else.

**Failure prevented:** cross-peer writes, and an authorization table that has
to be right for that to hold.

### 2. Frames are bounded before they are decoded

A sync frame carries at most 32 KiB of document bytes, checked on the encoded
length before allocating for the decode.

**Failure prevented:** memory amplification (the engine has an open issue
there too) from a frame that costs nothing to send.

### 3. The blob is judged before the engine sees it

Header metadata is decoded with its checksum, and two verdicts are refusals
rather than errors:

- **Already applied.** The document holds everything the blob carries. This
  is the ordinary redelivery an at-least-once ladder produces, and on a
  compacted document it is also the #1068 shape.
- **Reaches below the trim point.** No run of changes can express the gap, so
  the answer is to ask for a snapshot instead.

The second refusal has to be narrow as well as safe. Refusing every change a
synced peer sends after the first compaction would be sound and useless, so
that half is pinned by its own test.

**Failure prevented:** an aborted process, or an unrecoverable document, from
bytes that are perfectly ordinary traffic.

### 4. A blob in flight is remembered on disk

Before a novel blob is handed to the engine, its digest is written to a sealed
per-space record; the record is cleared when the call returns. A digest still
present at the next open means the previous run did not survive that import,
and the blob is refused from then on.

The ordering is the mechanism. Written after, it would record only the
imports that already worked.

This costs one extra sealed write per novel blob, and buys the difference
between "one crafted frame ends this install" and "one crafted frame ends it
once": without it, the sender's retry ladder re-delivers the same blob and
kills the process again on every launch. Duplicates never reach this layer,
because verdict 3 has already refused them.

**Failure prevented:** a crash loop driven by the delivery ladder faithfully
doing its job.

### 5. A parked change is answered, not stored

A change accepted but held behind a missing predecessor lives only in the
engine's memory: it is absent from the document's version, so nothing flushes
it and a restart loses it. Rather than build a second durable log for
something the layer already knows how to recover, parking triggers a version
exchange toward the sender, and the exchange refills both the predecessor and
the parked change.

**Failure prevented:** a durable inbox that has to be reconciled after a
crash, to solve a problem the anti-entropy exchange already solves.

## Consequences

- The residual is real and worth stating plainly: an authenticated member of a
  space can, with a deliberately crafted blob whose metadata lies about its
  contents, abort the process once per unique blob on a `panic = "abort"`
  build. It is attributable, it is bounded by layer 4, and it requires
  someone the user has already accepted into a shared document.
- The refusals cost round trips, never convergence. Every one of them is
  answered by the version exchange, which is why they can be conservative
  without stalling.
- This is a workaround for engine defects, and it should shrink. When loro's
  import hardening reaches the Rust release, the pin moves (re-running the
  MSRV check and the binary size measurement, per ADR 0018) and layer 3's
  second verdict can be revisited. Layers 1, 2 and 4 stand regardless.
- The judging happens in the engine crate, not the protocol crate, so no
  caller can reach the unguarded path by accident. `import` remains for bytes
  out of a sealed record; `import_remote` is the only door from the network.

## Alternatives rejected

- **Trust MLS and import directly.** The whole subject of this ADR.
  Authentication is not validation.
- **Rely on `catch_unwind`.** It does not exist under `panic = "abort"`,
  which is the profile the mobile artifact ships. A defense that is absent
  on the platform with the most users is not a defense.
- **Validate the blob ourselves before importing.** Re-implementing the
  engine's decoder to decide what the engine will accept means maintaining a
  second parser that has to agree with the first one forever, and the day it
  disagrees is the day it is wrong.
- **A durable inbox of received blobs.** Heavier write amplification than the
  in-flight marker, and unnecessary once parked changes are recovered by the
  version exchange rather than by replay.
- **Quarantine in memory only.** It survives nothing, and the failure it has
  to survive is the process dying.
