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

### A package the application publishes keeps its record

**Invariant: no key package record of ours is deleted while its provider key
is still resident, unless the provider key is destroyed in the same step.** The
only record-only deletes are the loader's, for a package whose provider key is
already gone or whose bytes name no key at all.
`only_the_known_sites_delete_a_key_package_record_alone` reads the source and
refuses a third.

`mark_key_package_synced` is the case this used to miss. It exists only on the
FFI surface, for an application that uploads a package to its own key server,
and it deleted the record and left the provider key in place. Keeping the key
was right: a stranger who fetched the uploaded copy may still Welcome us, and
only that Welcome consumes it. Deleting the record was not, because the record
is what carries expiry, so a published package nobody claimed kept its init
key for the life of the install (issue 367).

Marking now keeps the record and sets `synced` on it:

1. **The record stays,** so the package is withdrawn at expiry and its key
   destroyed past the grace window like every other package.
2. **The package is withheld from both hand-out paths,** through the same
   predicate that withholds a slot package. A synced package is one somebody
   else holds, and pointing a peer at it too is the shared-key failure this
   ADR exists to prevent.
3. **The pending list holds only what the application may publish:** unclaimed,
   unreserved and unsynced. It used to list every live package, so an
   application following the documented upload loop marked the engine's own
   slot and push packages synced. With the old delete, each one was stranded as
   soon as the engine minted its successor.

Marking an unknown, expired or consumed id does nothing. Marking a package the
push path already handed to a peer is allowed and logged: the two holders now
share an init key, the peer's copy stays openable, and if the uploaded copy is
spent first, the next push to that peer mints a successor. A package can still
be claimed by a push between the application's list call and its mark; the
window is one discovery event wide and the outcome is the same as this case.

## What would undo this

Adding a "get a key package" convenience that does not take a peer and does not
skip assigned packages. The peerless escape hatch exists for FFI and tests, and
it must skip both reserved and peer-assigned packages.

Deleting a key package record without purging its provider key.

A hand-out path that stops skipping synced packages, or a pending list that
shows a package the engine owns.
