package com.offlineprotocol

import android.content.Context
import android.util.Base64
import androidx.security.crypto.EncryptedSharedPreferences
import androidx.security.crypto.MasterKey
import uniffi.offline_protocol.MlsStorageException
import uniffi.offline_protocol.MlsStorageProvider

/**
 * Built-in MLS storage using Android EncryptedSharedPreferences.
 *
 * This implementation:
 * - Uses AES-256 encryption with hardware-backed keystore when available
 * - Provides atomic operations for thread safety
 * - Maintains an index for efficient key listing
 */
class MlsSecureStorage(context: Context) : MlsStorageProvider {
    
    private val masterKey = MasterKey.Builder(context)
        .setKeyScheme(MasterKey.KeyScheme.AES256_GCM)
        .build()
    
    private val sharedPreferences = EncryptedSharedPreferences.create(
        context,
        PREFS_FILE_NAME,
        masterKey,
        EncryptedSharedPreferences.PrefKeyEncryptionScheme.AES256_SIV,
        EncryptedSharedPreferences.PrefValueEncryptionScheme.AES256_GCM
    )
    
    companion object {
        private const val PREFS_FILE_NAME = "mls_secure_storage"
        private const val INDEX_PREFIX = "index:"
        // Global lock to ensure index consistency across instances/threads
        private val LOCK = Any()
    }
    
    /**
     * Stores data securely using EncryptedSharedPreferences.
     */
    override fun store(keyType: String, keyId: String, data: List<UByte>) {
        synchronized(LOCK) {
            try {
                val key = makeKey(keyType, keyId)
                val byteArray = data.map { it.toByte() }.toByteArray()
                val encoded = Base64.encodeToString(byteArray, Base64.NO_WRAP)
                
                sharedPreferences.edit().apply {
                    putString(key, encoded)
                    
                    // Update the index for this key type
                    val indexKey = "$INDEX_PREFIX$keyType"
                    // Note: getStringSet returns a reference that shouldn't be modified directly
                    // We must create a new set to avoid ConcurrentModificationException or side effects
                    val existingKeys = sharedPreferences.getStringSet(indexKey, null) ?: emptySet()
                    val updatedKeys = HashSet(existingKeys)
                    updatedKeys.add(keyId)
                    
                    putStringSet(indexKey, updatedKeys)
                    
                    // Use commit() instead of apply() to ensure synchronous persistence if needed, 
                    // or stick to apply() if speed is preferred. 
                    // Since we are synchronized, apply() is safe for the in-memory state which 
                    // SharedPreferences maintains.
                    apply() 
                }
            } catch (e: Exception) {
                throw MlsStorageException.StoreFailed("EncryptedSharedPreferences store failed: ${e.message}")
            }
        }
    }
    
    /**
     * Loads data from EncryptedSharedPreferences.
     */
    override fun load(keyType: String, keyId: String): List<UByte>? {
        // No lock needed for simple read
        return try {
            val key = makeKey(keyType, keyId)
            val encoded = sharedPreferences.getString(key, null) ?: return null
            val decoded = Base64.decode(encoded, Base64.NO_WRAP)
            decoded.map { it.toUByte() }
        } catch (e: Exception) {
            throw MlsStorageException.LoadFailed("EncryptedSharedPreferences load failed: ${e.message}")
        }
    }
    
    /**
     * Deletes data from EncryptedSharedPreferences.
     */
    override fun delete(keyType: String, keyId: String) {
        synchronized(LOCK) {
            try {
                val key = makeKey(keyType, keyId)
                
                sharedPreferences.edit().apply {
                    remove(key)
                    
                    // Update the index for this key type
                    val indexKey = "$INDEX_PREFIX$keyType"
                    val existingKeys = sharedPreferences.getStringSet(indexKey, null) ?: emptySet()
                    if (existingKeys.contains(keyId)) {
                        val updatedKeys = HashSet(existingKeys)
                        updatedKeys.remove(keyId)
                        putStringSet(indexKey, updatedKeys)
                    }
                    
                    apply()
                }
            } catch (e: Exception) {
                throw MlsStorageException.DeleteFailed("EncryptedSharedPreferences delete failed: ${e.message}")
            }
        }
    }
    
    /**
     * Lists all key IDs for a given key type.
     */
    override fun listKeys(keyType: String): List<String> {
        synchronized(LOCK) {
            return try {
                val indexKey = "$INDEX_PREFIX$keyType"
                sharedPreferences.getStringSet(indexKey, emptySet())?.toList() ?: emptyList()
            } catch (e: Exception) {
                throw MlsStorageException.LoadFailed("EncryptedSharedPreferences listKeys failed: ${e.message}")
            }
        }
    }
    
    private fun makeKey(keyType: String, keyId: String): String = "$keyType:$keyId"
}
