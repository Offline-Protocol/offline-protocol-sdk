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

    @Test
    fun adoptAndResumeAllowReadThrough() {
        assertTrue(LegacyStoreAdoption.allowsReadThrough(LegacyStoreAdoption.Decision.Adopt))
        assertTrue(LegacyStoreAdoption.allowsReadThrough(LegacyStoreAdoption.Decision.Resume))
        assertFalse(LegacyStoreAdoption.allowsReadThrough(LegacyStoreAdoption.Decision.None))
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
