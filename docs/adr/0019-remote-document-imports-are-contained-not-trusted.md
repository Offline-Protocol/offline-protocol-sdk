# 0019. Remote document imports are contained, not trusted

**Status:** Accepted
**Shipped in:** unreleased (F3, 1:1 replication; extended to group spaces
by F4)

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

### 1. MLS scopes the surface; the space is derived, never declared

A blob reaches the import path only from an authenticated member of the space,
and the space is derived rather than read off the frame. On a 1:1 session it
is the wire sender. In a group it is the group whose key opened the
ciphertext, so reaching a group's documents requires being able to encrypt
under that group's key, which is membership.

A peer therefore cannot name a space in either scope, which means they cannot
reach a document shared with somebody else.

**Failure prevented:** cross-space writes, and an authorization table that has
to be right for that to hold.

**What group spaces change:** the number of members who can reach this path,
and nothing else. Every layer below is per space rather than per sender, so a
group is contained by the same machinery as a pair, and a blob refused because
one member sent it stays refused when the next one does.

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
- **Needs history this replica trimmed.** Asked of a whole document as well
  as of a run of changes, and asked differently for each. For a run of
  changes: does it start below this document's base. For a snapshot: does it
  still contain everything this document holds.

Exempting snapshots from the second verdict is the mistake this design made
first, on the reasoning that a snapshot carries its own base so the trim
question cannot arise for one. It reads well and it is wrong, because the
missing ancestors are missing from the *receiver*. A trimmed replica deleted
the ops a forked branch depends on, and supplying them inside the snapshot
does not help: they sit below the replica's own base. Handing one over aborts
the process exactly as a run of changes of the same shape does, which is what
the test now asserts by failing with `SIGABRT` when the verdict is removed.

The verdict has to be narrow as well as safe. Refusing every change a synced
peer sends after the first compaction would be sound and useless, so that
half is pinned by its own test.

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
- A process that dies mid-import for reasons of its own (the user force
  quits it, the system reclaims its memory) quarantines a blob that was never
  at fault. The cost is one document change refused on this device, and it
  clears itself: the digest is over the bytes, so the next catch-up the
  sender computes carries different ones as soon as either replica gains a
  change. The window is the length of one import, and the alternative is
  assuming that a process which died during an import died for an unrelated
  reason.
- A refusal on a run of changes costs a round trip. The receiver names what
  would work (`need_snap`), and a sender holding a superset of what the
  receiver kept answers with a snapshot that closes the gap.

  Answering with a version offer instead does not work, and the failure is
  not a slow one: the peer recomputes changes since our version, which is the
  same refused delta, and the two sides trade it for as long as both keep at
  it. That is why the request is its own frame rather than a reuse of the
  offer.

- **A refusal on a snapshot costs convergence, and this is a real limit
  rather than a round trip.** When two replicas fork below a point one of
  them has compacted away, the ancestors the branch depends on were deleted
  on that side. No frame carries them back, including the whole document, so
  the two stay apart and the divergence is logged rather than retried. It
  takes a partition that outlives a compaction, which is ordinary, so this is
  a known gap in 1:1 replication and not a corner. Refusing is still correct:
  the alternative is not convergence, it is an aborted process.

  It shrinks the same way the rest of this does, and in the meantime the
  honest shape of the guarantee is: replicas that stay in contact converge,
  and replicas separated across a compaction may not.
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
