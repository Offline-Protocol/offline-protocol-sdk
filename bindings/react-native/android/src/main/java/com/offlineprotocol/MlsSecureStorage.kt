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
class MlsSecureStorage internal constructor(
    accountNamespace: String,
    private val sharedPreferences: SharedPreferences,
    /**
     * The pre-namespace preferences file, opened only when it already exists.
     * Opening it unconditionally would create an empty one on every fresh
     * install and make "is there anything to inherit?" unanswerable.
     */
    private val legacyPreferences: SharedPreferences?
) : MlsStorageProvider {

    private val namespace = StorageNamespace.requireAccount(accountNamespace)

    /**
     * Outcome of the one-time legacy-store adoption, for the caller to surface.
     * [LegacyStoreAdoption.Decision.Conflict] in particular must not pass
     * silently: this account is starting from a fresh identity.
     */
    internal val legacyAdoption: LegacyStoreAdoption.Decision = resolveLegacyAdoption()

    /**
     * The production constructor: both stores are `EncryptedSharedPreferences`
     * over the androidx master key.
     *
     * The stores are injectable above so the adoption, tombstone, and
     * read-through logic can be exercised under a JVM harness.
     * `EncryptedSharedPreferences` needs a real AndroidKeyStore and cannot run
     * there — but none of that logic is encryption-aware, so a plain
     * `SharedPreferences` pair drives exactly the same code. What the seam does
     * *not* fake is the store this class actually ships with; keep the
     * production path above this comment trivial enough to read.
     */
    constructor(
        context: Context,
        accountNamespace: String,
        adoptLegacyStore: Boolean = true
    ) : this(
        accountNamespace,
        openNamespacedPreferences(context, StorageNamespace.requireAccount(accountNamespace)),
        if (adoptLegacyStore) openLegacyPreferences(context) else null
    )

    companion object {
        private const val TAG = "MlsSecureStorage"
        private const val PREFS_FILE_PREFIX = "mls_secure_storage_v2_"
        private const val LEGACY_PREFS_FILE_NAME = "mls_secure_storage"
        private const val INDEX_PREFIX = "index:"
        // Global lock to ensure index consistency across instances/threads
        private val LOCK = Any()

        /**
         * Value written for a tombstone. Only its *presence* is the signal —
         * nothing reads the bytes back — so it stays one byte rather than
         * restating the key.
         */
        private val TOMBSTONE_VALUE = byteArrayOf(1)

        private fun masterKey(context: Context): MasterKey =
            MasterKey.Builder(context)
                .setKeyScheme(MasterKey.KeyScheme.AES256_GCM)
                .build()

        private fun openPreferences(context: Context, name: String): SharedPreferences =
            EncryptedSharedPreferences.create(
                context,
                name,
                masterKey(context),
                EncryptedSharedPreferences.PrefKeyEncryptionScheme.AES256_SIV,
                EncryptedSharedPreferences.PrefValueEncryptionScheme.AES256_GCM
            )

        private fun openNamespacedPreferences(
            context: Context,
            namespace: String
        ): SharedPreferences = openPreferences(context, "$PREFS_FILE_PREFIX$namespace")

        private fun openLegacyPreferences(context: Context): SharedPreferences? {
            if (!legacyPreferencesFile(context).exists()) {
                return null
            }
            return try {
                openPreferences(context, LEGACY_PREFS_FILE_NAME)
            } catch (error: Exception) {
                // A legacy file we cannot open (rotated master key, corruption)
                // is not inheritable. Report it rather than silently rotating.
                Log.e(TAG, "Legacy secure store exists but could not be opened", error)
                null
            }
        }

        /**
         * Erases every secure-store entry this SDK holds for one account.
         *
         * Deliberately static: it must run when no instance exists — after
         * `destroy`, on logout — and constructing one would be actively wrong,
         * because the constructor *claims* the legacy store as a side effect. A
         * wipe that built a provider first could therefore claim a store on
         * behalf of an account that is being erased.
         *
         * The legacy store goes first. Read-through and the claim both live
         * there, so wiping the namespaced store first and then failing would
         * leave an install that re-promotes, on its next launch, exactly the
         * material it was asked to destroy. In the other order a partial wipe
         * leaves only the namespaced store, which the next wipe removes and
         * which nothing re-populates.
         *
         * Whether the legacy store may be destroyed at all is
         * [LegacyStoreAdoption.shouldWipeLegacy]'s decision: it was shared by
         * every account on a pre-split install, so another account's claim makes
         * it off-limits.
         *
         * Both phases are attempted even if the first fails, and the first error
         * is rethrown afterwards — a failure on the legacy store must not strand
         * the namespaced one, which is where everything written since the
         * storage split lives. Idempotent: a caller that gets an error should
         * call again.
         *
         * The androidx [MasterKey] is never touched: it is shared with every
         * other account's store, and with any other library in the process that
         * uses the default master key alias.
         *
         * @param wipeLegacyStore off only for tests that must not touch the
         *   shared pre-namespace store.
         */
        @JvmStatic
        fun wipeAccount(
            context: Context,
            accountNamespace: String,
            wipeLegacyStore: Boolean = true
        ) = wipeAccount(context, accountNamespace, wipeLegacyStore, ::readLegacyClaim)

        /**
         * [wipeAccount] with the legacy claim read by [readClaim].
         *
         * The reader is injectable so the branch that *destroys* the shared
         * pre-namespace store can be exercised. It cannot be reached otherwise
         * under a JVM harness: the real reader opens an
         * `EncryptedSharedPreferences`, which needs a real AndroidKeyStore, so
         * every claim reads as `Unreadable` and the wipe always fails closed —
         * the one outcome that was already covered, and only by accident.
         */
        internal fun wipeAccount(
            context: Context,
            accountNamespace: String,
            wipeLegacyStore: Boolean,
            readClaim: (Context) -> LegacyStoreAdoption.LegacyClaim
        ) {
            val namespace = StorageNamespace.requireAccount(accountNamespace)
            synchronized(LOCK) {
                var firstError: Exception? = null
                if (wipeLegacyStore) {
                    try {
                        wipeLegacy(context, namespace, readClaim)
                    } catch (error: Exception) {
                        firstError = error
                    }
                }
                try {
                    deletePreferences(context, "$PREFS_FILE_PREFIX$namespace")
                } catch (error: Exception) {
                    if (firstError == null) {
                        firstError = error
                    }
                }
                firstError?.let {
                    throw MlsStorageException.DeleteFailed(
                        "Failed to wipe secure storage: ${it.message}"
                    )
                }
            }
        }

        private fun wipeLegacy(
            context: Context,
            namespace: String,
            readClaim: (Context) -> LegacyStoreAdoption.LegacyClaim
        ) {
            if (!legacyPreferencesFile(context).exists()) {
                return
            }
            val claim = readClaim(context)
            if (!LegacyStoreAdoption.shouldWipeLegacy(claim, namespace)) {
                Log.i(
                    TAG,
                    "Leaving the pre-namespace secure store in place: it is not " +
                        "this account's to erase"
                )
                return
            }
            deletePreferences(context, LEGACY_PREFS_FILE_NAME)
        }

        /**
         * Reads the legacy store's claim, keeping "not recorded" and "could not
         * be read" apart — see [LegacyStoreAdoption.LegacyClaim].
         *
         * A store that will not open is reported as unreadable rather than
         * unclaimed, so the wipe fails closed. The failure is not always
         * permanent: a rotated master key makes it so, but a locked keystore
         * makes it transient, and the two are indistinguishable here. Treating
         * either as "unclaimed" would let a transient failure destroy another
         * account's identity and block list.
         */
        private fun readLegacyClaim(context: Context): LegacyStoreAdoption.LegacyClaim = try {
            val legacy = openPreferences(context, LEGACY_PREFS_FILE_NAME)
            val encoded = legacy.getString(
                "${LegacyStoreAdoption.CLAIM_KEY_TYPE}:${LegacyStoreAdoption.CLAIM_KEY_ID}",
                null
            )
            if (encoded == null) {
                LegacyStoreAdoption.LegacyClaim.Absent
            } else {
                LegacyStoreAdoption.LegacyClaim.of(Base64.decode(encoded, Base64.NO_WRAP))
            }
        } catch (error: Exception) {
            Log.w(TAG, "Could not read the legacy secure store claim; not wiping it", error)
            LegacyStoreAdoption.LegacyClaim.Unreadable
        }

        private fun legacyPreferencesFile(context: Context): File =
            File(context.dataDir, "shared_prefs/$LEGACY_PREFS_FILE_NAME.xml")

        private fun deletePreferences(context: Context, name: String) {
            // Clears the instance `ContextImpl` caches under this name as well
            // as the file. Unlinking the file alone would leave a cached
            // `SharedPreferences` that recreates it on its next commit.
            // API 24, which is this module's minSdk.
            context.deleteSharedPreferences(name)

            // Verify rather than trust the return value, which is false both
            // when the file was absent and when the delete failed.
            val remaining = File(context.dataDir, "shared_prefs/$name.xml")
            if (remaining.exists() && !remaining.delete()) {
                throw MlsStorageException.DeleteFailed(
                    "Secure store $name is still present after wipe"
                )
            }
        }
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
     *
     * Each primitive below takes the lock, but this *compound* read-then-promote
     * is not atomic against [delete], and deliberately so. Interleaved, they
     * would resurrect key material: this method could observe a miss in the
     * namespaced store, read the legacy value, and then promote it after a
     * concurrent delete had already removed both copies — defeating the very
     * guarantee [delete] documents.
     *
     * That is unreachable because the SDK is the only caller and serialises
     * every storage operation behind its own mutex: `OfflineProtocol`'s methods
     * take `&mut self` and the UniFFI wrapper holds them under one lock, so no
     * two provider calls overlap. Widening the lock to cover the whole compound
     * operation would mean holding it across a legacy-store read on every miss,
     * which is the common path during an upgrade. If a second caller is ever
     * given this provider, that trade has to be revisited.
     *
     * [wipeAccount] is not that second caller. It touches the same store but
     * only ever for an account with no live instance — the bridge refuses to
     * wipe the one it is running — so it cannot interleave with a promotion on
     * the same preferences file.
     *
     * A tombstoned key reads as absent without consulting the legacy store at
     * all: its copy there outlived a delete, and promoting it would resurrect
     * key material the caller was told was gone.
     */
    override fun load(keyType: String, keyId: String): List<UByte>? {
        if (LegacyStoreAdoption.isReservedEntry(keyType)) {
            return null
        }

        // No lock needed for simple reads of the namespaced store; the
        // promotion below takes it.
        read(sharedPreferences, keyType, keyId)?.let { return it.map { byte -> byte.toUByte() } }

        val legacy = readThroughStore(keyType) ?: return null
        val tombstone = tombstoneState(keyType, keyId)
        if (tombstone.suppressesReadThrough) {
            if (tombstone.allowsRemovalRetry) {
                // Opportunistic heal: the removal that failed may succeed now,
                // which is the only thing that retires a tombstone. Gated on a
                // *confirmed* tombstone — a read that merely failed must not
                // delete a copy that may still be inheritable.
                retryTombstonedRemoval(legacy, keyType, keyId)
            }
            return null
        }
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
     *
     * A delete that left the legacy copy in place would let read-through
     * resurrect key material the caller believes is gone. When that removal
     * fails, the key is tombstoned rather than reported: see
     * [LegacyStoreAdoption.TOMBSTONE_KEY_TYPE] for why this cannot be signalled
     * by throwing. The delete has still done what it promised — nothing will
     * hand that key back — so it returns successfully.
     *
     * Only a *double* fault throws: a legacy copy that will not delete and a
     * namespaced store that will not record the tombstone leaves no way to keep
     * the promise, and a store failing both is failing everything else too.
     */
    override fun delete(keyType: String, keyId: String) {
        synchronized(LOCK) {
            remove(sharedPreferences, keyType, keyId)
            val legacy = readThroughStore(keyType) ?: return
            try {
                remove(legacy, keyType, keyId)
            } catch (error: Exception) {
                Log.w(
                    TAG,
                    "Failed to delete inherited entry for $keyType; tombstoning it " +
                        "so read-through cannot resurrect it",
                    error
                )
                tombstone(keyType, keyId, error)
                return
            }
            clearTombstone(keyType, keyId)
        }
    }

    /**
     * Lists all key IDs for a given key type, unioned across the adopted legacy
     * store so a not-yet-promoted entry is still discoverable — except where a
     * tombstone says that entry is a corpse, which must not be advertised as a
     * key that can be loaded.
     */
    override fun listKeys(keyType: String): List<String> {
        if (LegacyStoreAdoption.isReservedEntry(keyType)) {
            return emptyList()
        }
        synchronized(LOCK) {
            val keys = LinkedHashSet(index(sharedPreferences, keyType))
            readThroughStore(keyType)?.let { legacy ->
                try {
                    keys.addAll(
                        index(legacy, keyType).filterNot {
                            tombstoneState(keyType, it).suppressesReadThrough
                        }
                    )
                } catch (error: Exception) {
                    Log.w(TAG, "Failed to list inherited entries for $keyType", error)
                }
            }
            return keys.toList()
        }
    }

    // -- tombstones ----------------------------------------------------------

    /**
     * Records that a legacy copy survived its deletion.
     *
     * @param cause the removal failure this stands in for, folded into the
     *   thrown message so a double fault names both halves.
     */
    private fun tombstone(keyType: String, keyId: String, cause: Exception) {
        try {
            store(
                sharedPreferences,
                LegacyStoreAdoption.TOMBSTONE_KEY_TYPE,
                LegacyStoreAdoption.tombstoneKeyId(keyType, keyId),
                TOMBSTONE_VALUE
            )
        } catch (error: Exception) {
            throw MlsStorageException.DeleteFailed(
                "Delete left an inherited copy of $keyType in place " +
                    "(${cause.message}) and could not tombstone it: ${error.message}"
            )
        }
    }

    /**
     * What the namespaced store says about this key's legacy copy.
     *
     * A read that throws is [LegacyStoreAdoption.TombstoneState.UNREADABLE],
     * which fails closed as far as *reading* goes: read-through cannot be
     * proven safe, and suppressing a legitimate inherited entry costs an
     * identity rotation while resurrecting a consumed key costs forward
     * secrecy. It deliberately stops short of authorising the removal retry —
     * see [LegacyStoreAdoption.TombstoneState]. Near-unreachable in practice:
     * the namespaced read in [load] runs first against the same store and would
     * have thrown.
     */
    private fun tombstoneState(
        keyType: String,
        keyId: String
    ): LegacyStoreAdoption.TombstoneState = try {
        val recorded = read(
            sharedPreferences,
            LegacyStoreAdoption.TOMBSTONE_KEY_TYPE,
            LegacyStoreAdoption.tombstoneKeyId(keyType, keyId)
        ) != null
        if (recorded) {
            LegacyStoreAdoption.TombstoneState.RECORDED
        } else {
            LegacyStoreAdoption.TombstoneState.ABSENT
        }
    } catch (error: Exception) {
        Log.w(
            TAG,
            "Could not read the tombstone for $keyType; suppressing read-through " +
                "without retiring the legacy copy",
            error
        )
        LegacyStoreAdoption.TombstoneState.UNREADABLE
    }

    /** Best-effort retry of the legacy removal a tombstone stands in for. */
    private fun retryTombstonedRemoval(
        legacy: SharedPreferences,
        keyType: String,
        keyId: String
    ) {
        synchronized(LOCK) {
            try {
                remove(legacy, keyType, keyId)
            } catch (error: Exception) {
                return
            }
            clearTombstone(keyType, keyId)
        }
    }

    /**
     * Retires a tombstone once the legacy copy is genuinely gone.
     *
     * Best effort: a tombstone that outlives its corpse only costs the
     * inherited entry it suppresses, and there is nothing left to resurrect.
     */
    private fun clearTombstone(keyType: String, keyId: String) {
        try {
            remove(
                sharedPreferences,
                LegacyStoreAdoption.TOMBSTONE_KEY_TYPE,
                LegacyStoreAdoption.tombstoneKeyId(keyType, keyId)
            )
        } catch (error: Exception) {
            Log.d(TAG, "Failed to clear the tombstone for $keyType", error)
        }
    }

    // -- legacy adoption -----------------------------------------------------

    /**
     * Resolves — and, when the legacy store is unclaimed, records and then
     * *verifies* — this account's right to inherit it.
     *
     * The claim is read back rather than assumed, because a write that failed
     * silently would leave the store looking unclaimed to the next account,
     * which would then adopt the same identity. See
     * [LegacyStoreAdoption.confirmClaim].
     *
     * The whole probe → claim → read-back sequence runs under [LOCK], because
     * reading it back is not on its own enough to make inheritance exclusive.
     * The read back closes a write that silently failed, and a second account
     * claiming between our probe and our write. It does not close two accounts
     * interleaving like this:
     *
     *     A.readClaim() -> null        B.readClaim() -> null
     *     A.store(nsA)
     *     A.readClaim() -> nsA  => adopt
     *                              B.store(nsB)
     *                              B.readClaim() -> nsB  => adopt
     *
     * Both adopt, both promote the same MLS signing identity, and each ends up
     * holding the other's sessions and group state — which is the outcome the
     * claim exists to prevent, arriving silently. The invariant is "at most one
     * account holds a verified claim", and an unsynchronised read-modify-write
     * does not provide it. A process-wide lock does: two accounts on one device
     * are two objects in one process, and there is no cross-process case for a
     * single application's credential store.
     */
    private fun resolveLegacyAdoption(): LegacyStoreAdoption.Decision {
        val legacy = legacyPreferences ?: return LegacyStoreAdoption.Decision.None
        synchronized(LOCK) {
            return resolveLegacyAdoptionLocked(legacy)
        }
    }

    private fun resolveLegacyAdoptionLocked(
        legacy: SharedPreferences
    ): LegacyStoreAdoption.Decision {
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
        if (LegacyStoreAdoption.isReservedEntry(keyType)) {
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
