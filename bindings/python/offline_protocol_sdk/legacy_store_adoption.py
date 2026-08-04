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
account can inherit it. The first to launch writes a claim *and reads it back* —
an unverified claim is not an adoption, see :func:`confirm_claim`; a second
account seeing a foreign claim gets a fresh identity — correct, because the
legacy store never held a separable identity for it — but must say so out loud
rather than rotate silently.

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
from enum import Enum

#: Key under which an adopting account records its claim in the *legacy* store.
#: Namespaced away from any real key type so it can never collide with MLS
#: material, and filtered out of read-through and listing.
CLAIM_KEY_TYPE = "__offline_protocol_migration__"
CLAIM_KEY_ID = "claimed_by"

#: Key type under which a *namespaced* store records that a legacy copy
#: survived its own deletion.
#:
#: ``SecureStorage.delete`` removes both copies, because read-through would
#: otherwise hand back key material the caller believes is gone. The legacy
#: removal can fail on its own — a backend that refuses the delete, a locked
#: credential store — and it cannot be reported by failing the delete: core
#: treats a storage delete as fatal almost everywhere (OpenMLS aborts Welcome
#: processing and every commit merge on one), and there is no retry anywhere to
#: fall back on. So a failed legacy removal is recorded instead: a tombstone
#: makes read-through treat that key as absent, which is the guarantee
#: ``delete`` actually owes its caller. The corpse in the legacy store is inert.
#:
#: Tombstones live only in the namespaced store, are never promoted, and are
#: never reported as key material.
TOMBSTONE_KEY_TYPE = "__offline_protocol_tombstone__"


def tombstone_key_id(key_type: str, key_id: str) -> str:
    """The tombstone entry naming one legacy key.

    Joined exactly like the stores' own account keys, so it inherits their
    existing (accepted) ambiguity between ``("a", "b:c")`` and ``("a:b", "c")``
    rather than introducing a new one. A collision would over-suppress a legacy
    read — degraded, never a resurrection.
    """

    return f"{key_type}:{key_id}"


class TombstoneState(Enum):
    """What a tombstone read established about one legacy key.

    Three-way for the same reason the wipe's claim classification is: the two
    non-``ABSENT`` answers authorise different things. Both suppress
    read-through, because a read that failed cannot prove read-through is safe.
    Only ``RECORDED`` additionally authorises *deleting* the legacy copy, and
    that asymmetry is the point — a failed read is not evidence that a tombstone
    exists, so deleting on it would destroy the last copy of a key that was
    legitimately inheritable, which on a first post-upgrade launch can be the
    MLS signing identity. Suppression costs a read-through until the store
    recovers and the next read heals it; the deletion cannot be walked back.
    """

    #: A tombstone is recorded: this key's legacy copy outlived a delete.
    RECORDED = "recorded"
    #: No tombstone recorded. Read-through may proceed.
    ABSENT = "absent"
    #: The tombstone could not be read, so it is unknown either way.
    UNREADABLE = "unreadable"

    @property
    def suppresses_read_through(self) -> bool:
        """Whether the legacy store must not be consulted for this key."""

        return self is not TombstoneState.ABSENT

    @property
    def allows_removal_retry(self) -> bool:
        """Whether the legacy copy may be deleted on sight. Only a *confirmed*
        tombstone: see the class note."""

        return self is TombstoneState.RECORDED


@dataclass(frozen=True)
class Decision:
    """Outcome of resolving one account's claim on the legacy store."""

    #: ``"adopt"`` (unclaimed — claim it), ``"resume"`` (already ours),
    #: ``"conflict"`` (another account owns it), ``"claim_unverified"`` (the
    #: claim could not be recorded), or ``"none"`` (nothing to inherit).
    kind: str
    claimed_by: str | None = None

    @property
    def allows_read_through(self) -> bool:
        return self.kind in ("adopt", "resume")


ADOPT = Decision("adopt")
RESUME = Decision("resume")
NONE = Decision("none")
CLAIM_UNVERIFIED = Decision("claim_unverified")


def decide(existing_claim: str | None, namespace: str) -> Decision:
    if not existing_claim:
        return ADOPT
    if existing_claim == namespace:
        return RESUME
    return Decision("conflict", claimed_by=existing_claim)


def confirm_claim(read_back: str | None, namespace: str) -> Decision:
    """Confirm a claim by what the legacy store reports *after* the write.

    :func:`decide` returning ``ADOPT`` only means the store looked unclaimed;
    it is the recorded claim that makes inheritance exclusive. A write whose
    result is not read back is therefore not an adoption: if it silently
    failed, the next account to launch also finds the store unclaimed, also
    adopts, and the two end up sharing one MLS signing identity — and with it
    each other's sessions and group state. That is strictly worse than the
    conflict this claim exists to produce, so an unproven claim fails closed to
    ``CLAIM_UNVERIFIED``.

    The cost is a fresh identity for a launch that hit a transient store
    failure. Accepted deliberately: confidentiality between two accounts on one
    device outranks the sessions of an install whose credential store is
    failing writes — and the same failure would break every other write this
    session anyway.

    *read_back* is what the legacy store reports for the claim entry once the
    write returned, or ``None`` when the write raised or the read back failed.
    """

    if not read_back:
        return CLAIM_UNVERIFIED
    if read_back == namespace:
        return ADOPT
    return Decision("conflict", claimed_by=read_back)


def is_claim_entry(key_type: str) -> bool:
    """True for the reserved claim entry, which must never be promoted into the
    new store or reported by ``list_keys``."""

    return key_type == CLAIM_KEY_TYPE


def is_reserved_entry(key_type: str) -> bool:
    """True for either reserved entry — the legacy store's claim and the
    namespaced store's tombstones.

    Both are the provider's own bookkeeping rather than key material, so
    neither may reach a caller: read-through skips them, ``load`` reports them
    absent, and ``list_keys`` never names them. The provider reads its own
    tombstones through the private primitives, which are not gated.
    """

    return key_type in (CLAIM_KEY_TYPE, TOMBSTONE_KEY_TYPE)
