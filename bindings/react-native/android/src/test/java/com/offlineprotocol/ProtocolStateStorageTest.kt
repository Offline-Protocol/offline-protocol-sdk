package com.offlineprotocol

import android.content.Context
import org.junit.Assert.assertEquals
import org.junit.Assert.assertFalse
import org.junit.Assert.assertNull
import org.junit.Assert.assertThrows
import org.junit.Assert.assertTrue
import org.junit.Test
import org.junit.runner.RunWith
import org.robolectric.RobolectricTestRunner
import org.robolectric.RuntimeEnvironment
import uniffi.offline_protocol.MlsStorageException
import java.io.File
import java.io.RandomAccessFile
import java.util.UUID

@RunWith(RobolectricTestRunner::class)
class ProtocolStateStorageTest {
    private val context: Context
        get() = RuntimeEnvironment.getApplication()

    private fun namespace(label: String): String =
        StorageNamespace.account("protocol-state-test-${UUID.randomUUID()}", label)

    // ByteArray has identity equality, so compare stable numeric values.
    private fun loadedBytes(
        storage: AppContainerProtocolStateStorage,
        keyType: String,
        keyId: String
    ): List<Int>? = storage.load(keyType, keyId)?.map { it.toInt() and 0xff }

    private fun entryFile(account: String, keyType: String, keyId: String): File =
        File(
            File(
                File(context.noBackupFilesDir, "offline-protocol/protocol-state-v1"),
                account
            ),
            "${ProtocolStateRecord.typeDirectoryName(keyType)}/" +
                ProtocolStateRecord.entryName(keyType, keyId)
        )

    @Test
    fun roundTripOverwriteListingAndIdempotentDelete() {
        val storage = AppContainerProtocolStateStorage(context, namespace("round-trip"))

        storage.store("pending/messages", "peer with punctuation", byteArrayOf(0, 1, -1))
        assertEquals(
            listOf(0, 1, 255),
            loadedBytes(storage, "pending/messages", "peer with punctuation")
        )
        assertEquals(
            listOf("peer with punctuation"),
            storage.listKeys("pending/messages")
        )

        storage.store("pending/messages", "peer with punctuation", byteArrayOf(4, 5))
        assertEquals(
            listOf(4, 5),
            loadedBytes(storage, "pending/messages", "peer with punctuation")
        )

        storage.delete("pending/messages", "peer with punctuation")
        storage.delete("pending/messages", "peer with punctuation")
        assertNull(loadedBytes(storage, "pending/messages", "peer with punctuation"))
        assertEquals(emptyList<String>(), storage.listKeys("pending/messages"))
    }

    @Test
    fun accountNamespacesDoNotShareState() {
        val alice = AppContainerProtocolStateStorage(context, namespace("alice"))
        val bob = AppContainerProtocolStateStorage(context, namespace("bob"))

        alice.store("outbox", "message-1", byteArrayOf(1, 2, 3))

        assertEquals(listOf(1, 2, 3), loadedBytes(alice, "outbox", "message-1"))
        assertNull(loadedBytes(bob, "outbox", "message-1"))
    }

    @Test
    fun restartReopensTheSameInstallRoot() {
        val account = namespace("restart")
        val first = AppContainerProtocolStateStorage(context, account)
        first.store("outbox", "message-1", byteArrayOf(7, 8, 9))

        val restarted = AppContainerProtocolStateStorage(context, account)

        assertEquals(listOf(7, 8, 9), loadedBytes(restarted, "outbox", "message-1"))
    }

    // -- filesystem-key safety ----------------------------------------------

    /**
     * "AAG" and "AAa" differ only in the case of one base64url character, so an
     * encoding-based filename gives them the same name on a case-insensitive
     * volume and one record silently overwrites the other. A digest name cannot
     * collide this way.
     */
    @Test
    fun caseFoldingIdsAreDistinctRecords() {
        val storage = AppContainerProtocolStateStorage(context, namespace("case-fold"))

        storage.store("outbox", "AAG", byteArrayOf(1))
        storage.store("outbox", "AAa", byteArrayOf(2))

        assertEquals(listOf(1), loadedBytes(storage, "outbox", "AAG"))
        assertEquals(listOf(2), loadedBytes(storage, "outbox", "AAa"))
        assertEquals(listOf("AAG", "AAa"), storage.listKeys("outbox"))
    }

    /**
     * Core accepts user ids up to 256 bytes. Base64 of 190 bytes already
     * overruns the 255-byte NAME_MAX most filesystems enforce; a digest name is
     * a fixed 66 characters no matter how long the key is.
     */
    @Test
    fun maximumLengthIdsRoundTrip() {
        val storage = AppContainerProtocolStateStorage(context, namespace("long-ids"))
        val longId = "u".repeat(256)

        storage.store("outbox", longId, byteArrayOf(9))

        assertEquals(listOf(9), loadedBytes(storage, "outbox", longId))
        assertEquals(listOf(longId), storage.listKeys("outbox"))
        assertEquals(66, ProtocolStateRecord.entryName("outbox", longId).length)
    }

    @Test
    fun everyEntryNameIsFixedLengthAndLowercase() {
        for (keyId in listOf("", "AAG", "x".repeat(4096), "péer/ id")) {
            val name = ProtocolStateRecord.entryName("outbox", keyId)
            assertEquals(66, name.length)
            assertEquals(name.lowercase(), name)
        }
    }

    // -- framing -------------------------------------------------------------

    /**
     * Golden vector. The iOS and Python providers must produce these exact bytes
     * and names for the same input, or a record written by one platform is
     * unreadable by another sharing a container.
     */
    @Test
    fun framingGoldenVector() {
        val framed = ProtocolStateRecord.frame("outbox", "m-1", byteArrayOf(0xAA.toByte(), 0xBB.toByte()))

        assertEquals(
            listOf(
                0x4F, 0x50, 0x53, 0x31, // "OPS1"
                0x00, 0x06, // key_type length
                0x00, 0x03, // key_id length
                0x6F, 0x75, 0x74, 0x62, 0x6F, 0x78, // "outbox"
                0x6D, 0x2D, 0x31, // "m-1"
                0xAA, 0xBB
            ),
            framed.map { it.toInt() and 0xff }
        )

        assertEquals(
            "t_d5fac01c82279b8b061df80b3c312942e2ce27a41a48b1b7479ff07ad5a6198d",
            ProtocolStateRecord.typeDirectoryName("outbox")
        )
        assertEquals(
            "k_db5fcc2398ef2863d4269a61be6ea2de1f80d2889f34670c9a57c79cbe8058a1",
            ProtocolStateRecord.entryName("outbox", "m-1")
        )

        val header = ProtocolStateRecord.parseHeader(framed)!!
        assertEquals("outbox", header.keyType)
        assertEquals("m-1", header.keyId)
        assertEquals(17, header.valueOffset)
    }

    @Test
    fun emptyValueRoundTrips() {
        val storage = AppContainerProtocolStateStorage(context, namespace("empty"))

        storage.store("blocked_users", "peer-1", ByteArray(0))

        assertEquals(emptyList<Int>(), loadedBytes(storage, "blocked_users", "peer-1"))
        assertEquals(listOf("peer-1"), storage.listKeys("blocked_users"))
    }

    // -- bounded reads -------------------------------------------------------

    /**
     * A record over the ceiling cannot have been written through [store], so it
     * must be dropped by size alone — never read into memory first.
     */
    @Test
    fun oversizedFileIsRejectedWithoutBeingRead() {
        val account = namespace("oversized")
        val storage = AppContainerProtocolStateStorage(context, account)
        storage.store("outbox", "message-1", byteArrayOf(1, 2, 3))

        // Sparse file: the ceiling is enforced on the *reported* size, so this
        // never occupies real disk in CI.
        val file = entryFile(account, "outbox", "message-1")
        RandomAccessFile(file, "rw").use {
            it.setLength(ProtocolStateRecord.MAX_FILE_BYTES.toLong() + 1)
        }

        assertThrows(MlsStorageException.CorruptedData::class.java) {
            storage.load("outbox", "message-1")
        }
        assertFalse(file.exists())
    }

    @Test
    fun storeRefusesValuesOverTheCeiling() {
        assertThrows(MlsStorageException.StoreFailed::class.java) {
            ProtocolStateRecord.frame(
                "outbox",
                "m-1",
                ByteArray(ProtocolStateRecord.MAX_VALUE_BYTES + 1)
            )
        }
    }

    /**
     * A file whose framing does not name the key that was asked for is not that
     * record — drop it rather than hand back someone else's bytes, and report
     * the drop so the SDK can settle the message id the app holds.
     *
     * Destruction is not absence: a silent null is indistinguishable from a
     * record that was never written, which would leave that id unresolved
     * forever.
     */
    @Test
    fun malformedRecordIsDroppedRatherThanReturned() {
        val account = namespace("malformed")
        val storage = AppContainerProtocolStateStorage(context, account)
        storage.store("outbox", "message-1", byteArrayOf(1, 2, 3))

        entryFile(account, "outbox", "message-1")
            .writeBytes(byteArrayOf(0, 1, 2, 3, 4, 5, 6, 7, 8))

        assertThrows(MlsStorageException.CorruptedData::class.java) {
            storage.load("outbox", "message-1")
        }
        assertEquals(emptyList<String>(), storage.listKeys("outbox"))
    }

    @Test
    fun unframedStrayFilesAreIgnoredByListing() {
        val account = namespace("stray")
        val storage = AppContainerProtocolStateStorage(context, account)
        storage.store("outbox", "message-1", byteArrayOf(1))

        val directory = entryFile(account, "outbox", "message-1").parentFile!!
        File(directory, "k_not-a-record").writeBytes(byteArrayOf(1, 2, 3))
        File(directory, "unrelated.tmp").writeBytes(byteArrayOf(1, 2, 3))

        assertEquals(listOf("message-1"), storage.listKeys("outbox"))
    }

    /**
     * The bound exists for a tampered container, and there the entries are
     * exactly the ones that yield no key. Counting keys collected would leave
     * every one of these opened on every launch while the counter sat at zero.
     */
    @Test
    fun enumerationBoundCountsEntriesExaminedNotKeysReturned() {
        val account = namespace("bounded-listing")
        val storage = AppContainerProtocolStateStorage(context, account)
        storage.store("outbox", "message-1", byteArrayOf(1))

        val directory = entryFile(account, "outbox", "message-1").parentFile!!
        for (index in 0 until 10) {
            File(directory, "k_unparseable-$index").writeBytes(byteArrayOf(1, 2, 3))
        }

        val enumeration = storage.enumerateKeys("outbox", 4)

        assertEquals(
            "enumeration must stop at the bound it was given",
            4,
            enumeration.examined
        )
        assertTrue(enumeration.keys.size <= 1)
    }

    /**
     * A digest names exactly one record, so two names for one key id can only
     * come from an `AtomicFile` twin or a copy planted in the container.
     * Restore must not walk the id twice because of it.
     */
    @Test
    fun listingDedupesARecordReachableUnderTwoNames() {
        val account = namespace("duplicate-names")
        val storage = AppContainerProtocolStateStorage(context, account)
        storage.store("outbox", "message-1", byteArrayOf(1, 2, 3))

        val original = entryFile(account, "outbox", "message-1")
        File(original.parentFile!!, "k_copy-of-message-1").writeBytes(original.readBytes())

        assertEquals(listOf("message-1"), storage.listKeys("outbox"))
    }
}
