"""Whether an account may inherit the pre-namespace secure store.

Scoping the store to ``(app_id, user_id)`` renamed it. Left alone, the first
launch after an upgrade would find an empty store, mint a *new* MLS signing
identity, and abandon every session, group, and TOFU pin the install already
had — peers still holding the old pin would then reject it. So the new store
adopts the old one instead.

Adoption is read-through rather than a bulk copy on purpose. The legacy store's
key types are not a closed set (OpenMLS contributes its own labels, and the
``keyring`` backends cannot enumerate at all), so there is no reliable way to
walk it. A miss in the new store consults the legacy one and promotes what it
finds, which is naturally idempotent and resumable.

The legacy store was shared by every account on the install, so at most one
account can inherit it. The first to launch writes a claim; a second account
seeing a foreign claim gets a fresh identity — correct, because the legacy store
never held a separable identity for it — but must say so out loud rather than
rotate silently.

Read-through is also what the SDK's *protocol-state* adoption sweep rides on:
pre-split delivery state sits in this same un-namespaced store, and the sweep
enumerates the namespaced handle. So a conflict costs more than the MLS
identity — that account also comes up with an empty outbox, an empty pending
queue, and an empty **block list**, every previously blocked peer unblocked.
Say all of it, not just the identity.

Keep this policy in sync with ``LegacyStoreAdoption.swift`` and
``LegacyStoreAdoption.kt``.
"""

from __future__ import annotations

from dataclasses import dataclass

#: Key under which an adopting account records its claim in the *legacy* store.
#: Namespaced away from any real key type so it can never collide with MLS
#: material, and filtered out of read-through and listing.
CLAIM_KEY_TYPE = "__offline_protocol_migration__"
CLAIM_KEY_ID = "claimed_by"


@dataclass(frozen=True)
class Decision:
    """Outcome of resolving one account's claim on the legacy store."""

    #: ``"adopt"`` (unclaimed — claim it), ``"resume"`` (already ours),
    #: ``"conflict"`` (another account owns it), or ``"none"`` (nothing to
    #: inherit).
    kind: str
    claimed_by: str | None = None

    @property
    def allows_read_through(self) -> bool:
        return self.kind in ("adopt", "resume")


ADOPT = Decision("adopt")
RESUME = Decision("resume")
NONE = Decision("none")


def decide(existing_claim: str | None, namespace: str) -> Decision:
    if not existing_claim:
        return ADOPT
    if existing_claim == namespace:
        return RESUME
    return Decision("conflict", claimed_by=existing_claim)


def is_claim_entry(key_type: str) -> bool:
    """True for the reserved claim entry, which must never be promoted into the
    new store or reported by ``list_keys``."""

    return key_type == CLAIM_KEY_TYPE
