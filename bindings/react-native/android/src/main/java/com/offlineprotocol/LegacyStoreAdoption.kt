package com.offlineprotocol

/**
 * Whether an account may read through to the legacy, un-namespaced secure store
 * that shipped before storage was scoped to `(app_id, user_id)`.
 *
 * Scoping the store renamed it. Left alone, the first launch after an upgrade
 * would find an empty store, mint a *new* MLS signing identity, and abandon
 * every session, group, and TOFU pin the install already had — peers still
 * holding the old pin would then reject it. So the new store adopts the old one
 * instead.
 *
 * Adoption is read-through rather than a bulk copy on purpose. The legacy
 * store's key types are not a closed set (OpenMLS contributes its own labels
 * and Python's keyring backend cannot enumerate at all), so there is no
 * reliable way to walk it. A miss in the new store consults the legacy one and
 * promotes what it finds, which is naturally idempotent and resumable.
 *
 * The legacy store was shared by every account on the install, so at most one
 * account can inherit it. The first to launch writes a claim *and reads it
 * back* — an unverified claim is not an adoption, see [confirmClaim]; a second
 * account seeing a foreign claim gets a fresh identity — correct, because the
 * legacy store never held a separable identity for it — but must say so out
 * loud rather than rotate silently.
 *
 * Read-through is also what the SDK's *protocol-state* adoption sweep rides on:
 * pre-split delivery state sits in this same un-namespaced store, and the sweep
 * enumerates the namespaced handle. So a conflict costs more than the MLS
 * identity — that account also comes up with an empty outbox, an empty pending
 * queue, and an empty **block list**, every previously blocked peer unblocked.
 * Say all of it, not just the identity.
 *
 * Keep this policy in sync with `LegacyStoreAdoption.swift` and
 * `legacy_store_adoption.py`.
 */
internal object LegacyStoreAdoption {
    /**
     * Key under which an adopting account records its claim in the *legacy*
     * store. Namespaced away from any real key type so it can never collide
     * with MLS material, and filtered out of read-through and listing.
     */
    const val CLAIM_KEY_TYPE = "__offline_protocol_migration__"
    const val CLAIM_KEY_ID = "claimed_by"

    sealed class Decision {
        /** Legacy store is unclaimed: claim it and read through. */
        object Adopt : Decision()

        /** We already claimed it on an earlier launch: keep reading through. */
        object Resume : Decision()

        /**
         * Another account owns the legacy identity. Read-through is off and
         * this account starts fresh — surface it.
         */
        data class Conflict(val claimedBy: String) : Decision()

        /**
         * The claim could not be recorded, so ownership is unproven.
         * Read-through is off — see [confirmClaim].
         */
        object ClaimUnverified : Decision()

        /** No legacy store to inherit from (fresh install, or opted out). */
        object None : Decision()
    }

    fun decide(existingClaim: String?, namespace: String): Decision = when {
        existingClaim.isNullOrEmpty() -> Decision.Adopt
        existingClaim == namespace -> Decision.Resume
        else -> Decision.Conflict(existingClaim)
    }

    /**
     * Confirms a claim by what the legacy store reports *after* the write.
     *
     * [decide] returning [Decision.Adopt] only means the store looked
     * unclaimed; it is the recorded claim that makes inheritance exclusive. A
     * write whose result is not read back is therefore not an adoption: if it
     * silently failed, the next account to launch also finds the store
     * unclaimed, also adopts, and the two end up sharing one MLS signing
     * identity — and with it each other's sessions and group state. That is
     * strictly worse than the conflict this claim exists to produce, so an
     * unproven claim fails closed to [Decision.ClaimUnverified].
     *
     * The cost is a fresh identity for a launch that hit a transient store
     * failure. Accepted deliberately: confidentiality between two accounts on
     * one device outranks the sessions of an install whose credential store is
     * failing writes — and the same failure would break every other write this
     * session anyway.
     *
     * @param readBack what the legacy store reports for the claim entry once
     *   the write returned, or null when the write threw or the read back
     *   failed.
     */
    fun confirmClaim(readBack: String?, namespace: String): Decision = when {
        readBack.isNullOrEmpty() -> Decision.ClaimUnverified
        readBack == namespace -> Decision.Adopt
        else -> Decision.Conflict(readBack)
    }

    /** True when read-through to the legacy store is permitted. */
    fun allowsReadThrough(decision: Decision?): Boolean =
        decision is Decision.Adopt || decision is Decision.Resume

    /**
     * True for the reserved claim entry, which must never be promoted into the
     * new store or reported by `listKeys`.
     */
    fun isClaimEntry(keyType: String): Boolean = keyType == CLAIM_KEY_TYPE
}
