package com.offlineprotocol

import android.content.Context
import android.content.SharedPreferences
import android.util.Base64
import android.util.Log
import androidx.security.crypto.EncryptedSharedPreferences
import androidx.security.crypto.MasterKey
import uniffi.offline_protocol.MlsStorageException
import uniffi.offline_protocol.MlsStorageProvider
import java.io.File

/**
 * Built-in MLS storage using Android EncryptedSharedPreferences.
 *
 * This implementation:
 * - Uses AES-256 encryption with hardware-backed keystore when available
 * - Provides atomic operations for thread safety
 * - Maintains an index for efficient key listing
 * - Adopts the pre-namespace store on upgrade (see [LegacyStoreAdoption])
 *
 * Writes are durable before they return. Core treats a successful [store] as
 * persisted — most sharply for the per-install protocol-state record key, which
 * it immediately starts sealing app-container records under — so every mutation
 * uses `commit()` (synchronous, reports failure) rather than `apply()`
 * (in-memory now, disk later, failure invisible). A crash in an `apply()`
 * window would leave durable ciphertext whose key was never written.
 */
class MlsSecureStorage(
    context: Context,
    accountNamespace: String,
    adoptLegacyStore: Boolean = true
) : MlsStorageProvider {

    private val masterKey = MasterKey.Builder(context)
        .setKeyScheme(MasterKey.KeyScheme.AES256_GCM)
        .build()

    private val namespace = StorageNamespace.requireAccount(accountNamespace)

    private val sharedPreferences = EncryptedSharedPreferences.create(
        context,
        "$PREFS_FILE_PREFIX$namespace",
        masterKey,
        EncryptedSharedPreferences.PrefKeyEncryptionScheme.AES256_SIV,
        EncryptedSharedPreferences.PrefValueEncryptionScheme.AES256_GCM
    )

    /**
     * The pre-namespace preferences file, opened only when it already exists.
     * Opening it unconditionally would create an empty one on every fresh
     * install and make "is there anything to inherit?" unanswerable.
     */
    private val legacyPreferences: SharedPreferences? =
        if (adoptLegacyStore) openLegacyPreferences(context) else null

    /**
     * Outcome of the one-time legacy-store adoption, for the caller to surface.
     * [LegacyStoreAdoption.Decision.Conflict] in particular must not pass
     * silently: this account is starting from a fresh identity.
     */
    internal val legacyAdoption: LegacyStoreAdoption.Decision = resolveLegacyAdoption()

    companion object {
        private const val TAG = "MlsSecureStorage"
        private const val PREFS_FILE_PREFIX = "mls_secure_storage_v2_"
        private const val LEGACY_PREFS_FILE_NAME = "mls_secure_storage"
        private const val INDEX_PREFIX = "index:"
        // Global lock to ensure index consistency across instances/threads
        private val LOCK = Any()
    }

    /**
     * Stores data securely in the namespaced store.
     */
    override fun store(keyType: String, keyId: String, data: List<UByte>) {
        synchronized(LOCK) {
            store(sharedPreferences, keyType, keyId, data.map { it.toByte() }.toByteArray())
        }
    }

    /**
     * Loads data, falling through to the adopted legacy store on a miss and
     * promoting what it finds, so an upgraded install keeps its identity,
     * sessions, and TOFU pins without a bulk migration pass.
     */
    override fun load(keyType: String, keyId: String): List<UByte>? {
        // No lock needed for simple reads of the namespaced store; the
        // promotion below takes it.
        read(sharedPreferences, keyType, keyId)?.let { return it.map { byte -> byte.toUByte() } }

        val legacy = readThroughStore(keyType) ?: return null
        val inherited = read(legacy, keyType, keyId) ?: return null
        synchronized(LOCK) {
            // Best-effort promotion: a failed copy still returns the value, it
            // just costs another read-through next launch.
            try {
                store(sharedPreferences, keyType, keyId, inherited)
            } catch (error: Exception) {
                Log.w(TAG, "Failed to promote inherited entry for $keyType", error)
            }
        }
        return inherited.map { it.toUByte() }
    }

    /**
     * Deletes data from the namespaced store, and from the legacy store too.
     * A delete that left the legacy copy in place would let read-through
     * resurrect key material the caller believes is gone.
     */
    override fun delete(keyType: String, keyId: String) {
        synchronized(LOCK) {
            remove(sharedPreferences, keyType, keyId)
            readThroughStore(keyType)?.let { legacy ->
                try {
                    remove(legacy, keyType, keyId)
                } catch (error: Exception) {
                    Log.w(TAG, "Failed to delete inherited entry for $keyType", error)
                }
            }
        }
    }

    /**
     * Lists all key IDs for a given key type, unioned across the adopted legacy
     * store so a not-yet-promoted entry is still discoverable.
     */
    override fun listKeys(keyType: String): List<String> {
        synchronized(LOCK) {
            val keys = LinkedHashSet(index(sharedPreferences, keyType))
            readThroughStore(keyType)?.let { legacy ->
                try {
                    keys.addAll(index(legacy, keyType))
                } catch (error: Exception) {
                    Log.w(TAG, "Failed to list inherited entries for $keyType", error)
                }
            }
            return keys.toList()
        }
    }

    // -- legacy adoption -----------------------------------------------------

    private fun openLegacyPreferences(context: Context): SharedPreferences? {
        val file = File(context.dataDir, "shared_prefs/$LEGACY_PREFS_FILE_NAME.xml")
        if (!file.exists()) {
            return null
        }
        return try {
            EncryptedSharedPreferences.create(
                context,
                LEGACY_PREFS_FILE_NAME,
                masterKey,
                EncryptedSharedPreferences.PrefKeyEncryptionScheme.AES256_SIV,
                EncryptedSharedPreferences.PrefValueEncryptionScheme.AES256_GCM
            )
        } catch (error: Exception) {
            // A legacy file we cannot open (rotated master key, corruption) is
            // not inheritable. Report it rather than silently rotating.
            Log.e(TAG, "Legacy secure store exists but could not be opened", error)
            null
        }
    }

    /**
     * Resolves — and, when the legacy store is unclaimed, records and then
     * *verifies* — this account's right to inherit it.
     *
     * The claim is read back rather than assumed, because a write that failed
     * silently would leave the store looking unclaimed to the next account,
     * which would then adopt the same identity. See
     * [LegacyStoreAdoption.confirmClaim].
     */
    private fun resolveLegacyAdoption(): LegacyStoreAdoption.Decision {
        val legacy = legacyPreferences ?: return LegacyStoreAdoption.Decision.None

        val decision = LegacyStoreAdoption.decide(readClaim(legacy), namespace)
        if (decision !is LegacyStoreAdoption.Decision.Adopt) {
            if (decision is LegacyStoreAdoption.Decision.Conflict) {
                Log.e(
                    TAG,
                    "Legacy secure store already belongs to another account; this " +
                        "account starts from a fresh MLS identity and cannot " +
                        "decrypt its old sessions."
                )
            }
            return decision
        }

        val confirmed = try {
            store(
                legacy,
                LegacyStoreAdoption.CLAIM_KEY_TYPE,
                LegacyStoreAdoption.CLAIM_KEY_ID,
                namespace.toByteArray(Charsets.UTF_8)
            )
            LegacyStoreAdoption.confirmClaim(readClaim(legacy), namespace)
        } catch (error: Exception) {
            Log.w(TAG, "Failed to claim the legacy secure store", error)
            LegacyStoreAdoption.Decision.ClaimUnverified
        }

        when (confirmed) {
            is LegacyStoreAdoption.Decision.Adopt ->
                Log.i(TAG, "Adopting the pre-namespace secure store for this account")
            is LegacyStoreAdoption.Decision.ClaimUnverified -> Log.e(
                TAG,
                "Could not record this account's claim on the legacy secure " +
                    "store, so it was not adopted: another account could " +
                    "otherwise inherit the same MLS identity. This account " +
                    "starts from a fresh identity."
            )
            is LegacyStoreAdoption.Decision.Conflict -> Log.e(
                TAG,
                "Legacy secure store was claimed by another account concurrently; " +
                    "this account starts from a fresh MLS identity."
            )
            else -> Unit
        }
        return confirmed
    }

    /**
     * The claim recorded in the legacy store, or null when absent or
     * unreadable. A failed read is deliberately not distinguished from an
     * absent claim on the way *in* (both mean "looks unclaimed") but is on the
     * way back *out*, where it means the claim is unproven.
     */
    private fun readClaim(legacy: SharedPreferences): String? = try {
        read(
            legacy,
            LegacyStoreAdoption.CLAIM_KEY_TYPE,
            LegacyStoreAdoption.CLAIM_KEY_ID
        )?.toString(Charsets.UTF_8)
    } catch (error: Exception) {
        Log.w(TAG, "Failed to read the legacy secure store claim", error)
        null
    }

    /**
     * The legacy store to consult for [keyType], or null when read-through is
     * off (no legacy store, another account claimed it, or this account could
     * not prove its own claim).
     */
    private fun readThroughStore(keyType: String): SharedPreferences? {
        if (!LegacyStoreAdoption.allowsReadThrough(legacyAdoption)) {
            return null
        }
        if (LegacyStoreAdoption.isClaimEntry(keyType)) {
            return null
        }
        return legacyPreferences
    }

    // -- EncryptedSharedPreferences primitives -------------------------------

    private fun store(
        preferences: SharedPreferences,
        keyType: String,
        keyId: String,
        data: ByteArray
    ) {
        try {
            val encoded = Base64.encodeToString(data, Base64.NO_WRAP)

            // Update the index for this key type.
            // Note: getStringSet returns a reference that shouldn't be modified directly.
            // We must create a new set to avoid ConcurrentModificationException or side effects
            val indexKey = "$INDEX_PREFIX$keyType"
            val existingKeys = preferences.getStringSet(indexKey, null) ?: emptySet()
            val updatedKeys = HashSet(existingKeys)
            updatedKeys.add(keyId)

            val committed = preferences.edit()
                .putString(makeKey(keyType, keyId), encoded)
                .putStringSet(indexKey, updatedKeys)
                .commit()
            if (!committed) {
                throw MlsStorageException.StoreFailed(
                    "EncryptedSharedPreferences store failed: commit rejected for $keyType"
                )
            }
        } catch (e: MlsStorageException) {
            throw e
        } catch (e: Exception) {
            throw MlsStorageException.StoreFailed("EncryptedSharedPreferences store failed: ${e.message}")
        }
    }

    private fun read(
        preferences: SharedPreferences,
        keyType: String,
        keyId: String
    ): ByteArray? {
        return try {
            val encoded = preferences.getString(makeKey(keyType, keyId), null) ?: return null
            Base64.decode(encoded, Base64.NO_WRAP)
        } catch (e: Exception) {
            throw MlsStorageException.LoadFailed("EncryptedSharedPreferences load failed: ${e.message}")
        }
    }

    private fun remove(preferences: SharedPreferences, keyType: String, keyId: String) {
        try {
            val editor = preferences.edit().remove(makeKey(keyType, keyId))

            // Update the index for this key type
            val indexKey = "$INDEX_PREFIX$keyType"
            val existingKeys = preferences.getStringSet(indexKey, null) ?: emptySet()
            if (existingKeys.contains(keyId)) {
                val updatedKeys = HashSet(existingKeys)
                updatedKeys.remove(keyId)
                editor.putStringSet(indexKey, updatedKeys)
            }

            // commit(), not apply(): a delete that is only in memory can be
            // undone by a crash, resurrecting key material the caller
            // believes is gone.
            if (!editor.commit()) {
                throw MlsStorageException.DeleteFailed(
                    "EncryptedSharedPreferences delete failed: commit rejected for $keyType"
                )
            }
        } catch (e: MlsStorageException) {
            throw e
        } catch (e: Exception) {
            throw MlsStorageException.DeleteFailed("EncryptedSharedPreferences delete failed: ${e.message}")
        }
    }

    private fun index(preferences: SharedPreferences, keyType: String): List<String> {
        return try {
            preferences.getStringSet("$INDEX_PREFIX$keyType", emptySet())?.toList() ?: emptyList()
        } catch (e: Exception) {
            throw MlsStorageException.LoadFailed("EncryptedSharedPreferences listKeys failed: ${e.message}")
        }
    }

    private fun makeKey(keyType: String, keyId: String): String = "$keyType:$keyId"
}
