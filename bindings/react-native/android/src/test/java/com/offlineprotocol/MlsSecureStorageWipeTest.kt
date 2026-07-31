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
