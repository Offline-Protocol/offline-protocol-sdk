package com.offlineprotocol

import android.content.Context
import org.junit.Assert.assertEquals
import org.junit.Assert.assertNotNull
import org.junit.Assert.assertNull
import org.junit.Test
import org.junit.runner.RunWith
import org.robolectric.RobolectricTestRunner
import org.robolectric.RuntimeEnvironment
import org.robolectric.annotation.Config
import java.util.UUID

/**
 * The protocol-state half of [OfflineProtocolReferenceStorage], and the
 * adoption outcome it reports.
 *
 * The secure half cannot run here: `EncryptedSharedPreferences` needs a real
 * AndroidKeyStore (see [MlsSecureStorageWipeTest]).
 *
 * Pinned to this module's `minSdkVersion` because [OfflineProtocolReferenceStorage.wipe]
 * reaches [MlsSecureStorage.wipeAccount], which needs API 24.
 */
@RunWith(RobolectricTestRunner::class)
@Config(sdk = [24])
class ReferenceStorageTest {
    private val context: Context
        get() = RuntimeEnvironment.getApplication()

    /**
     * [OfflineProtocolReferenceStorage.wipe] erases the store
     * [OfflineProtocolReferenceStorage.makeProtocolStateStorage] opened. A wipe
     * that derived the account's path on its own would return and erase
     * nothing. The secure half may fail here; the state store must still be
     * wiped, because the wipe attempts both halves.
     */
    @Test
    fun wipeErasesTheStoreTheFactoryOpened() {
        val appId = "reference-storage-test"
        val profile = UUID.randomUUID().toString()
        val written = OfflineProtocolReferenceStorage.makeProtocolStateStorage(context, appId, profile)
        written.store("pending/messages", "peer", byteArrayOf(1, 2, 3))
        assertNotNull(written.load("pending/messages", "peer"))

        runCatching { OfflineProtocolReferenceStorage.wipe(context, appId, profile) }

        val reopened = OfflineProtocolReferenceStorage.makeProtocolStateStorage(context, appId, profile)
        assertNull(reopened.load("pending/messages", "peer"))
    }

    /**
     * Only the two outcomes that start a fresh identity carry a warning, and
     * every decision the secure store can reach maps to one outcome.
     */
    @Test
    fun onlyAFreshIdentityCarriesAWarning() {
        val outcomes = listOf(
            LegacyStoreAdoption.Decision.None to OfflineProtocolReferenceStorage.LegacyAdoption.NONE,
            LegacyStoreAdoption.Decision.Adopt to OfflineProtocolReferenceStorage.LegacyAdoption.ADOPTED,
            LegacyStoreAdoption.Decision.Resume to OfflineProtocolReferenceStorage.LegacyAdoption.RESUMED,
            LegacyStoreAdoption.Decision.Conflict("account-other") to
                OfflineProtocolReferenceStorage.LegacyAdoption.CONFLICT,
            LegacyStoreAdoption.Decision.ClaimUnverified to
                OfflineProtocolReferenceStorage.LegacyAdoption.CLAIM_UNVERIFIED,
        )
        for ((decision, expected) in outcomes) {
            val adoption = OfflineProtocolReferenceStorage.LegacyAdoption.of(decision)
            assertEquals(expected, adoption)
            assertEquals(adoption.name, adoption.startsFreshIdentity, adoption.warning != null)
        }
        assertEquals(
            setOf(
                OfflineProtocolReferenceStorage.LegacyAdoption.CONFLICT,
                OfflineProtocolReferenceStorage.LegacyAdoption.CLAIM_UNVERIFIED,
            ),
            OfflineProtocolReferenceStorage.LegacyAdoption.values().filter { it.startsFreshIdentity }.toSet(),
        )
    }
}
