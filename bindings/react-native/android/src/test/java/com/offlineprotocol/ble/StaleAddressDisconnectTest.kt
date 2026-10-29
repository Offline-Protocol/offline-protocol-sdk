package com.offlineprotocol.ble

import android.bluetooth.BluetoothAdapter
import android.bluetooth.BluetoothDevice
import android.bluetooth.BluetoothGatt
import android.bluetooth.BluetoothProfile
import android.os.Handler
import android.os.Looper
import com.offlineprotocol.BleAppTag
import com.offlineprotocol.mesh.MeshController
import org.junit.Assert.assertEquals
import org.junit.Assert.assertFalse
import org.junit.Assert.assertNull
import org.junit.Assert.assertTrue
import org.junit.Test
import org.junit.runner.RunWith
import org.robolectric.RobolectricTestRunner
import org.robolectric.Shadows.shadowOf
import org.robolectric.annotation.Config
import org.robolectric.shadows.ShadowBluetoothDevice
import org.robolectric.shadows.ShadowBluetoothGatt
import uniffi.offline_protocol.OfflineProtocol
import java.time.Duration
import java.util.UUID

/**
 * Drives the real [CentralGattClient] disconnect path for a peer that came
 * back from a new BLE address (iOS rotates its random address across a
 * Bluetooth power-cycle). The old address dropping must neither redial it nor
 * give up on the peer: giving up reports a false `neighbor_lost` for a peer
 * that is live at the new address.
 */
@RunWith(RobolectricTestRunner::class)
@Config(sdk = [34])
class StaleAddressDisconnectTest {

    private val uuid = UUID.fromString("6E400001-B5A3-F393-E0A9-E50E24DCCA9E")
    private val old = "AA:BB:CC:DD:EE:01"
    private val new = "AA:BB:CC:DD:EE:02"
    private val host = FakeHost()

    private val client = CentralGattClient(
        bleHandler = Handler(Looper.getMainLooper()),
        serviceUuid = uuid,
        messageCharUuid = uuid,
        deviceIdCharUuid = uuid,
        identityCharUuid = uuid,
        appTagCharUuid = uuid,
        appTag = BleAppTag.compute("our-app"),
        host = host,
        diagnosticEmitter = { _, _, _ -> },
    )

    private fun gatt(address: String): BluetoothGatt =
        ShadowBluetoothGatt.newInstance(ShadowBluetoothDevice.newInstance(address))

    /** Delivers the disconnect, then runs past every reconnect backoff. */
    private fun disconnect(gatt: BluetoothGatt) {
        client.callback.onConnectionStateChange(gatt, 8, BluetoothProfile.STATE_DISCONNECTED)
        shadowOf(Looper.getMainLooper()).idleFor(Duration.ofMinutes(2))
    }

    @Test
    fun `the old address dropping neither redials it nor gives up on the live peer`() {
        val oldGatt = gatt(old)
        host.connections.registerGatt(old, oldGatt)
        host.connections.setDeviceIdentifier(old, "peerA")
        host.connections.registerGatt(new, gatt(new))
        host.connections.setDeviceIdentifier(new, "peerA")
        host.pendingInbound.enqueue(old, byteArrayOf(1))

        disconnect(oldGatt)

        assertEquals("no false peer loss", emptyList<String>(), host.givenUp)
        assertEquals("the stale address is not redialled", emptyList<String>(), host.dialed)
        assertEquals(new, host.connections.addressForDevice("peerA"))
        assertNull(host.connections.deviceIdForAddress(old))
        assertEquals("the facade drops the dead address's state", listOf(old), host.staleDropped)
        assertFalse("the dead address's inbound is dropped", host.pendingInbound.hasPending(old))
    }

    @Test
    fun `a peer that comes back while the give-up is queued is not given up`() {
        // A stale callback with no other link yet posts the give-up. The new
        // link lands before it runs, so only the check inside the give-up
        // itself can keep the live peer.
        host.connections.setDeviceIdentifier(old, "peerC")

        client.callback.onConnectionStateChange(gatt(old), 8, BluetoothProfile.STATE_DISCONNECTED)
        host.connections.registerGatt(new, gatt(new))
        host.connections.setDeviceIdentifier(new, "peerC")
        shadowOf(Looper.getMainLooper()).idleFor(Duration.ofMinutes(2))

        assertEquals("no false peer loss", emptyList<String>(), host.givenUp)
        assertEquals(listOf(old), host.staleDropped)
        assertEquals(new, host.connections.addressForDevice("peerC"))
    }

    @Test
    fun `a peer with no other link is still given up`() {
        // Not registered as a gatt: the stale-callback branch gives up at once.
        host.connections.setDeviceIdentifier(old, "peerB")

        disconnect(gatt(old))

        assertEquals(listOf("peerB"), host.givenUp)
        assertEquals(emptyList<String>(), host.staleDropped)
        assertNull(host.connections.addressForDevice("peerB"))
    }

    @Test
    fun `a link that drops with nothing left reaches the facade at once, not after the backoff`() {
        // The backoff takes about two minutes to give up, and a peer whose
        // Bluetooth went off stayed a neighbour all that time (#513). The
        // facade bounds it, so it must hear of the drop when it happens.
        val dropped = gatt(old)
        host.connections.registerGatt(old, dropped)
        host.connections.markClientEstablished(old)
        host.connections.setDeviceIdentifier(old, "peerD")

        client.callback.onConnectionStateChange(dropped, 0, BluetoothProfile.STATE_DISCONNECTED)
        shadowOf(Looper.getMainLooper()).idle()

        assertEquals(listOf("peerD"), host.linkDropped)
        assertEquals("the backoff still carries on behind it", emptyList<String>(), host.givenUp)
        assertFalse(host.connections.hasEstablishedLink("peerD"))
    }

    @Test
    fun `a dropped link's gatt is closed before the redial`() {
        // The redial opens a fresh handle, so an unclosed one leaks a client
        // slot per drop until the stack refuses every new connection.
        val dropped = gatt(old)
        host.connections.registerGatt(old, dropped)
        host.connections.markClientEstablished(old)
        host.connections.setDeviceIdentifier(old, "peerF")

        disconnect(dropped)

        assertTrue("the dead handle is closed", shadowOf(dropped).isClosed)
        assertEquals("the redial still happens", listOf(old), host.dialed)
    }

    @Test
    fun `a link disconnected to redial is redialled even though the peer holds another link`() {
        // A silent link's peer usually also writes to our server from an
        // address its hello mapped. Dropping the silent address as stale left
        // the redial to a fresh advertisement, which a backgrounded iPhone may
        // never send.
        val silent = gatt(old)
        host.connections.registerGatt(old, silent)
        host.connections.markClientEstablished(old)
        host.connections.setDeviceIdentifier(old, "peerH")
        host.connections.registerGatt(new, gatt(new))
        host.connections.setDeviceIdentifier(new, "peerH")

        client.disconnectToRedial(silent)
        disconnect(silent)

        assertEquals(listOf(old), host.dialed)
        assertEquals(emptyList<String>(), host.staleDropped)
    }

    @Test
    fun `fragments drained once their writer is named count as hearing the peer`() {
        // Otherwise an answer that arrived before its sender was resolved went
        // unseen, and a link the peer had answered was torn down as silent.
        host.pendingInbound.enqueue(old, byteArrayOf(1))
        host.pendingInbound.enqueue(old, byteArrayOf(2))

        client.drainPendingInboundFor(old, "peerI")
        client.drainPendingInboundFor(new, "peerJ")

        assertEquals(listOf("peerI"), host.heard)
    }

    @Test
    fun `a stale address's gatt is closed even though it is not redialled`() {
        val oldGatt = gatt(old)
        host.connections.registerGatt(old, oldGatt)
        host.connections.setDeviceIdentifier(old, "peerG")
        host.connections.registerGatt(new, gatt(new))
        host.connections.setDeviceIdentifier(new, "peerG")

        disconnect(oldGatt)

        assertTrue("the dead handle is closed", shadowOf(oldGatt).isClosed)
    }

    @Test
    fun `a dial still in flight is not a link`() {
        // A dial to a phone whose Bluetooth is off lasts as long as the
        // stack's connect timeout; counting it would defer the loss that long.
        host.connections.setDeviceIdentifier(old, "peerE")
        host.connections.registerGatt(old, gatt(old))
        assertFalse(host.connections.hasEstablishedLink("peerE"))

        host.connections.markClientEstablished(old)
        assertTrue(host.connections.hasEstablishedLink("peerE"))

        host.connections.removeGatt(old)
        assertFalse(host.connections.hasEstablishedLink("peerE"))
    }

    private class FakeHost : CentralGattClient.Host {
        val givenUp = mutableListOf<String>()
        val linkDropped = mutableListOf<String>()
        val dialed = mutableListOf<String>()
        val staleDropped = mutableListOf<String>()
        val heard = mutableListOf<String>()

        // `finalizeGivenUpPeer` catches what this throws, so the test records
        // peer loss through `onPeerGivenUp`, which runs right after it.
        override val protocol: OfflineProtocol
            get() = throw IllegalStateException("no core in this test")
        override val connections = MeshConnectionRegistry()
        override val pendingInbound = InboundFragmentBuffer(bleThreadCheck = {})
        override val outboundQueue = OutboundFragmentQueue(bleThreadCheck = {})
        override val meshController = MeshController("self", MeshController.MeshConfig(maxConnections = 4))
        @Suppress("DEPRECATION")
        override val bluetoothAdapter: BluetoothAdapter? = BluetoothAdapter.getDefaultAdapter()
        override val selfDeviceId = "self"
        override fun isShuttingDown() = false
        override fun isRunning() = true
        override fun rssiFor(address: String): Short? = null
        override fun clearRssi(address: String) {}
        override fun markNonMeshDevice(address: String) {}
        override fun refreshAdvertising(reason: String) {}
        override fun refreshSelfMetrics() {}
        override fun maybeHandleRebalance(trigger: String) {}
        override fun drainAndSendFragments() {}
        override fun onWriteCompleted(address: String) {}
        override fun handleInboundFragment(address: String, data: ByteArray) {}
        override fun onPeerHeard(deviceId: String) { heard += deviceId }
        override fun onPeerMtuNegotiated(address: String, maxPayload: Int) {}
        override fun onDeviceIdResolved(address: String, deviceId: String) {}
        override fun onPeerGivenUp(address: String, peerId: String) { givenUp += peerId }
        override fun onStaleAddressDropped(address: String) { staleDropped += address }
        override fun onPeerLinkDropped(peerId: String) { linkDropped += peerId }
        override fun connectToDevice(device: BluetoothDevice) { dialed += device.address }
    }
}
