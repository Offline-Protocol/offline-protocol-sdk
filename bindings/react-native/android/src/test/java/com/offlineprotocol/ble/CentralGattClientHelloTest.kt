package com.offlineprotocol.ble

import android.bluetooth.BluetoothAdapter
import android.bluetooth.BluetoothDevice
import android.bluetooth.BluetoothGatt
import android.bluetooth.BluetoothGattCharacteristic
import android.bluetooth.BluetoothGattDescriptor
import android.bluetooth.BluetoothGattService
import android.bluetooth.BluetoothStatusCodes
import android.os.Handler
import android.os.Looper
import com.offlineprotocol.BleAppTag
import com.offlineprotocol.mesh.MeshController
import org.junit.Assert.assertArrayEquals
import org.junit.Assert.assertEquals
import org.junit.Test
import org.junit.runner.RunWith
import org.robolectric.RobolectricTestRunner
import org.robolectric.Shadows.shadowOf
import org.robolectric.annotation.Config
import org.robolectric.shadow.api.Shadow
import org.robolectric.shadows.ShadowBluetoothDevice
import org.robolectric.shadows.ShadowBluetoothGatt
import uniffi.offline_protocol.OfflineProtocol
import java.time.Duration
import java.util.UUID

/**
 * The central writes its identity assertion to the peer's Hello
 * characteristic between the CCCD ack and link-ready (#512), so the peer maps
 * our address before our first Message write instead of dialling it back.
 * Every way the hello cannot happen must still leave the link ready.
 */
@RunWith(RobolectricTestRunner::class)
@Config(sdk = [34], shadows = [RecordingGatt::class])
class CentralGattClientHelloTest {

    private val serviceUuid = UUID.fromString("6E400001-B5A3-F393-E0A9-E50E24DCCA9E")
    private val messageUuid = UUID.fromString("6E400002-B5A3-F393-E0A9-E50E24DCCA9E")
    private val deviceIdUuid = UUID.fromString("6E400003-B5A3-F393-E0A9-E50E24DCCA9E")
    private val identityUuid = UUID.fromString("6E400004-B5A3-F393-E0A9-E50E24DCCA9E")
    private val appTagUuid = UUID.fromString("6E400005-B5A3-F393-E0A9-E50E24DCCA9E")
    private val helloUuid = UUID.fromString("6E400006-B5A3-F393-E0A9-E50E24DCCA9E")
    private val address = "AA:BB:CC:DD:EE:01"
    private val assertion = ByteArray(115) { it.toByte() }
    private val host = FakeHost(assertion)

    private val client = CentralGattClient(
        bleHandler = Handler(Looper.getMainLooper()),
        serviceUuid = serviceUuid,
        messageCharUuid = messageUuid,
        deviceIdCharUuid = deviceIdUuid,
        identityCharUuid = identityUuid,
        appTagCharUuid = appTagUuid,
        appTag = BleAppTag.compute("our-app"),
        host = host,
        helloCharUuid = helloUuid,
    )

    private class Link(val gatt: BluetoothGatt, val shadow: RecordingGatt, val hello: BluetoothGattCharacteristic?)

    private fun link(withHello: Boolean): Link {
        val service = BluetoothGattService(serviceUuid, BluetoothGattService.SERVICE_TYPE_PRIMARY)
        service.addCharacteristic(
            BluetoothGattCharacteristic(
                messageUuid,
                BluetoothGattCharacteristic.PROPERTY_INDICATE or BluetoothGattCharacteristic.PROPERTY_WRITE_NO_RESPONSE,
                BluetoothGattCharacteristic.PERMISSION_WRITE,
            )
        )
        for (uuid in listOf(deviceIdUuid, identityUuid)) {
            service.addCharacteristic(
                BluetoothGattCharacteristic(
                    uuid,
                    BluetoothGattCharacteristic.PROPERTY_READ,
                    BluetoothGattCharacteristic.PERMISSION_READ,
                )
            )
        }
        val hello = if (withHello) {
            BluetoothGattCharacteristic(
                helloUuid,
                BluetoothGattCharacteristic.PROPERTY_WRITE,
                BluetoothGattCharacteristic.PERMISSION_WRITE,
            ).also { service.addCharacteristic(it) }
        } else {
            null
        }
        val gatt = ShadowBluetoothGatt.newInstance(ShadowBluetoothDevice.newInstance(address))
        val shadow = Shadow.extract<RecordingGatt>(gatt)
        shadow.registered += service
        host.connections.registerGatt(address, gatt)
        return Link(gatt, shadow, hello)
    }

    private fun idle() = shadowOf(Looper.getMainLooper()).idle()

    /** Takes the link through discovery and the MTU exchange to the CCCD ack. */
    private fun cccdAcked(link: Link, mtu: Int = 247) {
        client.callback.onServicesDiscovered(link.gatt, BluetoothGatt.GATT_SUCCESS)
        client.callback.onMtuChanged(link.gatt, mtu, BluetoothGatt.GATT_SUCCESS)
        idle()
        val cccd = BluetoothGattDescriptor(
            PeripheralGattServer.CCCD_UUID,
            BluetoothGattDescriptor.PERMISSION_WRITE,
        )
        client.callback.onDescriptorWrite(link.gatt, cccd, BluetoothGatt.GATT_SUCCESS)
        idle()
    }

    @Test
    fun `the hello is written after the CCCD ack and the link goes ready only when it completes`() {
        val link = link(withHello = true)

        cccdAcked(link)

        assertEquals(listOf(helloUuid), link.shadow.writes.map { it.first })
        assertArrayEquals("the value the Identity characteristic serves", assertion, link.shadow.writes.single().second)
        assertEquals("no Message write may overtake the hello", 0, host.drains)

        client.callback.onCharacteristicWrite(link.gatt, link.hello!!, BluetoothGatt.GATT_SUCCESS)
        idle()
        assertEquals(1, host.drains)
    }

    @Test
    fun `a peer without the characteristic gets no hello and the link goes ready at once`() {
        val link = link(withHello = false)

        cccdAcked(link)

        assertEquals(emptyList<UUID>(), link.shadow.writes.map { it.first })
        assertEquals(1, host.drains)
    }

    @Test
    fun `a link whose MTU cannot carry the hello in one write skips it`() {
        val link = link(withHello = true)

        cccdAcked(link, mtu = 23)

        assertEquals(emptyList<UUID>(), link.shadow.writes.map { it.first })
        assertEquals(1, host.drains)
    }

    @Test
    fun `a refused write and a missing callback both still go ready`() {
        val refused = link(withHello = true)
        refused.shadow.writeResult = BluetoothStatusCodes.ERROR_GATT_WRITE_REQUEST_BUSY
        cccdAcked(refused)
        assertEquals("refused at the stack", 1, host.drains)

        host.drains = 0
        val silent = link(withHello = true)
        cccdAcked(silent)
        assertEquals(0, host.drains)
        shadowOf(Looper.getMainLooper()).idleFor(Duration.ofSeconds(3))
        assertEquals("the watchdog goes ready without the callback", 1, host.drains)

        // A callback after the watchdog does not mark the link ready twice.
        client.callback.onCharacteristicWrite(silent.gatt, silent.hello!!, BluetoothGatt.GATT_SUCCESS)
        idle()
        assertEquals(1, host.drains)
    }

    private class FakeHost(private val hello: ByteArray?) : CentralGattClient.Host {
        var drains = 0
        override val protocol: OfflineProtocol
            get() = throw IllegalStateException("the hello reaches no core")
        override val connections = MeshConnectionRegistry()
        override val pendingInbound = InboundFragmentBuffer(bleThreadCheck = {})
        override val outboundQueue = OutboundFragmentQueue(bleThreadCheck = {})
        override val meshController = MeshController("self", MeshController.MeshConfig(maxConnections = 4))
        override val bluetoothAdapter: BluetoothAdapter? = null
        override val selfDeviceId = "self"
        override fun isShuttingDown() = false
        override fun isRunning() = true
        override fun rssiFor(address: String): Short? = null
        override fun clearRssi(address: String) {}
        override fun markNonMeshDevice(address: String) {}
        override fun refreshAdvertising(reason: String) {}
        override fun refreshSelfMetrics() {}
        override fun maybeHandleRebalance(trigger: String) {}
        override fun drainAndSendFragments() { drains += 1 }
        override fun onWriteCompleted(address: String) {}
        override fun handleInboundFragment(address: String, data: ByteArray) {}
        override fun onPeerMtuNegotiated(address: String, maxPayload: Int) {}
        override fun onDeviceIdResolved(address: String, deviceId: String) {}
        override fun onPeerGivenUp(address: String, peerId: String) {}
        override fun onStaleAddressDropped(address: String) {}
        override fun connectToDevice(device: BluetoothDevice) {}
        override fun helloBytes(): ByteArray? = hello
    }
}
