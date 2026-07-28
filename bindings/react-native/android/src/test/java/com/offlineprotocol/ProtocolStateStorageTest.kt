package com.offlineprotocol

import android.content.Context
import org.junit.Assert.assertEquals
import org.junit.Assert.assertNull
import org.junit.Test
import org.junit.runner.RunWith
import org.robolectric.RobolectricTestRunner
import org.robolectric.RuntimeEnvironment
import java.util.UUID

@RunWith(RobolectricTestRunner::class)
class ProtocolStateStorageTest {
    private val context: Context
        get() = RuntimeEnvironment.getApplication()

    private fun namespace(label: String): String =
        StorageNamespace.account("protocol-state-test-${UUID.randomUUID()}", label)

    // Robolectric loads Android code in a sandbox classloader. UByte is an
    // inline class, so comparing the provider's boxed List<UByte> directly to
    // a test-created list can fail solely because the boxed classes came from
    // different classloaders. Compare their stable numeric values instead.
    private fun loadedBytes(
        storage: AppContainerProtocolStateStorage,
        keyType: String,
        keyId: String
    ): List<Int>? = storage.load(keyType, keyId)?.map { it.toInt() }

    @Test
    fun roundTripOverwriteListingAndIdempotentDelete() {
        val storage = AppContainerProtocolStateStorage(context, namespace("round-trip"))

        storage.store("pending/messages", "peer with punctuation", listOf(0u, 1u, 255u))
        assertEquals(
            listOf(0, 1, 255),
            loadedBytes(storage, "pending/messages", "peer with punctuation")
        )
        assertEquals(
            listOf("peer with punctuation"),
            storage.listKeys("pending/messages")
        )

        storage.store("pending/messages", "peer with punctuation", listOf(4u, 5u))
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

        alice.store("outbox", "message-1", listOf(1u, 2u, 3u))

        assertEquals(listOf(1, 2, 3), loadedBytes(alice, "outbox", "message-1"))
        assertNull(loadedBytes(bob, "outbox", "message-1"))
    }

    @Test
    fun restartReopensTheSameInstallRoot() {
        val account = namespace("restart")
        val first = AppContainerProtocolStateStorage(context, account)
        first.store("outbox", "message-1", listOf(7u, 8u, 9u))

        val restarted = AppContainerProtocolStateStorage(context, account)

        assertEquals(listOf(7, 8, 9), loadedBytes(restarted, "outbox", "message-1"))
    }
}
