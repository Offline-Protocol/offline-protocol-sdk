package com.offlineprotocol

import org.junit.Assert.assertEquals
import org.junit.Assert.assertFalse
import org.junit.Assert.assertTrue
import org.junit.Test

class LegacyStoreAdoptionTest {
    private val namespace = "account-" + "a".repeat(64)
    private val other = "account-" + "b".repeat(64)

    /**
     * The upgrade case: an install that predates namespacing has an unclaimed
     * legacy store, and the account that launches first inherits it. Without
     * this the account would look brand new and mint a fresh MLS identity.
     */
    @Test
    fun unclaimedLegacyStoreIsAdopted() {
        assertEquals(
            LegacyStoreAdoption.Decision.Adopt,
            LegacyStoreAdoption.decide(null, namespace)
        )
        assertEquals(
            LegacyStoreAdoption.Decision.Adopt,
            LegacyStoreAdoption.decide("", namespace)
        )
    }

    /**
     * Adoption must be resumable: a launch after the claim was written but
     * before every entry was promoted still reads through.
     */
    @Test
    fun ourOwnClaimResumesReadThrough() {
        assertEquals(
            LegacyStoreAdoption.Decision.Resume,
            LegacyStoreAdoption.decide(namespace, namespace)
        )
    }

    /**
     * The legacy store was shared by every account on the install, so only one
     * can inherit it. A second account is genuinely new — but that must be
     * reported, not silently rotated into.
     */
    @Test
    fun foreignClaimBlocksReadThrough() {
        val decision = LegacyStoreAdoption.decide(other, namespace)

        assertEquals(LegacyStoreAdoption.Decision.Conflict(other), decision)
        assertFalse(LegacyStoreAdoption.allowsReadThrough(decision))
    }

    /**
     * A claim we can read back as our own is what actually makes inheritance
     * exclusive; the pre-write probe only says the store *looked* unclaimed.
     */
    @Test
    fun claimReadBackAsOursCompletesTheAdoption() {
        assertEquals(
            LegacyStoreAdoption.Decision.Adopt,
            LegacyStoreAdoption.confirmClaim(namespace, namespace)
        )
    }

    /**
     * The write reported success (or threw, which the caller spells as null)
     * but the store does not hold our claim. Adopting anyway is what lets a
     * second account find the store still unclaimed, adopt it too, and end up
     * sharing this account's MLS identity — so an unproven claim fails closed.
     */
    @Test
    fun unrecordedClaimDoesNotAdopt() {
        val decision = LegacyStoreAdoption.confirmClaim(null, namespace)

        assertEquals(LegacyStoreAdoption.Decision.ClaimUnverified, decision)
        assertFalse(LegacyStoreAdoption.allowsReadThrough(decision))
        assertEquals(
            LegacyStoreAdoption.Decision.ClaimUnverified,
            LegacyStoreAdoption.confirmClaim("", namespace)
        )
    }

    /**
     * The read back also catches a racing claim, which the pre-write probe
     * cannot see.
     */
    @Test
    fun claimReadBackAsSomeoneElsesIsAConflict() {
        assertEquals(
            LegacyStoreAdoption.Decision.Conflict(other),
            LegacyStoreAdoption.confirmClaim(other, namespace)
        )
    }

    @Test
    fun adoptAndResumeAllowReadThrough() {
        assertTrue(LegacyStoreAdoption.allowsReadThrough(LegacyStoreAdoption.Decision.Adopt))
        assertTrue(LegacyStoreAdoption.allowsReadThrough(LegacyStoreAdoption.Decision.Resume))
        assertFalse(LegacyStoreAdoption.allowsReadThrough(LegacyStoreAdoption.Decision.None))
        assertFalse(
            LegacyStoreAdoption.allowsReadThrough(LegacyStoreAdoption.Decision.ClaimUnverified)
        )
        assertFalse(LegacyStoreAdoption.allowsReadThrough(null))
    }

    /**
     * The claim entry is bookkeeping, not key material: promoting it into the
     * new store would make a later account read its own namespace back as an
     * inherited value.
     */
    @Test
    fun claimEntryIsNeverReadThrough() {
        assertTrue(LegacyStoreAdoption.isClaimEntry(LegacyStoreAdoption.CLAIM_KEY_TYPE))
        assertFalse(LegacyStoreAdoption.isClaimEntry("identity"))
    }
}
