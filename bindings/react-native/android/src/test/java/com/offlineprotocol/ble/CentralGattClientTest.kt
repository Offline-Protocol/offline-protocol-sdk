package com.offlineprotocol.ble

import org.junit.Assert.assertEquals
import org.junit.Assert.assertFalse
import org.junit.Assert.assertTrue
import org.junit.Test

/**
 * Unit tests for the pieces of [CentralGattClient] that can be exercised in
 * plain JVM without an Android framework or a live UniFFI `OfflineProtocol`.
 *
 * The interesting state machines in this class are gated behind
 * `BluetoothGatt` callbacks and UniFFI calls, both of which are out of reach
 * of a pure JVM test. What we *can* lock down is the per-peer buffer cleanup
 * that runs when the client decides to give up on a peer — that cleanup used
 * to drop `pendingInbound` but forget `outboundQueue`, which is the exact
 * regression this test file was created to guard.
 */
class CentralGattClientTest {

    private val noopBleThreadCheck: () -> Unit = {}

    @Test
    fun `clearPeerBuffers drains the outbound queue for the given peer`() {
        // Outbound queue is keyed by device id, not BLE address. Populate
        // two peers so we can verify the helper only touches the one we
        // asked for.
        val outbound = OutboundFragmentQueue(bleThreadCheck = noopBleThreadCheck)
        outbound.enqueue("peer-alpha", byteArrayOf(1, 2, 3))
        outbound.enqueue("peer-alpha", byteArrayOf(4, 5, 6))
        outbound.enqueue("peer-beta", byteArrayOf(7, 8, 9))

        val pending = InboundFragmentBuffer(bleThreadCheck = noopBleThreadCheck)
        val attempts = mutableMapOf<String, Long>()

        clearPeerBuffers(
            address = "AA:BB:CC:DD:EE:FF",
            peerId = "peer-alpha",
            pendingInbound = pending,
            outboundQueue = outbound,
            resolutionAttempts = attempts,
        )

        // Re-running removeAll must now be a no-op for the cleared peer.
        assertEquals(
            "outbound queue for cleared peer should be empty after clearPeerBuffers",
            0,
            outbound.removeAll("peer-alpha"),
        )
        // The other peer's queue must be untouched.
        assertEquals(
            "outbound queue for other peers must not be affected",
            1,
            outbound.removeAll("peer-beta"),
        )
    }

    @Test
    fun `clearPeerBuffers drains pendingInbound for the given address`() {
        val outbound = OutboundFragmentQueue(bleThreadCheck = noopBleThreadCheck)
        val pending = InboundFragmentBuffer(bleThreadCheck = noopBleThreadCheck)
        pending.enqueue("AA:BB:CC:DD:EE:FF", byteArrayOf(1, 2, 3))
        pending.enqueue("AA:BB:CC:DD:EE:FF", byteArrayOf(4, 5, 6))
        pending.enqueue("11:22:33:44:55:66", byteArrayOf(7))

        val attempts = mutableMapOf<String, Long>()

        clearPeerBuffers(
            address = "AA:BB:CC:DD:EE:FF",
            peerId = "peer-alpha",
            pendingInbound = pending,
            outboundQueue = outbound,
            resolutionAttempts = attempts,
        )

        assertFalse(
            "pendingInbound for the cleared address must be empty",
            pending.hasPending("AA:BB:CC:DD:EE:FF"),
        )
        assertTrue(
            "pendingInbound for other addresses must not be affected",
            pending.hasPending("11:22:33:44:55:66"),
        )
    }

    @Test
    fun `clearPeerBuffers drops the resolution attempt entry for the address`() {
        val outbound = OutboundFragmentQueue(bleThreadCheck = noopBleThreadCheck)
        val pending = InboundFragmentBuffer(bleThreadCheck = noopBleThreadCheck)
        val attempts = mutableMapOf(
            "AA:BB:CC:DD:EE:FF" to 12345L,
            "11:22:33:44:55:66" to 67890L,
        )

        clearPeerBuffers(
            address = "AA:BB:CC:DD:EE:FF",
            peerId = "peer-alpha",
            pendingInbound = pending,
            outboundQueue = outbound,
            resolutionAttempts = attempts,
        )

        assertFalse(
            "resolution attempt entry for the cleared address must be dropped",
            attempts.containsKey("AA:BB:CC:DD:EE:FF"),
        )
        assertTrue(
            "resolution attempt entries for other addresses must be untouched",
            attempts.containsKey("11:22:33:44:55:66"),
        )
    }

    @Test
    fun `clearPeerBuffers is a no-op when the peer has no outstanding buffers`() {
        // The give-up path is also reachable for peers that never managed
        // to transmit anything (e.g. failed handshake). Cleanup must not
        // throw or corrupt state in that case.
        val outbound = OutboundFragmentQueue(bleThreadCheck = noopBleThreadCheck)
        val pending = InboundFragmentBuffer(bleThreadCheck = noopBleThreadCheck)
        val attempts = mutableMapOf<String, Long>()

        clearPeerBuffers(
            address = "AA:BB:CC:DD:EE:FF",
            peerId = "peer-alpha",
            pendingInbound = pending,
            outboundQueue = outbound,
            resolutionAttempts = attempts,
        )

        assertEquals(0, outbound.totalCount())
        assertEquals(0, pending.totalCount())
        assertTrue(attempts.isEmpty())
    }
}
