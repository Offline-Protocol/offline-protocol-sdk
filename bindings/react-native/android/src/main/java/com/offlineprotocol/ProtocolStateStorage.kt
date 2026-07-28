package com.offlineprotocol

import android.content.Context
import android.util.AtomicFile
import android.util.Base64
import uniffi.offline_protocol.MlsStorageException
import uniffi.offline_protocol.ProtocolStateStorageProvider
import java.io.File
import java.io.FileOutputStream

/**
 * App-container storage for restartable delivery and protocol state.
 *
 * The root lives under noBackupFilesDir, so neither uninstall/reinstall nor
 * Android Auto Backup can resurrect an old outbox or retry lifecycle.
 */
class AppContainerProtocolStateStorage(
    context: Context,
    accountNamespace: String
) : ProtocolStateStorageProvider {
    private val root = File(
        File(context.noBackupFilesDir, SCHEMA_DIRECTORY),
        StorageNamespace.requireAccount(accountNamespace)
    ).also {
        if (!it.exists() && !it.mkdirs()) {
            throw MlsStorageException.StoreFailed(
                "Failed to create protocol-state directory: ${it.absolutePath}"
            )
        }
    }

    override fun store(keyType: String, keyId: String, data: List<UByte>) {
        synchronized(LOCK) {
            val directory = typeDirectory(keyType)
            if (!directory.exists() && !directory.mkdirs()) {
                throw MlsStorageException.StoreFailed(
                    "Failed to create protocol-state type directory"
                )
            }

            val atomicFile = AtomicFile(entryFile(keyType, keyId))
            var output: FileOutputStream? = null
            try {
                output = atomicFile.startWrite()
                output.write(data.map { it.toByte() }.toByteArray())
                atomicFile.finishWrite(output)
                output = null
            } catch (error: Exception) {
                output?.let { atomicFile.failWrite(it) }
                throw MlsStorageException.StoreFailed(
                    "Failed to persist protocol state: ${error.message}"
                )
            }
        }
    }

    override fun load(keyType: String, keyId: String): List<UByte>? {
        synchronized(LOCK) {
            val file = entryFile(keyType, keyId)
            val atomicFile = AtomicFile(file)
            val legacyBackup = File("${file.path}.bak")
            if (!file.exists() && !legacyBackup.exists()) {
                return null
            }
            return try {
                atomicFile.readFully().map { it.toUByte() }
            } catch (error: Exception) {
                throw MlsStorageException.LoadFailed(
                    "Failed to load protocol state: ${error.message}"
                )
            }
        }
    }

    override fun delete(keyType: String, keyId: String) {
        synchronized(LOCK) {
            try {
                AtomicFile(entryFile(keyType, keyId)).delete()
            } catch (error: Exception) {
                throw MlsStorageException.DeleteFailed(
                    "Failed to delete protocol state: ${error.message}"
                )
            }
        }
    }

    override fun listKeys(keyType: String): List<String> {
        synchronized(LOCK) {
            val directory = typeDirectory(keyType)
            if (!directory.exists()) {
                return emptyList()
            }
            return try {
                directory.listFiles()
                    ?.asSequence()
                    ?.filter { it.isFile && !it.name.endsWith(".new") }
                    ?.map { it.name.removeSuffix(".bak") }
                    ?.mapNotNull(::decodeComponent)
                    ?.distinct()
                    ?.sorted()
                    ?.toList()
                    ?: emptyList()
            } catch (error: Exception) {
                throw MlsStorageException.LoadFailed(
                    "Failed to list protocol-state keys: ${error.message}"
                )
            }
        }
    }

    private fun typeDirectory(keyType: String): File =
        File(root, encodeComponent(keyType))

    private fun entryFile(keyType: String, keyId: String): File =
        File(typeDirectory(keyType), encodeComponent(keyId))

    private fun encodeComponent(value: String): String =
        "k_" + Base64.encodeToString(
            value.toByteArray(Charsets.UTF_8),
            Base64.URL_SAFE or Base64.NO_WRAP or Base64.NO_PADDING
        )

    private fun decodeComponent(value: String): String? {
        if (!value.startsWith("k_")) {
            return null
        }
        return try {
            val decoded = Base64.decode(
                value.removePrefix("k_"),
                Base64.URL_SAFE or Base64.NO_WRAP or Base64.NO_PADDING
            )
            decoded.toString(Charsets.UTF_8)
        } catch (_: IllegalArgumentException) {
            null
        }
    }

    companion object {
        private const val SCHEMA_DIRECTORY = "offline-protocol/protocol-state-v1"
        private val LOCK = Any()
    }
}
