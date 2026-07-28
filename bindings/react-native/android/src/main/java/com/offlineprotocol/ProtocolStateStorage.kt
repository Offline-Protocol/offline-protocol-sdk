package com.offlineprotocol

import android.content.Context
import android.util.AtomicFile
import uniffi.offline_protocol.MlsStorageException
import uniffi.offline_protocol.ProtocolStateStorageProvider
import java.io.DataInputStream
import java.io.File
import java.io.FileInputStream
import java.io.FileOutputStream
import java.security.MessageDigest

/**
 * On-disk record format shared by every built-in protocol-state provider.
 *
 * Filenames are a fixed-length lowercase hex digest rather than an encoding of
 * the key itself. An encoding cannot be a correct filesystem key: it is
 * case-sensitive (so `AAG` and `AAa` name the same file on a case-insensitive
 * volume, and one record silently overwrites the other) and its length grows
 * with the key (so a long-but-valid protocol id overruns `NAME_MAX`). A digest
 * is fixed-length, lowercase, and collision-free in practice.
 *
 * Because a digest is one-way, the exact key cannot be recovered from the name,
 * so each record carries it in a header instead. That also makes every file
 * independently attributable and lets a read verify it opened the record it
 * asked for.
 *
 *     bytes 0..3   magic "OPS1"
 *     bytes 4..5   key_type length, big-endian u16
 *     bytes 6..7   key_id length, big-endian u16
 *     then         key_type UTF-8, key_id UTF-8, value bytes
 *
 * Keep this format, its limits, and the golden vectors in
 * `ProtocolStateStorageTest` in sync with the iOS and Python providers.
 */
internal object ProtocolStateRecord {
    val MAGIC = byteArrayOf(0x4F, 0x50, 0x53, 0x31) // "OPS1"

    /** Longest accepted key_type / key_id, in UTF-8 bytes. */
    const val MAX_COMPONENT_BYTES = 4096

    /**
     * Provider ceiling on a record's value. Deliberately a generous superset of
     * core's `MAX_PROTOCOL_STATE_RECORD_BYTES` (4 MiB) plus its seal envelope,
     * so this never rejects a record the SDK legitimately wrote — it exists to
     * bound allocation for a file the SDK never wrote.
     */
    const val MAX_VALUE_BYTES = 8 * 1024 * 1024

    /** Largest file the reader will pull into memory. */
    const val MAX_FILE_BYTES = 8 + 2 * MAX_COMPONENT_BYTES + MAX_VALUE_BYTES

    /** Longest possible header, bounding the partial read `listKeys` does. */
    const val MAX_HEADER_BYTES = 8 + 2 * MAX_COMPONENT_BYTES

    /**
     * Ceiling on entries one `listKeys` will open. Core caps every category far
     * below this; a directory holding more has been tampered with.
     */
    const val MAX_LISTED_KEYS = 65_536

    private const val HEX = "0123456789abcdef"

    fun digest(vararg components: String): String {
        val sha = MessageDigest.getInstance("SHA-256")
        for (component in components) {
            sha.update(component.toByteArray(Charsets.UTF_8))
            sha.update(0)
        }
        val bytes = sha.digest()
        return buildString(bytes.size * 2) {
            bytes.forEach { byte ->
                val value = byte.toInt() and 0xff
                append(HEX[value ushr 4])
                append(HEX[value and 0x0f])
            }
        }
    }

    fun typeDirectoryName(keyType: String): String = "t_" + digest(keyType)

    fun entryName(keyType: String, keyId: String): String = "k_" + digest(keyType, keyId)

    fun frame(keyType: String, keyId: String, value: ByteArray): ByteArray {
        val typeBytes = keyType.toByteArray(Charsets.UTF_8)
        val idBytes = keyId.toByteArray(Charsets.UTF_8)
        if (typeBytes.size > MAX_COMPONENT_BYTES || idBytes.size > MAX_COMPONENT_BYTES) {
            throw MlsStorageException.StoreFailed(
                "protocol-state key exceeds $MAX_COMPONENT_BYTES bytes"
            )
        }
        if (value.size > MAX_VALUE_BYTES) {
            throw MlsStorageException.StoreFailed(
                "protocol-state record is ${value.size} bytes, over the $MAX_VALUE_BYTES byte limit"
            )
        }

        val out = ByteArray(8 + typeBytes.size + idBytes.size + value.size)
        MAGIC.copyInto(out)
        out[4] = (typeBytes.size ushr 8).toByte()
        out[5] = (typeBytes.size and 0xff).toByte()
        out[6] = (idBytes.size ushr 8).toByte()
        out[7] = (idBytes.size and 0xff).toByte()
        typeBytes.copyInto(out, 8)
        idBytes.copyInto(out, 8 + typeBytes.size)
        value.copyInto(out, 8 + typeBytes.size + idBytes.size)
        return out
    }

    /** Key a framed record belongs to, plus where its value starts. */
    data class Header(val keyType: String, val keyId: String, val valueOffset: Int)

    /** `null` means the bytes are not a record this SDK wrote. */
    fun parseHeader(bytes: ByteArray): Header? {
        if (bytes.size < 8) {
            return null
        }
        for (index in MAGIC.indices) {
            if (bytes[index] != MAGIC[index]) {
                return null
            }
        }
        val typeLen = (bytes[4].toInt() and 0xff) shl 8 or (bytes[5].toInt() and 0xff)
        val idLen = (bytes[6].toInt() and 0xff) shl 8 or (bytes[7].toInt() and 0xff)
        if (typeLen > MAX_COMPONENT_BYTES || idLen > MAX_COMPONENT_BYTES) {
            return null
        }
        if (bytes.size < 8 + typeLen + idLen) {
            return null
        }
        val keyType = String(bytes, 8, typeLen, Charsets.UTF_8)
        val keyId = String(bytes, 8 + typeLen, idLen, Charsets.UTF_8)
        return Header(keyType, keyId, 8 + typeLen + idLen)
    }
}

/**
 * App-container storage for restartable delivery and protocol state.
 *
 * The root lives under noBackupFilesDir, so neither uninstall/reinstall nor
 * Android Auto Backup can resurrect an old outbox or retry lifecycle.
 *
 * Values are opaque bytes and are written verbatim: the SDK seals the
 * categories that can carry message plaintext or media key material before
 * handing them over, so they arrive as ciphertext. Do not inspect, re-encode,
 * or truncate them.
 */
class AppContainerProtocolStateStorage(
    context: Context,
    accountNamespace: String
) : ProtocolStateStorageProvider {
    private val root = File(
        File(context.noBackupFilesDir, SCHEMA_DIRECTORY),
        StorageNamespace.requireAccount(accountNamespace)
    ).also {
        // Check `isDirectory` rather than `mkdirs()`'s return value: it reports
        // false when a concurrent creator won the race, which is success, not
        // failure.
        it.mkdirs()
        if (!it.isDirectory) {
            throw MlsStorageException.StoreFailed(
                "Failed to create protocol-state directory: ${it.absolutePath}"
            )
        }
    }

    override fun store(keyType: String, keyId: String, data: List<UByte>) {
        val framed = ProtocolStateRecord.frame(
            keyType,
            keyId,
            data.map { it.toByte() }.toByteArray()
        )

        synchronized(LOCK) {
            val directory = typeDirectory(keyType)
            directory.mkdirs()
            if (!directory.isDirectory) {
                throw MlsStorageException.StoreFailed(
                    "Failed to create protocol-state type directory"
                )
            }

            val atomicFile = AtomicFile(entryFile(keyType, keyId))
            var output: FileOutputStream? = null
            try {
                output = atomicFile.startWrite()
                output.write(framed)
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

            // Stat before reading. A record over the ceiling cannot have been
            // written through `store`, so it is corrupt or tampered — removing
            // it keeps a poison file from being re-examined on every boot, and
            // keeps the read itself from becoming an unbounded allocation.
            val physicalBytes = maxOf(file.length(), legacyBackup.length())
            if (physicalBytes > ProtocolStateRecord.MAX_FILE_BYTES) {
                throw discard(atomicFile, "record is $physicalBytes bytes, over the ceiling")
            }

            val raw = try {
                atomicFile.readFully()
            } catch (error: Exception) {
                throw MlsStorageException.LoadFailed(
                    "Failed to load protocol state: ${error.message}"
                )
            }

            // Re-check post-read: the stat above can race a concurrent writer.
            if (raw.size > ProtocolStateRecord.MAX_FILE_BYTES) {
                throw discard(atomicFile, "record is ${raw.size} bytes, over the ceiling")
            }

            val header = ProtocolStateRecord.parseHeader(raw)
            if (header == null || header.keyType != keyType || header.keyId != keyId) {
                // Malformed framing, or a name that resolves to some other
                // record: either way this is not the entry that was asked for.
                throw discard(atomicFile, "record framing does not name $keyType/$keyId")
            }
            return raw.copyOfRange(header.valueOffset, raw.size).map { it.toUByte() }
        }
    }

    /**
     * Removes a record that can never be read and returns the exception that
     * says so.
     *
     * CorruptedData, not a silent null: absence and destruction are different
     * answers upstream. The SDK settles a destroyed record — the application is
     * holding the message id `sendMessage` returned for it — while absence is
     * simply nothing to restore, reported to no one.
     */
    private fun discard(atomicFile: AtomicFile, reason: String): MlsStorageException {
        atomicFile.delete()
        return MlsStorageException.CorruptedData(
            "Dropped unreadable protocol-state record: $reason"
        )
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
                // `Files.newDirectoryStream` needs API 26 and minSdk here is 24,
                // so the name array is unavoidable — but names are a fixed 66
                // characters and only MAX_LISTED_KEYS of them are ever opened,
                // which is where the real cost would be.
                val names = directory.list() ?: return emptyList()
                val keys = ArrayList<String>()
                for (name in names) {
                    if (keys.size >= ProtocolStateRecord.MAX_LISTED_KEYS) {
                        break
                    }
                    if (!name.startsWith("k_") || name.endsWith(".new")) {
                        continue
                    }
                    val header = readHeader(File(directory, name.removeSuffix(".bak")))
                        ?: continue
                    if (header.keyType == keyType && !keys.contains(header.keyId)) {
                        keys.add(header.keyId)
                    }
                }
                keys.sorted()
            } catch (error: Exception) {
                throw MlsStorageException.LoadFailed(
                    "Failed to list protocol-state keys: ${error.message}"
                )
            }
        }
    }

    /**
     * Reads only a record's header. Enumeration must stay bounded even when the
     * container has been tampered with, so this never pulls in a whole file.
     */
    private fun readHeader(file: File): ProtocolStateRecord.Header? {
        val target = if (file.exists()) file else File("${file.path}.bak")
        if (!target.isFile) {
            return null
        }
        return try {
            DataInputStream(FileInputStream(target)).use { stream ->
                val prefix = ByteArray(
                    minOf(target.length(), ProtocolStateRecord.MAX_HEADER_BYTES.toLong()).toInt()
                )
                stream.readFully(prefix)
                ProtocolStateRecord.parseHeader(prefix)
            }
        } catch (_: Exception) {
            null
        }
    }

    private fun typeDirectory(keyType: String): File =
        File(root, ProtocolStateRecord.typeDirectoryName(keyType))

    private fun entryFile(keyType: String, keyId: String): File =
        File(typeDirectory(keyType), ProtocolStateRecord.entryName(keyType, keyId))

    companion object {
        private const val SCHEMA_DIRECTORY = "offline-protocol/protocol-state-v1"
        private val LOCK = Any()
    }
}
