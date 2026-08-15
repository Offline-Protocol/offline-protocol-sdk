# 0003. Identity is a self-certifying address, not a trust-on-first-use pin

**Status:** Accepted
**Shipped in:** 0.21.0

## Context

Earlier designs identified peers by an application-chosen name and defended
impersonation with a trust-on-first-use store: the first identity key seen for a
name was pinned, and a later mismatch was refused.

Three problems with that in a mesh:

1. **First contact is undefended.** The pin protects the second meeting onward.
   In a mesh where peers meet strangers constantly, first contact is the common
   case, not the edge case.
2. **The pin store is state that must be persisted, migrated, synchronized
   across a user's devices, and recovered after a reinstall.** Every one of
   those is a place to get it wrong, and getting it wrong fails **open**.
3. **The relay answers are structurally unsignable**, so the pin could never be
   applied to them anyway.

## Decision

Make the identity name a function of the identity key:

```
address = bech32m("off", 0x01 || SHA-256(ed25519_public_key)[0..20])
```

Verification derives an address from the presented public key and compares it to
the claimed sender. No stored state, no first-contact window, and nothing to
migrate.

The trust-on-first-use store was deleted rather than kept alongside.

## Consequences

**Good.** First contact is as defended as the thousandth. Impersonation of a
chosen peer costs a second preimage, roughly 2^160.

**Good.** No trust state to persist, corrupt, or lose.

**Cost.** Addresses are not human-readable. Any human-facing name is a directory
concern layered on top, and the mapping from name to address is outside the
protocol's trust model.

**Cost, and this one is real.** Truncation to 160 bits gives only ~2^80
collision resistance. An attacker who finds a collision holds two signing keys
indistinguishable at the address layer, which defeats the one-identity-one-leaf
property the group binding otherwise inherits from MLS. The trade was made
because every mesh frame carries two addresses and the Bluetooth LE budget is
binding. Widening the hash is a version bump and a migration.

**Cost.** Two identifier namespaces now coexist in deployments with a relay that
keys by username. They do not intersect, and assuming they do has already caused
one delivery bug (see [ADR 0011](0011-relay-broadcast-gated-on-delivery-report.md)).

## Canonicality is a requirement, not a nicety

A bech32 decoder that accepts a string and returns a payload has **not** proved
that re-encoding the payload yields that string. Two spellings decoding to one
address splits every set, map, and deduplicator keyed by the rendered form.

Implementations must re-encode and compare, or refuse every non-canonical form
explicitly. Uppercase input is refused even though BIP-173 permits it.

## Two orderings exist, and a tiebreaker must not mix them

The bech32 charset is not monotonic in ASCII, and the rendering includes
checksum characters that carry no identity, so **hash-byte order and rendered
string order are different orders**. `Address` implements `Ord` over the hash
bytes for exactly that reason: the identity-bearing comparison is the one on the
bytes.

The protocol's tiebreakers do not all use it:

| Tiebreaker | Compares |
|------------|----------|
| Session slot ownership (both-create) | `Address` values, so hash bytes, falling back to string order when either identifier does not parse as an address |
| Group leave election | rendered address strings |
| Admin auto-promotion | rendered address strings |
| Fork leader election | rendered address strings |

The fallback in the first row is part of the contract, not an implementation
detail: identifiers predate addresses, and a peer still carrying a legacy
identifier has no hash bytes to compare. Both sides apply the same fallback, so
it converges, but a second implementation that omits it diverges on exactly
those pairs. The session slot identifier derives its ordering the same way, with
the same fallback.

Both orders are deterministic and total, so each of these converges: every peer
running a given tiebreaker sorts the same way and reaches the same winner.

**The invariant is per tiebreaker, not global.** A second implementation MUST
use, for each tiebreaker, the order named above, and a change MUST NOT
"harmonize" one site onto the other order. That is the move that breaks
convergence, because the peers that changed and the peers that did not now elect
different winners from identical input, and neither side can detect the
disagreement locally.

Prefer hash-byte order for anything new: it compares identity rather than
encoding.
