# 0012. The push path assigns one MLS init key per peer

**Status:** Accepted
**Shipped in:** 0.20.0

## Context

An MLS init key is **single-use**: it is consumed when a Welcome built against
it is processed.

The push path returned the first stored key package to every caller and minted a
new one only once somebody's Welcome had spent it. One init key was therefore
advertised to every peer a device met until it was consumed.

The visible bug: two peers handed the same package cannot both establish. The
second peer's Welcome is unprocessable, and the symptom is a session that never
comes up rather than an error.

The security concern: this is the last-resort reuse RFC 9420 section 16.8
permits only as a denial-of-service fallback, and which external MLS audits have
flagged as enabling unsolicited joins, cross-group linkage, and resource
consumption.

## Decision

Assign **one package per peer**. Resolution order for a push:

1. This peer's own live package, so repeat pushes cost no key material.
2. An unclaimed package, **claimed here**. Claiming is what stops an upgrade
   stranding a pre-existing package.
3. A fresh mint.

Store the assignment **on the bundle**, not in a side map, so it survives
restarts and cannot disagree with the pool.

Rotate on consumption: a consumed package is reported gone and the next push
mints a successor.

## Consequences

**Good.** Every peer gets an init key only they can spend. Session establishment
stops failing silently in the two-new-peers case.

**Cost.** A per-push scan over the pool. Made cheap by caching each package's
provider reference at mint time, so usability checks skip a parse and a signature
validation. Without that cache a many-peer push loop costs minutes in a debug
build.

## The ceiling shares rather than refuses

At 64 live packages, the pool **shares the newest package** and reports
exhaustion as a suppressed warning.

Refusing to advertise or evicting would each cost session establishment outright,
which is worse than the reuse being avoided. This is the one condition under
which the old shape is back, and it is reported so it is visible.

**The ceiling gates only the mint**, which is the only step that grows the pool.
A claim relabels a package that already exists, so steps 1 and 2 run ahead of the
check and a full pool holding an unclaimed package still hands out its own key.
Gating the claim too would weaken forward secrecy to stay under a bound the claim
never approaches.

Reaching the shared branch therefore proves every live package belongs to another
peer, and "newest" makes it the one most likely mid-establishment: if the
over-ceiling peer's Welcome lands first, that peer's advertisement goes
unprocessable until its next push.

## Expiry destroys key material in two stages

Deleting the bundle record alone leaves the private init key in the MLS provider
**forever**, because only a peer's Welcome removes one. This was the pre-existing
leak and it is the part most likely to be reintroduced.

1. Expiry **withdraws** the package from every caller immediately.
2. Only past a grace window (7 days) is the provider key destroyed. The provider
   key is deleted **first**, and the record is kept so a failed deletion can be
   retried.

The grace window exists so a Welcome built just before expiry still opens.

Deletion also purges legacy records predating the bundle format: an unparseable
record is read as the serialized key package so its provider reference is
derivable. A record-only delete there is the exact stranding this rule removes.

## What would undo this

Adding a "get a key package" convenience that does not take a peer and does not
skip assigned packages. The peerless escape hatch exists for FFI and tests, and
it must skip both reserved and peer-assigned packages.

Deleting a key package record without purging its provider key.
