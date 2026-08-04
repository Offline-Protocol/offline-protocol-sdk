package com.offlineprotocol

import android.content.Context
import org.junit.Assert.assertFalse
import org.junit.Assert.assertTrue
import org.junit.Test
import org.junit.runner.RunWith
import org.robolectric.RobolectricTestRunner
import org.robolectric.RuntimeEnvironment
import org.robolectric.annotation.Config
import java.io.File
import java.util.UUID

/**
 * Covers the file-level half of [MlsSecureStorage.wipeAccount].
 *
 * `EncryptedSharedPreferences` cannot run under a JVM harness — it needs a real
 * AndroidKeyStore — which is why the *policy* half lives in
 * [LegacyStoreAdoption] and is tested directly. That limitation is load-bearing
 * for one of these cases: with no keystore the legacy claim reads as
 * `Unreadable`, which is exactly the fail-closed path
 * [legacyStoreThatCannotBeProvedOursIsLeftAlone] pins.
 *
 * It is also why the cases that *destroy* the shared store inject the claim
 * reader. Fail-closed was the only outcome the real reader could produce here,
 * so every branch that actually deletes the pre-namespace store — the one a
 * mistake cannot be walked back — went uncovered. Injecting the judgment, not
 * the deletion, keeps the file-level half of the wipe real.
 */
/// Pinned to this module's `minSdkVersion`. The wipe calls
/// `Context.deleteSharedPreferences` and `Context.getDataDir`, both API 24, and
/// this harness's Robolectric default runs below that — so an unpinned suite
/// would fail on the SDK floor rather than exercise it.
@RunWith(RobolectricTestRunner::class)
@Config(sdk = [24])
class MlsSecureStorageWipeTest {
    private val context: Context
        get() = RuntimeEnvironment.getApplication()

    private fun namespace(label: String): String =
        StorageNamespace.account("secure-wipe-test-${UUID.randomUUID()}", label)

    private fun preferencesFile(name: String): File =
        File(context.dataDir, "shared_prefs/$name.xml")

    /** Writes a stand-in for a store this harness cannot create for real. */
    private fun seedPreferencesFile(name: String): File {
        val file = preferencesFile(name)
        file.parentFile?.mkdirs()
        file.writeText("<?xml version='1.0' encoding='utf-8'?><map />")
        return file
    }

    private fun namespacedFile(account: String): File =
        preferencesFile("mls_secure_storage_v2_$account")

    /**
     * The account's own store is what a logout is for: MLS identity, sessions,
     * TOFU pins, the Nostr secret, and the key every sealed protocol-state
     * record is written under all live in it.
     */
    @Test
    fun wipeRemovesTheNamespacedSecureStore() {
        val account = namespace("wipe-namespaced")
        val file = seedPreferencesFile("mls_secure_storage_v2_$account")
        assertTrue(file.exists())

        MlsSecureStorage.wipeAccount(context, account)

        assertFalse(namespacedFile(account).exists())
    }

    /** A wipe names one account; another signed-in account keeps its identity. */
    @Test
    fun wipeLeavesAnotherAccountsStoreAlone() {
        val alice = namespace("wipe-alice")
        val bob = namespace("wipe-bob")
        seedPreferencesFile("mls_secure_storage_v2_$alice")
        seedPreferencesFile("mls_secure_storage_v2_$bob")

        MlsSecureStorage.wipeAccount(context, alice)

        assertFalse(namespacedFile(alice).exists())
        assertTrue(namespacedFile(bob).exists())
    }

    /**
     * The legacy store was shared by every account on a pre-split install. When
     * its claim cannot be read, ownership is unknown — and destroying another
     * account's MLS identity and block list is not recoverable, while leaving a
     * leftover behind is. So the wipe fails closed and still finishes the
     * namespaced store, which is unambiguously this account's.
     */
    @Test
    fun legacyStoreThatCannotBeProvedOursIsLeftAlone() {
        val account = namespace("wipe-legacy-unreadable")
        val legacy = seedPreferencesFile("mls_secure_storage")
        seedPreferencesFile("mls_secure_storage_v2_$account")

        MlsSecureStorage.wipeAccount(context, account)

        assertTrue(
            "an unreadable claim must not authorise destroying the shared store",
            legacy.exists()
        )
        assertFalse(
            "the namespaced store is unambiguously ours and must still go",
            namespacedFile(account).exists()
        )
    }

    /**
     * The leftover a logout is asked to erase, and the branch that actually
     * destroys the shared store.
     *
     * Every account that has completed a post-split launch records a claim, so
     * an unclaimed store is what the *previous* install left behind — and on a
     * platform whose credential store outlives the app container, what a
     * reinstall would otherwise re-adopt.
     */
    @Test
    fun unclaimedLegacyStoreIsWiped() {
        val account = namespace("wipe-legacy-unclaimed")
        val legacy = seedPreferencesFile("mls_secure_storage")
        seedPreferencesFile("mls_secure_storage_v2_$account")

        MlsSecureStorage.wipeAccount(context, account, true) {
            LegacyStoreAdoption.LegacyClaim.Absent
        }

        assertFalse("an unclaimed store is this logout's to erase", legacy.exists())
        assertFalse(namespacedFile(account).exists())
    }

    /** The ordinary logout: this account inherited it, so erasing it is its own. */
    @Test
    fun ourOwnClaimAuthorisesWipingTheLegacyStore() {
        val account = namespace("wipe-legacy-ours")
        val legacy = seedPreferencesFile("mls_secure_storage")
        seedPreferencesFile("mls_secure_storage_v2_$account")

        MlsSecureStorage.wipeAccount(context, account, true) {
            LegacyStoreAdoption.LegacyClaim.Owned(account)
        }

        assertFalse(legacy.exists())
        assertFalse(namespacedFile(account).exists())
    }

    /**
     * Another account's claim makes the shared store theirs. Wiping it would
     * destroy an MLS identity, sessions, and a block list that have nothing to
     * do with this logout — while this account's own store still goes.
     */
    @Test
    fun aForeignClaimLeavesTheLegacyStoreAlone() {
        val account = namespace("wipe-legacy-foreign")
        val stranger = namespace("wipe-legacy-stranger")
        val legacy = seedPreferencesFile("mls_secure_storage")
        seedPreferencesFile("mls_secure_storage_v2_$account")

        MlsSecureStorage.wipeAccount(context, account, true) {
            LegacyStoreAdoption.LegacyClaim.Owned(stranger)
        }

        assertTrue(
            "another account's claim is not ours to erase",
            legacy.exists()
        )
        assertFalse(namespacedFile(account).exists())
    }

    /**
     * The legacy store goes first. Read-through and the claim both live there,
     * so wiping the namespaced store first and then failing would leave an
     * install that re-promotes, on its next launch, exactly the material it was
     * asked to destroy.
     *
     * Observed from inside the claim read, which is the one point known to run
     * before the legacy delete and therefore before anything else.
     */
    @Test
    fun theLegacyStoreIsWipedBeforeTheNamespacedOne() {
        val account = namespace("wipe-order")
        seedPreferencesFile("mls_secure_storage")
        seedPreferencesFile("mls_secure_storage_v2_$account")
        var namespacedStillPresent = false

        MlsSecureStorage.wipeAccount(context, account, true) {
            namespacedStillPresent = namespacedFile(account).exists()
            LegacyStoreAdoption.LegacyClaim.Absent
        }

        assertTrue(
            "the namespaced store must still be intact when the legacy one is judged",
            namespacedStillPresent
        )
        assertFalse(namespacedFile(account).exists())
    }

    /** Nothing to erase is not a failure, and a retry after one must be safe. */
    @Test
    fun wipeIsIdempotentAndToleratesAMissingStore() {
        val account = namespace("wipe-idempotent")
        seedPreferencesFile("mls_secure_storage_v2_$account")

        MlsSecureStorage.wipeAccount(context, account)
        MlsSecureStorage.wipeAccount(context, account)
        MlsSecureStorage.wipeAccount(context, namespace("never-existed"))
    }
}
