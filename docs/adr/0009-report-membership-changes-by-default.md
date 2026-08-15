# 0009. Unauthorized membership changes are reported; rejection is opt-in

**Status:** Accepted

## Context

MLS Add and Remove commits are applied by every receiving member. Nothing in RFC
9420 says only an administrator may issue one; that is an application policy.

The obvious enforcement is to reject a commit from a non-administrator. In this
protocol that is dangerous, for a reason that has nothing to do with attackers:

**Rejecting a commit means declining the merge, which forks you permanently from
everyone who accepted it.** And the administrative overlay replicates
best-effort. Roles ride on unreconciled notifications; joiners receive a
point-in-time snapshot. A member whose role snapshot is merely **stale** would
therefore partition itself from the group with no attacker involved.

## Decision

**Report by default.** Emit an unauthorized-change event, rate-limited per
group, committer, **and whether enforcement was on**, and apply the commit. The
enforcement flag belongs in the key: without it an earlier report-only event
suppresses the refusal alarm for the same committer, which is the one event that
must always reach the application.

Carry a **three-valued** authorization field on roster events:

| Value | Meaning |
|-------|---------|
| checked and authorized | A check ran and passed |
| checked and unauthorized | A check ran and failed |
| **not evaluated** | No check ran: own Welcome join, relay-reconciled **adds** |

Relay-reconciled **removes** are the asymmetry in that last row: the remove path
does run an administrator check against the authenticated wire sender and
reports a real verdict. Adds do not, so they report "not evaluated".

Offer rejection as an explicit opt-in, default off, documented as suitable only
for a closed deployment that controls role distribution, never for part of a
fleet.

## Consequences

**Good.** An unauthorized change is visible to the application without risking a
partition.

**Good.** The third authorization state gives the paths that ran no check
something honest to say. Emitting "authorized" from such a path is a lie the
application cannot detect.

**Cost.** By default an unauthorized change **takes effect**. The protocol
reports; it does not prevent.

**Cost.** Enforcement acts only on a **present** administrative set that
positively excludes a principal; every absent input fails open. It cannot detect
a **divergent** view. Two honest members with different snapshots each hold a
non-empty set, so they reject each other and partition. That is why it stays
opt-in.

## The fail-open rule is load-bearing

When enforcement is on, merge anyway when:

- the commit proposes no membership change,
- the identifier names a 1:1 session,
- group metadata is unreadable or absent,
- **the administrative set is not known to be non-empty.**

Reject only when the administrative set is known non-empty **and** a principal
is positively not in it. The principals are the committer and the sender of each
**Add or Remove** proposal, since MLS lets a member commit a proposal another
member made. Update and PSK proposal senders are deliberately excluded: an
Update is legitimate self-service that needs no administrator, so rejecting an
admin's Add because it batched a member's key update would fork the group over a
proposal that changes no membership.

The creator of record is deliberately **not** consulted here. One unauthenticated
claim is too thin a basis to fork over.

## Three implementation rules

**Enforcement runs at the decryption chokepoint, pre-merge, not in the commit
handler.** Gating only the commit handler leaves two bypasses: a commit reframed
as an application message, and an encrypted envelope naming a group identifier.

**If either roster read fails, skip all delta-derived work.** The delta comes
from a pre-commit read and a post-merge read. A silent empty default on a failed
read fabricates a full-roster delta and a report naming an innocent committer.

**The pre-commit roster must be MLS-derived, never the members cache.** Relay
reconciliation splices entries into that cache that were never in the tree.

## Contrast with the leaf identity binding

[ADR 0010](0010-unconditional-leaf-identity-binding.md) is unconditional, and
the difference is the whole reason both exist.

An administrative verdict depends on best-effort-replicated state, so honest
members can disagree and partition each other. A leaf-binding verdict is computed
from the commit's own bytes, so every honest member reaches the same answer and a
refusal forks the **attacker** off a group that stays consistent.
