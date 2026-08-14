# 0010. Leaf identity binding is unconditional and checked at three seams

**Status:** Accepted
**Shipped in:** 0.21.0

## Context

RFC 9420 sections 5.3.1 and 7.3 assign the Authentication Service to the
**application**: the credential's presented identifiers must be correctly
associated with the signature key in the member's leaf node.

MLS libraries do not do this. OpenMLS says so explicitly for external-commit
validation ("This MUST be checked by the application") and exposes the
credentials for the application to judge.

Without it, an MLS basic credential is a bare self-asserted string. The
wire-sender to credential comparison that authenticates group messages then
proves only that the forger typed the name they wanted, and on the ungated group
data plane that costs a forger no signature from anyone.

## Decision

Every leaf entering local group state must carry the address its **own signature
key** derives to, using the single shared derivation function.

Check it at three seams. They are not redundant:

| Seam | When | Scope | Covers |
|------|------|-------|--------|
| Welcome | Before joining | The **whole** ratchet tree | The inviter chooses the tree wholesale |
| Commit | Pre-merge | Every credential the commit introduces or changes | New and renamed leaves |
| Use | At the sender check | The sending leaf, by index | A leaf that entered by neither gate |

Make it **unconditional**, unlike administrative enforcement.

## Consequences

**Good.** A refusal forks the **attacker** off a group that stays consistent,
because the verdict is computed from the commit's own bytes and every honest
member reaches the same answer.

**Good.** Safe for honest peers, because nothing in this protocol rotates a leaf
signature key or credential independently of the identity key. Any change that
adds such a rotation must revisit this ADR first.

**Cost.** A roster read must filter unbound leaves, so two roster reads in the
same codebase must apply the same filter or they disagree about who is in the
group.

## Why the Welcome walk is all-or-nothing

Joining while skipping bad leaves leaves the joiner at an epoch computed over the
**full** tree, decrypting nothing. There is no partial join.

## The commit walk covers four sources, not two

An implementation that walks only Add and Update proposals leaves the
**cheapest** attack open.

1. **The update-path leaf.** A member renames their own leaf to a peer's address.
   No new leaf and no invite needed. This is the source most often missed.
2. Update proposals.
3. Add proposals.
4. Group context extensions, specifically external senders. Refused outright, as
   are all non-member senders.

Source 2 is unreachable in this protocol today and is kept deliberately. By
value, MLS attributes the proposal to the committer and forbids committing your
own update; by reference, the receiver must hold the proposal, and this protocol
drops received proposals rather than storing them. A propose-only API makes it
live.

## Non-address credentials are refused, never skipped

"Nothing to derive, so pass" is the bypass. It is the same bypass every
derivation check in this protocol has to close explicitly.

## Two consequences that look unrelated and are not

**A refused commit must be classified permanently refused.** Retriability is
decided from an allowlist, so a refusal missing from it is buffered,
re-decrypted on every drain, and, because a buffered commit that expires having
been retried reads as an epoch fork, turns one forged commit into a group-wide
key update round plus a false fork report. Assert this through the frame handler,
not the MLS layer; asserting at the MLS layer is what hid it.

**Removal must remove every matching leaf, not the first.** Through the wire
gates a duplicate is unreachable, because MLS requires unique signature keys and
this binding ties credential to key. That argument covers the gates, not the
tree: a forged leaf written straight into a key store claims a peer's address
while carrying the attacker's key, violates no uniqueness rule, and sits beside
the victim's real leaf. First-match removal leaves the peer holding live keys
while every roster read shows them gone.

## Reporting

A refusal reports the peer that **delivered** the forgery, which is
signature-proved, never the impersonated address.

The report's reason text is identifier-free, because the telemetry scrubber
hashes the peer field and ships free text verbatim, and the address at stake
belongs to the impersonated third party. See
[ADR 0013](0013-exhaustive-privacy-classifier.md).

A roster read that finds an unbound leaf already seated in local state is a
fourth report site, and it differs in kind: no frame was refused and no peer
delivered it, so it names **this device** as the subject, and the remedy it
implies is to abandon the group rather than to evict a member. The leaf cannot
speak, but it holds live group secrets and reads everything, which no later
refusal undoes.
