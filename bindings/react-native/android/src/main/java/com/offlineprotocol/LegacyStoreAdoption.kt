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

    /**
     * Key type under which a *namespaced* store records that a legacy copy
     * survived its own deletion.
     *
     * [MlsSecureStorage.delete] removes both copies, because read-through would
     * otherwise hand back key material the caller believes is gone. The legacy
     * removal can fail on its own — a rotated master key, a keystore that will
     * not open — and it cannot be reported by failing the delete: core treats a
     * storage delete as fatal almost everywhere (OpenMLS aborts Welcome
     * processing and every commit merge on one), and there is no retry anywhere
     * to fall back on. So a failed legacy removal is recorded instead: a
     * tombstone makes read-through treat that key as absent, which is the
     * guarantee `delete` actually owes its caller. The corpse in the legacy
     * store is inert.
     *
     * Tombstones live only in the namespaced store, are never promoted, and are
     * never reported as key material.
     */
    const val TOMBSTONE_KEY_TYPE = "__offline_protocol_tombstone__"

    /**
     * The tombstone entry naming one legacy key.
     *
     * Joined exactly like the stores' own account keys, so it inherits their
     * existing (accepted) ambiguity between `("a", "b:c")` and `("a:b", "c")`
     * rather than introducing a new one. A collision would over-suppress a
     * legacy read — degraded, never a resurrection.
     */
    fun tombstoneKeyId(keyType: String, keyId: String): String = "$keyType:$keyId"

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

    /**
     * What the legacy store reports about its claim, for a caller deciding
     * whether it may *destroy* that store.
     *
     * Adoption collapses "absent" and "unreadable" into one answer on the way
     * in — both mean "looks unclaimed", and the worst case is a fresh identity.
     * A wipe cannot collapse them: the worst case there is deleting the MLS
     * identity, sessions, and block list of a *different* account that has not
     * yet had its first post-split launch. So the two are kept apart here.
     */
    sealed class LegacyClaim {
        /** No claim recorded. The store has not been inherited by anyone. */
        object Absent : LegacyClaim()

        /** The recorded claim, verbatim. */
        data class Owned(val namespace: String) : LegacyClaim()

        /** The claim could not be read, so ownership is unknown. */
        object Unreadable : LegacyClaim()

        companion object {
            /**
             * Classifies a claim value that was read successfully. An empty
             * value is absence, matching how [decide] reads it.
             */
            fun of(value: String?): LegacyClaim =
                if (value.isNullOrEmpty()) Absent else Owned(value)

            /**
             * Classifies a claim read as raw bytes.
             *
             * Decoded *lossily* — invalid sequences become U+FFFD rather than
             * failing the decode — so bytes that are present but not UTF-8
             * classify as [Owned], never [Absent].
             *
             * The distinction is load-bearing for the wipe. Bytes this SDK did
             * not write are still evidence that *something* claimed the store,
             * and [Absent] is the one classification that authorises destroying
             * it — the shared, pre-namespace store holding another account's
             * MLS identity, sessions, and block list. A garbled claim is far
             * more likely a claim this reader cannot interpret than no claim at
             * all, so it fails closed: the mismatch against any real namespace
             * refuses the wipe, and refusing costs only a leftover the next wipe
             * removes.
             *
             * Adoption gets the same answer, which is also right: an account
             * facing an unreadable claim starts fresh and says so, rather than
             * inheriting an identity whose owner it could not establish.
             *
             * Non-nullable on purpose, matching the Swift overload: a caller
             * that has no bytes at all knows it is looking at [Absent] without
             * decoding anything, and a nullable overload here would make the
             * bare `of(null)` ambiguous.
             */
            fun of(bytes: ByteArray): LegacyClaim = of(String(bytes, Charsets.UTF_8))
        }
    }

    /**
     * Whether [namespace] may delete the legacy store outright.
     *
     * Wiping is permitted when this account already owns the claim, and when
     * the store is unclaimed. Unclaimed is the case that matters in practice:
     * on the built-in path every account that has completed a post-split launch
     * has recorded a claim, so an unclaimed store is what the *previous* install
     * left behind — precisely the leftover a logout is asked to erase, and (on
     * platforms whose credential store outlives the app container) the state
     * that would otherwise be re-adopted after a reinstall.
     *
     * An unreadable claim fails closed. It is indistinguishable from a foreign
     * claim, and the two outcomes are not symmetric: refusing costs a leftover
     * store that the next successful wipe removes, while proceeding can silently
     * destroy another account's identity and block list.
     */
    fun shouldWipeLegacy(claim: LegacyClaim, namespace: String): Boolean = when (claim) {
        is LegacyClaim.Absent -> true
        is LegacyClaim.Owned -> claim.namespace == namespace
        is LegacyClaim.Unreadable -> false
    }

    /** True when read-through to the legacy store is permitted. */
    fun allowsReadThrough(decision: Decision?): Boolean =
        decision is Decision.Adopt || decision is Decision.Resume

    /**
     * True for the reserved claim entry, which must never be promoted into the
     * new store or reported by `listKeys`.
     */
    fun isClaimEntry(keyType: String): Boolean = keyType == CLAIM_KEY_TYPE

    /**
     * True for either reserved entry — the legacy store's claim and the
     * namespaced store's tombstones.
     *
     * Both are the provider's own bookkeeping rather than key material, so
     * neither may reach a caller: read-through skips them, `load` reports them
     * absent, and `listKeys` never names them. The provider reads its own
     * tombstones through the private primitives, which are not gated.
     */
    fun isReservedEntry(keyType: String): Boolean =
        keyType == CLAIM_KEY_TYPE || keyType == TOMBSTONE_KEY_TYPE
}
