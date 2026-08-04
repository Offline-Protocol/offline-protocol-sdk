package com.offlineprotocol

import android.content.Context
import android.content.SharedPreferences
import android.util.Base64
import org.junit.Assert.assertEquals
import org.junit.Assert.assertFalse
import org.junit.Assert.assertNotNull
import org.junit.Assert.assertNull
import org.junit.Assert.assertThrows
import org.junit.Assert.assertTrue
import org.junit.Test
import org.junit.runner.RunWith
import org.robolectric.RobolectricTestRunner
import org.robolectric.RuntimeEnvironment
import org.robolectric.annotation.Config
import uniffi.offline_protocol.MlsStorageException
import java.util.UUID

/**
 * Covers what [MlsSecureStorage.delete] promises when the legacy copy will not
 * go: nothing hands that key back afterwards.
 *
 * Driven through the injectable-store constructor with plain
 * `SharedPreferences`. `EncryptedSharedPreferences` needs a real
 * AndroidKeyStore and cannot run under a JVM harness — but none of the
 * adoption, tombstone, or read-through logic is encryption-aware, so the same
 * code runs either way. What this does *not* cover is the encrypted store
 * itself; that is [MlsSecureStorageWipeTest]'s file-level territory and, below
 * it, review.
 */
@RunWith(RobolectricTestRunner::class)
@Config(sdk = [24])
class MlsSecureStorageTest {
    private val context: Context
        get() = RuntimeEnvironment.getApplication()

    private fun namespace(label: String): String =
        StorageNamespace.account("secure-store-test-${UUID.randomUUID()}", label)

    /**
     * A store whose commits can be made to fail, separately for writes and
     * removals.
     *
     * The two are distinguishable because only [MlsSecureStorage.store] issues
     * a `putString` — `remove` touches the entry and the index alone — which is
     * what lets the double-fault case fail a tombstone write while leaving the
     * namespaced removal working.
     */
    private class FailingPreferences(
        private val delegate: SharedPreferences
    ) : SharedPreferences by delegate {
        var failRemovals = false
        var failWrites = false

        /**
         * Key *prefix* whose reads throw. Scoped to a prefix rather than the
         * whole store because the tombstone read has to fail while the
         * namespaced miss immediately before it succeeds — both are the same
         * store.
         */
        var failReadsWithPrefix: String? = null

        override fun getString(key: String?, defValue: String?): String? {
            val prefix = failReadsWithPrefix
            if (prefix != null && key != null && key.startsWith(prefix)) {
                throw IllegalStateException("refusing reads of $key")
            }
            return delegate.getString(key, defValue)
        }

        override fun edit(): SharedPreferences.Editor = Editor(delegate.edit())

        private inner class Editor(
            private val inner: SharedPreferences.Editor
        ) : SharedPreferences.Editor by inner {
            private var wrote = false

            override fun putString(key: String?, value: String?): SharedPreferences.Editor {
                wrote = true
                inner.putString(key, value)
                return this
            }

            override fun putStringSet(
                key: String?,
                values: MutableSet<String>?
            ): SharedPreferences.Editor {
                inner.putStringSet(key, values)
                return this
            }

            override fun remove(key: String?): SharedPreferences.Editor {
                inner.remove(key)
                return this
            }

            override fun commit(): Boolean = when {
                wrote && failWrites -> false
                !wrote && failRemovals -> false
                else -> inner.commit()
            }
        }
    }

    private fun preferences(name: String): FailingPreferences =
        FailingPreferences(context.getSharedPreferences(name, Context.MODE_PRIVATE))

    /** Writes an entry in the on-disk shape [MlsSecureStorage.store] produces. */
    private fun seed(
        preferences: SharedPreferences,
        keyType: String,
        keyId: String,
        data: ByteArray
    ) {
        val index = preferences.getStringSet("index:$keyType", null) ?: emptySet()
        preferences.edit()
            .putString("$keyType:$keyId", Base64.encodeToString(data, Base64.NO_WRAP))
            .putStringSet("index:$keyType", HashSet(index).apply { add(keyId) })
            .commit()
    }

    private fun rawEntry(preferences: SharedPreferences, keyType: String, keyId: String): String? =
        preferences.getString("$keyType:$keyId", null)

    private fun tombstone(
        preferences: SharedPreferences,
        keyType: String,
        keyId: String
    ): String? = rawEntry(
        preferences,
        LegacyStoreAdoption.TOMBSTONE_KEY_TYPE,
        LegacyStoreAdoption.tombstoneKeyId(keyType, keyId)
    )

    /** An adopted store over seeded legacy key material. */
    private fun adoptedStore(
        namespaced: FailingPreferences,
        legacy: FailingPreferences,
        account: String
    ): MlsSecureStorage {
        seed(legacy, "key_package", "peer-1", "pkg".toByteArray())
        val storage = MlsSecureStorage(account, namespaced, legacy)
        assertTrue(
            "read-through must be on for these cases to mean anything",
            LegacyStoreAdoption.allowsReadThrough(storage.legacyAdoption)
        )
        return storage
    }

    /**
     * The removal of the legacy copy can fail on its own. Reporting that by
     * throwing is not available — core treats a storage delete as fatal almost
     * everywhere (OpenMLS aborts Welcome processing and every commit merge on
     * one) and has no retry — so the key is tombstoned, and the delete keeps
     * its promise: nothing hands that material back.
     */
    @Test
    fun aLegacyCopyThatWillNotDeleteIsTombstoned() {
        val account = namespace("tombstone")
        val namespaced = preferences("ns-$account")
        val legacy = preferences("legacy-$account")
        val storage = adoptedStore(namespaced, legacy, account)
        assertNotNull(storage.load("key_package", "peer-1"))

        legacy.failRemovals = true
        storage.delete("key_package", "peer-1")

        assertNull(
            "read-through must not hand back a key the caller deleted",
            storage.load("key_package", "peer-1")
        )
        // The corpse is still there — suppression, not deletion, is what makes
        // this safe, so the test would pass for the wrong reason otherwise.
        assertNotNull(rawEntry(legacy, "key_package", "peer-1"))
        assertNotNull(tombstone(namespaced, "key_package", "peer-1"))
    }

    /**
     * listKeys unions the legacy index, so a suppressed entry must be filtered
     * out of it too: advertising a key that cannot be loaded would send core
     * looking for material this store has promised to withhold.
     */
    @Test
    fun aTombstonedKeyIsNotListed() {
        val account = namespace("tombstone-list")
        val namespaced = preferences("ns-$account")
        val legacy = preferences("legacy-$account")
        val storage = adoptedStore(namespaced, legacy, account)
        storage.store("key_package", "peer-2", listOf<UByte>(2u))

        legacy.failRemovals = true
        storage.delete("key_package", "peer-1")

        assertEquals(listOf("peer-2"), storage.listKeys("key_package"))
    }

    /**
     * A tombstone suppresses read-through, not the key. Re-storing under the
     * same id must be readable again — otherwise a key package that failed to
     * clean up would poison its own id for the life of the install.
     */
    @Test
    fun aTombstoneDoesNotShadowAFreshWrite() {
        val account = namespace("tombstone-rewrite")
        val namespaced = preferences("ns-$account")
        val legacy = preferences("legacy-$account")
        val storage = adoptedStore(namespaced, legacy, account)

        legacy.failRemovals = true
        storage.delete("key_package", "peer-1")
        storage.store("key_package", "peer-1", listOf<UByte>(9u, 9u))

        assertEquals(listOf<UByte>(9u, 9u), storage.load("key_package", "peer-1"))
        assertEquals(listOf("peer-1"), storage.listKeys("key_package"))
    }

    /**
     * The failure that stranded the copy may be transient. A later read retries
     * the removal, and once it lands the tombstone has nothing to suppress and
     * goes with it.
     */
    @Test
    fun aTombstoneIsRetiredOnceTheLegacyCopyGoes() {
        val account = namespace("tombstone-heal")
        val namespaced = preferences("ns-$account")
        val legacy = preferences("legacy-$account")
        val storage = adoptedStore(namespaced, legacy, account)

        legacy.failRemovals = true
        storage.delete("key_package", "peer-1")
        legacy.failRemovals = false

        assertNull(storage.load("key_package", "peer-1"))

        assertNull(
            "the retry must actually remove the corpse",
            rawEntry(legacy, "key_package", "peer-1")
        )
        assertNull(
            "a tombstone with nothing left to suppress must be retired",
            tombstone(namespaced, "key_package", "peer-1")
        )
    }

    /**
     * A tombstone read that fails is not evidence that a tombstone exists.
     * Suppressing read-through on it is right and costs a read-through until
     * the store recovers; *deleting* the legacy copy on it is not, and would
     * destroy the last copy of a key that was legitimately inheritable — on a
     * first post-upgrade launch, possibly the signing identity.
     */
    @Test
    fun anUnreadableTombstoneSuppressesButDoesNotDestroy() {
        val account = namespace("tombstone-unreadable")
        val namespaced = preferences("ns-$account")
        val legacy = preferences("legacy-$account")
        val storage = adoptedStore(namespaced, legacy, account)

        namespaced.failReadsWithPrefix = LegacyStoreAdoption.TOMBSTONE_KEY_TYPE

        assertNull(
            "an unprovable tombstone must still suppress read-through",
            storage.load("key_package", "peer-1")
        )
        assertNotNull(
            "a read that merely failed must not retire the legacy copy",
            rawEntry(legacy, "key_package", "peer-1")
        )

        // And once the tombstone is readable again, the key it never tombstoned
        // is inheritable exactly as before — the suppression was recoverable.
        namespaced.failReadsWithPrefix = null
        assertNotNull(storage.load("key_package", "peer-1"))
    }

    /**
     * The one case that must throw. With the legacy copy alive and the
     * tombstone unwritable there is no way to keep the delete's promise, and a
     * store failing both is failing everything else too.
     */
    @Test
    fun aDeleteThatCanNeitherRemoveNorTombstoneIsReported() {
        val account = namespace("tombstone-double-fault")
        val namespaced = preferences("ns-$account")
        val legacy = preferences("legacy-$account")
        val storage = adoptedStore(namespaced, legacy, account)

        legacy.failRemovals = true
        namespaced.failWrites = true

        val error = assertThrows(MlsStorageException.DeleteFailed::class.java) {
            storage.delete("key_package", "peer-1")
        }
        assertTrue(
            "the message must name the tombstone half: ${error.message}",
            error.message.orEmpty().contains("could not tombstone")
        )
        assertTrue(
            "the message must also name the removal that stranded the copy, " +
                "which is what an operator reading the crash needs: ${error.message}",
            error.message.orEmpty().contains("commit rejected")
        )
    }

    /** Tombstones are the provider's bookkeeping, never key material. */
    @Test
    fun tombstonesAreNeverExposedAsKeyMaterial() {
        val account = namespace("tombstone-hidden")
        val namespaced = preferences("ns-$account")
        val legacy = preferences("legacy-$account")
        val storage = adoptedStore(namespaced, legacy, account)

        legacy.failRemovals = true
        storage.delete("key_package", "peer-1")

        assertNull(
            storage.load(
                LegacyStoreAdoption.TOMBSTONE_KEY_TYPE,
                LegacyStoreAdoption.tombstoneKeyId("key_package", "peer-1")
            )
        )
        assertTrue(storage.listKeys(LegacyStoreAdoption.TOMBSTONE_KEY_TYPE).isEmpty())
    }

    /**
     * The happy path still clears both copies outright — a tombstone is the
     * fallback, not the mechanism.
     */
    @Test
    fun aSuccessfulDeleteRemovesBothCopiesAndLeavesNoTombstone() {
        val account = namespace("tombstone-none")
        val namespaced = preferences("ns-$account")
        val legacy = preferences("legacy-$account")
        val storage = adoptedStore(namespaced, legacy, account)
        assertNotNull(storage.load("key_package", "peer-1"))

        storage.delete("key_package", "peer-1")

        assertNull(storage.load("key_package", "peer-1"))
        assertNull(rawEntry(legacy, "key_package", "peer-1"))
        assertNull(tombstone(namespaced, "key_package", "peer-1"))
        assertFalse(storage.listKeys("key_package").contains("peer-1"))
    }
}
