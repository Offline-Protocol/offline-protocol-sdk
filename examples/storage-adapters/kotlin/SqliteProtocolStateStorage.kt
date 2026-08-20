package com.offlineprotocol.examples

import android.content.Context
import android.database.sqlite.SQLiteDatabase
import android.database.sqlite.SQLiteOpenHelper
import uniffi.offline_protocol.MlsStorageException
import uniffi.offline_protocol.ProtocolStateStorageProvider

/**
 * A SQLite-backed protocol-state adapter, as a worked reference.
 *
 * This is the whole "bring your own backend" path in one file. It exists to
 * prove the claim rather than to be copied verbatim: the SDK ships a working
 * file-backed provider, and an application only reaches for something like
 * this when it already has a store it wants its data inside.
 *
 * What the SDK guarantees to an adapter:
 *  - Values are opaque bytes. Records whose category requires sealing are
 *    encrypted before [store] is called, so this class never sees document
 *    content or message plaintext and cannot weaken the at-rest posture. It
 *    also means values must round-trip as a ByteArray, never through String.
 *  - `keyType` is a namespace and `keyId` an identifier within it. Both are
 *    opaque; the data layer composes ids like `space/doc/0000000000000001`,
 *    so do not treat them as file paths.
 *
 * What the adapter owes: the contract checked by `runStorageConformance`.
 * Green is the definition of "this backend is supported".
 *
 * Logout: an application that points documents here MUST call
 * `DataStore.wipeAll()` on logout. `wipePersistedState` clears the default
 * provider's directory, which this database is not inside. Stop the protocol
 * first: with the engine running and sessions live, the peer's next version
 * offer recreates and refills every document it wiped.
 */
class SqliteProtocolStateStorage(
    context: Context,
    databaseName: String = "offline_protocol_state.db",
) : ProtocolStateStorageProvider {

    private val helper = object : SQLiteOpenHelper(
        context.applicationContext,
        databaseName,
        null,
        VERSION,
    ) {
        override fun onCreate(db: SQLiteDatabase) {
            db.execSQL(
                """
                CREATE TABLE $TABLE (
                    key_type TEXT NOT NULL,
                    key_id   TEXT NOT NULL,
                    value    BLOB NOT NULL,
                    PRIMARY KEY (key_type, key_id)
                )
                """.trimIndent()
            )
        }

        override fun onUpgrade(db: SQLiteDatabase, oldVersion: Int, newVersion: Int) {
            // No schema history yet. A real adapter migrates here; dropping
            // the table would discard the SDK's crash-recovery state.
        }

        override fun onConfigure(db: SQLiteDatabase) {
            // WAL keeps restore reads from blocking background writes.
            db.enableWriteAheadLogging()
        }
    }

    private val lock = Any()

    override fun store(keyType: String, keyId: String, data: ByteArray) {
        try {
            synchronized(lock) {
                // REPLACE, not INSERT: a second store under the same key must
                // overwrite. A backend that keeps the first value is the defect
                // `store_overwrites` exists to catch, and it stays invisible
                // until data is stale.
                helper.writableDatabase.execSQL(
                    "INSERT OR REPLACE INTO $TABLE (key_type, key_id, value) VALUES (?, ?, ?)",
                    arrayOf<Any>(keyType, keyId, data),
                )
            }
        } catch (e: Exception) {
            throw MlsStorageException.StoreFailed()
        }
    }

    override fun load(keyType: String, keyId: String): ByteArray? {
        return try {
            synchronized(lock) {
                helper.readableDatabase.rawQuery(
                    "SELECT value FROM $TABLE WHERE key_type = ? AND key_id = ?",
                    arrayOf(keyType, keyId),
                ).use { cursor ->
                    // A missing row is null, not an error: the SDK asks for
                    // records that legitimately do not exist yet on every
                    // launch.
                    if (cursor.moveToFirst()) cursor.getBlob(0) else null
                }
            }
        } catch (e: Exception) {
            // LoadFailed, not CorruptedData. CorruptedData is a permanent
            // verdict the SDK settles messages on; a failed read is not that.
            throw MlsStorageException.LoadFailed()
        }
    }

    override fun delete(keyType: String, keyId: String) {
        try {
            synchronized(lock) {
                helper.writableDatabase.delete(
                    TABLE,
                    "key_type = ? AND key_id = ?",
                    arrayOf(keyType, keyId),
                )
            }
        } catch (e: Exception) {
            throw MlsStorageException.DeleteFailed()
        }
        // Deleting an absent key is deliberately not an error: the data layer
        // removes folded delta records a crash may already have taken.
    }

    override fun listKeys(keyType: String): List<String> {
        return try {
            synchronized(lock) {
                helper.readableDatabase.rawQuery(
                    "SELECT key_id FROM $TABLE WHERE key_type = ?",
                    arrayOf(keyType),
                ).use { cursor ->
                    val keys = ArrayList<String>(cursor.count)
                    while (cursor.moveToNext()) {
                        keys.add(cursor.getString(0))
                    }
                    keys
                }
            }
        } catch (e: Exception) {
            throw MlsStorageException.LoadFailed()
        }
    }

    fun close() = helper.close()

    private companion object {
        const val TABLE = "protocol_state"
        const val VERSION = 1
    }
}
