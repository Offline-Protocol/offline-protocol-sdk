package com.offlineprotocol.ble

import android.bluetooth.BluetoothAdapter
import android.bluetooth.BluetoothDevice
import android.bluetooth.BluetoothGatt
import android.bluetooth.BluetoothGattCharacteristic
import android.bluetooth.BluetoothGattService
import android.bluetooth.BluetoothProfile
import android.os.Handler
import android.os.Looper
import com.offlineprotocol.BleAppTag
import com.offlineprotocol.mesh.MeshController
import org.junit.Assert.assertEquals
import org.junit.Assert.assertSame
import org.junit.Assert.assertTrue
import org.junit.Test
import org.junit.runner.RunWith
import org.robolectric.RobolectricTestRunner
import org.robolectric.Shadows.shadowOf
import org.robolectric.annotation.Config
import org.robolectric.annotation.Implementation
import org.robolectric.annotation.Implements
import org.robolectric.shadow.api.Shadow
import org.robolectric.shadows.ShadowBluetoothDevice
import org.robolectric.shadows.ShadowBluetoothGatt
import uniffi.offline_protocol.OfflineProtocol
import java.time.Duration
import java.util.UUID

/**
 * Drives the real [CentralGattClient] handshake chain through a link that
 * presents several instances of the service, the path a phone running more
 * than one SDK app produces and CI never exercises.
 *
 * The stock Robolectric shadow does not implement `readCharacteristic`, so
 * [RecordingGatt] records every read the client issues and lets the test
 * deliver each answer through the client's own callback, one GATT op at a
 * time, exactly as the stack does.
 */
@RunWith(RobolectricTestRunner::class)
@Config(sdk = [34], shadows = [RecordingGatt::class])
class CentralGattClientInstanceSelectionTest {

    private val serviceUuid = UUID.fromString("6E400001-B5A3-F393-E0A9-E50E24DCCA9E")
    private val messageUuid = UUID.fromString("6E400002-B5A3-F393-E0A9-E50E24DCCA9E")
    private val deviceIdUuid = UUID.fromString("6E400003-B5A3-F393-E0A9-E50E24DCCA9E")
    private val identityUuid = UUID.fromString("6E400004-B5A3-F393-E0A9-E50E24DCCA9E")
    private val appTagUuid = UUID.fromString("6E400005-B5A3-F393-E0A9-E50E24DCCA9E")

    private val ourTag = BleAppTag.compute("our-app")
    private val otherTag = BleAppTag.compute("other-app")
    private val address = "AA:BB:CC:DD:EE:01"

    private val diagnostics = mutableListOf<Pair<String, Map<String, Any?>>>()

    /** One instance of the service as a remote phone's app registers it. */
    private class Instance(
        val service: BluetoothGattService,
        val deviceId: BluetoothGattCharacteristic,
        val identity: BluetoothGattCharacteristic,
        val appTag: BluetoothGattCharacteristic?,
    )

    private fun instance(instanceId: Int, deviceId: String, tag: ByteArray?, tagged: Boolean = tag != null): Instance {
        val service = BluetoothGattService(serviceUuid, BluetoothGattService.SERVICE_TYPE_PRIMARY)
        // Hidden on the SDK stub, present on the framework Robolectric runs.
        BluetoothGattService::class.java.getMethod("setInstanceId", Int::class.javaPrimitiveType).invoke(service, instanceId)
        fun readable(uuid: UUID, value: ByteArray): BluetoothGattCharacteristic {
            val c = BluetoothGattCharacteristic(uuid, BluetoothGattCharacteristic.PROPERTY_READ, BluetoothGattCharacteristic.PERMISSION_READ)
            @Suppress("DEPRECATION")
            c.value = value
            service.addCharacteristic(c)
            return c
        }
        service.addCharacteristic(
            BluetoothGattCharacteristic(
                messageUuid,
                BluetoothGattCharacteristic.PROPERTY_NOTIFY or BluetoothGattCharacteristic.PROPERTY_WRITE_NO_RESPONSE,
                BluetoothGattCharacteristic.PERMISSION_WRITE,
            )
        )
        val id = readable(deviceIdUuid, deviceId.toByteArray())
        val identity = readable(identityUuid, ByteArray(96))
        val appTag = if (tagged) readable(appTagUuid, tag ?: ByteArray(0)) else null
        return Instance(service, id, identity, appTag)
    }

    private class Link(val gatt: BluetoothGatt, val shadow: RecordingGatt)

    private fun link(vararg instances: Instance): Link {
        val device: BluetoothDevice = ShadowBluetoothDevice.newInstance(address)
        val gatt = ShadowBluetoothGatt.newInstance(device)
        val shadow = Shadow.extract<RecordingGatt>(gatt)
        instances.forEach { shadow.registered += it.service }
        return Link(gatt, shadow)
    }

    private fun client(): CentralGattClient = CentralGattClient(
        bleHandler = Handler(Looper.getMainLooper()),
        serviceUuid = serviceUuid,
        messageCharUuid = messageUuid,
        deviceIdCharUuid = deviceIdUuid,
        identityCharUuid = identityUuid,
        appTagCharUuid = appTagUuid,
        appTag = ourTag,
        host = FakeHost(),
        diagnosticEmitter = { _, message, ctx -> diagnostics += message to ctx },
    )

    private fun idle() = shadowOf(Looper.getMainLooper()).idle()
    private fun idleFor(ms: Long) = shadowOf(Looper.getMainLooper()).idleFor(Duration.ofMillis(ms))

    /** The stack answered the client's last read with the characteristic's stored value. */
    private fun answerLastRead(client: CentralGattClient, link: Link) {
        val read = link.shadow.reads.last()
        client.callback.onCharacteristicRead(link.gatt, read, BluetoothGatt.GATT_SUCCESS)
        idle()
    }

    private fun chosen(): Map<String, Any?> =
        diagnostics.last { it.first == "BLE service instance chosen" }.second

    // --- Two instances ------------------------------------------------------

    @Test
    fun `two instances - tags are read one at a time and the device id is read on our app's instance`() {
        val first = instance(instanceId = 1, deviceId = "peer-first", tag = otherTag)
        val second = instance(instanceId = 2, deviceId = "peer-second", tag = ourTag)
        val link = link(first, second)
        val client = client()

        client.callback.onServicesDiscovered(link.gatt, BluetoothGatt.GATT_SUCCESS)
        assertEquals("nothing is read before the MTU exchange", 0, link.shadow.reads.size)
        assertEquals(1, link.shadow.mtuRequests)

        client.callback.onMtuChanged(link.gatt, 247, BluetoothGatt.GATT_SUCCESS)
        idle()
        assertSame("the first instance's tag is read first", first.appTag, link.shadow.reads.last())
        assertEquals("one GATT op at a time", 1, link.shadow.reads.size)

        answerLastRead(client, link)
        assertSame("then the second instance's tag", second.appTag, link.shadow.reads.last())
        assertEquals(2, link.shadow.reads.size)

        answerLastRead(client, link)
        assertEquals(1, chosen()["index"])
        assertEquals("same_app", chosen()["reason"])
        assertEquals(2, chosen()["tagged"])
        assertSame("the handshake continues with the CHOSEN instance's device id", second.deviceId, link.shadow.reads.last())
        assertEquals(3, link.shadow.reads.size)
        assertEquals(2, client.handshakeService(link.gatt)?.instanceId)
        assertTrue("the link is never closed", !link.shadow.closed)
    }

    @Test
    fun `two instances - our tag on the first instance is chosen without reading a device id elsewhere`() {
        val first = instance(instanceId = 1, deviceId = "peer-first", tag = ourTag)
        val second = instance(instanceId = 2, deviceId = "peer-second", tag = otherTag)
        val link = link(first, second)
        val client = client()

        client.callback.onServicesDiscovered(link.gatt, BluetoothGatt.GATT_SUCCESS)
        client.callback.onMtuChanged(link.gatt, 247, BluetoothGatt.GATT_SUCCESS)
        idle()
        answerLastRead(client, link)
        answerLastRead(client, link)

        assertEquals(0, chosen()["index"])
        assertEquals("same_app", chosen()["reason"])
        assertSame(first.deviceId, link.shadow.reads.last())
        assertEquals(1, client.handshakeService(link.gatt)?.instanceId)
    }

    @Test
    fun `two instances - no tag of ours and one untagged build - the untagged build is chosen`() {
        val tagged = instance(instanceId = 1, deviceId = "peer-tagged", tag = otherTag)
        val old = instance(instanceId = 2, deviceId = "peer-old", tag = null)
        val link = link(tagged, old)
        val client = client()

        client.callback.onServicesDiscovered(link.gatt, BluetoothGatt.GATT_SUCCESS)
        client.callback.onMtuChanged(link.gatt, 247, BluetoothGatt.GATT_SUCCESS)
        idle()
        assertSame(tagged.appTag, link.shadow.reads.last())
        answerLastRead(client, link)
        // The old build has no APP_TAG characteristic: nothing to read, the
        // choice is made straight away.
        assertEquals(1, chosen()["index"])
        assertEquals("sole_untagged", chosen()["reason"])
        assertSame(old.deviceId, link.shadow.reads.last())
        assertEquals(2, client.handshakeService(link.gatt)?.instanceId)
    }

    @Test
    fun `two instances - neither is ours and both are tagged - the first is chosen as before the tag existed`() {
        val a = instance(instanceId = 7, deviceId = "peer-a", tag = otherTag)
        val b = instance(instanceId = 9, deviceId = "peer-b", tag = BleAppTag.compute("third-app"))
        val link = link(a, b)
        val client = client()

        client.callback.onServicesDiscovered(link.gatt, BluetoothGatt.GATT_SUCCESS)
        client.callback.onMtuChanged(link.gatt, 247, BluetoothGatt.GATT_SUCCESS)
        idle()
        answerLastRead(client, link)
        answerLastRead(client, link)

        assertEquals(0, chosen()["index"])
        assertEquals("first_instance", chosen()["reason"])
        assertSame(a.deviceId, link.shadow.reads.last())
        assertEquals(7, client.handshakeService(link.gatt)?.instanceId)
    }

    @Test
    fun `a tag read the stack never answers times out as untagged and the chain goes on`() {
        val first = instance(instanceId = 1, deviceId = "peer-first", tag = otherTag)
        val second = instance(instanceId = 2, deviceId = "peer-second", tag = ourTag)
        val link = link(first, second)
        val client = client()

        client.callback.onServicesDiscovered(link.gatt, BluetoothGatt.GATT_SUCCESS)
        client.callback.onMtuChanged(link.gatt, 247, BluetoothGatt.GATT_SUCCESS)
        idle()
        assertSame(first.appTag, link.shadow.reads.last())

        idleFor(2_999)
        assertEquals("still waiting on the first tag", 1, link.shadow.reads.size)
        idleFor(1)
        assertSame("the watchdog moved on to the second instance", second.appTag, link.shadow.reads.last())

        answerLastRead(client, link)
        assertEquals(1, chosen()["index"])
        assertEquals("same_app", chosen()["reason"])
        assertEquals("the timed-out instance counts as untagged", 1, chosen()["tagged"])
        assertSame(second.deviceId, link.shadow.reads.last())
    }

    @Test
    fun `a late answer to a read already given up on is not credited to the next instance`() {
        val first = instance(instanceId = 1, deviceId = "peer-first", tag = ourTag)
        val second = instance(instanceId = 2, deviceId = "peer-second", tag = otherTag)
        val link = link(first, second)
        val client = client()

        client.callback.onServicesDiscovered(link.gatt, BluetoothGatt.GATT_SUCCESS)
        client.callback.onMtuChanged(link.gatt, 247, BluetoothGatt.GATT_SUCCESS)
        idle()
        idleFor(3_000)
        assertSame(second.appTag, link.shadow.reads.last())

        // The first instance's read answers only now, while the second's is
        // in flight. Were it credited to the second instance, the second
        // would look like ours.
        client.callback.onCharacteristicRead(link.gatt, first.appTag!!, BluetoothGatt.GATT_SUCCESS)
        idle()
        assertTrue("no choice yet: the second read is still outstanding", diagnostics.none { it.first == "BLE service instance chosen" })

        answerLastRead(client, link)
        // Credited to the second instance, the late tag would have made it
        // look like ours (same_app, index 1). It was not: the first instance
        // stands as untagged, and being the only untagged one it is chosen.
        assertEquals("sole_untagged", chosen()["reason"])
        assertEquals(0, chosen()["index"])
        assertEquals(1, chosen()["tagged"])
    }

    @Test
    fun `the MTU watchdog exit also turns to the tag reads`() {
        val first = instance(instanceId = 1, deviceId = "peer-first", tag = otherTag)
        val second = instance(instanceId = 2, deviceId = "peer-second", tag = ourTag)
        val link = link(first, second)
        val client = client()

        client.callback.onServicesDiscovered(link.gatt, BluetoothGatt.GATT_SUCCESS)
        idleFor(3_000)
        assertSame("no MTU answer: the watchdog resumes with the tag reads", first.appTag, link.shadow.reads.last())
        answerLastRead(client, link)
        answerLastRead(client, link)
        assertEquals("same_app", chosen()["reason"])
        assertSame(second.deviceId, link.shadow.reads.last())
    }

    @Test
    fun `a refused MTU request also turns to the tag reads`() {
        val first = instance(instanceId = 1, deviceId = "peer-first", tag = otherTag)
        val second = instance(instanceId = 2, deviceId = "peer-second", tag = ourTag)
        val link = link(first, second)
        link.shadow.mtuResult = false
        val client = client()

        client.callback.onServicesDiscovered(link.gatt, BluetoothGatt.GATT_SUCCESS)
        idle()
        assertSame(first.appTag, link.shadow.reads.last())
        answerLastRead(client, link)
        answerLastRead(client, link)
        assertEquals("same_app", chosen()["reason"])
        assertSame(second.deviceId, link.shadow.reads.last())
    }

    @Test
    fun `a new connection on the same address chooses afresh`() {
        val first = instance(instanceId = 1, deviceId = "peer-first", tag = otherTag)
        val second = instance(instanceId = 2, deviceId = "peer-second", tag = ourTag)
        val link = link(first, second)
        val client = client()

        client.callback.onServicesDiscovered(link.gatt, BluetoothGatt.GATT_SUCCESS)
        client.callback.onMtuChanged(link.gatt, 247, BluetoothGatt.GATT_SUCCESS)
        idle()
        answerLastRead(client, link)
        answerLastRead(client, link)
        assertEquals(2, client.handshakeService(link.gatt)?.instanceId)

        client.callback.onConnectionStateChange(link.gatt, BluetoothGatt.GATT_SUCCESS, BluetoothProfile.STATE_CONNECTED)
        assertEquals("the binding is dropped with the old connection: back to the first instance until a new choice",
            1, client.handshakeService(link.gatt)?.instanceId)
    }

    // --- One instance -------------------------------------------------------

    @Test
    fun `one instance - the tag is never read and the device id read goes to it`() {
        val only = instance(instanceId = 3, deviceId = "peer-only", tag = otherTag)
        val link = link(only)
        val client = client()

        client.callback.onServicesDiscovered(link.gatt, BluetoothGatt.GATT_SUCCESS)
        client.callback.onMtuChanged(link.gatt, 247, BluetoothGatt.GATT_SUCCESS)
        idle()

        assertEquals(listOf(only.deviceId), link.shadow.reads)
        assertTrue(diagnostics.none { it.first.startsWith("BLE service instance") || it.first.startsWith("BLE peer serves") })
        assertEquals(3, client.handshakeService(link.gatt)?.instanceId)
    }

    @Test
    fun `one instance - every MTU exit goes straight to the device id read`() {
        for (exit in listOf("callback", "watchdog", "refused")) {
            diagnostics.clear()
            val only = instance(instanceId = 3, deviceId = "peer-only", tag = null)
            val link = link(only)
            if (exit == "refused") link.shadow.mtuResult = false
            val client = client()
            client.callback.onServicesDiscovered(link.gatt, BluetoothGatt.GATT_SUCCESS)
            when (exit) {
                "callback" -> client.callback.onMtuChanged(link.gatt, 247, BluetoothGatt.GATT_SUCCESS)
                "watchdog" -> idleFor(3_000)
            }
            idle()
            assertEquals("exit=$exit", listOf(only.deviceId), link.shadow.reads)
        }
    }

    // --- Fakes --------------------------------------------------------------

    /** Everything the chain up to the device-id read touches on the host. */
    private class FakeHost : CentralGattClient.Host {
        override val protocol: OfflineProtocol
            get() = throw IllegalStateException("the core is not reached before the identity read")
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
        override fun drainAndSendFragments() {}
        override fun onWriteCompleted(address: String) {}
        override fun handleInboundFragment(address: String, data: ByteArray) {}
        override fun onPeerMtuNegotiated(address: String, maxPayload: Int) {}
        override fun onDeviceIdResolved(address: String, deviceId: String) {}
        override fun onPeerGivenUp(address: String, peerId: String) {}
        override fun connectToDevice(device: BluetoothDevice) {}
    }
}

/**
 * A `BluetoothGatt` that reports the services the test registered, records
 * every characteristic read the client issues, and answers nothing on its
 * own: the test delivers each answer through the client's callback.
 */
@Implements(BluetoothGatt::class)
class RecordingGatt : ShadowBluetoothGatt() {
    val registered = mutableListOf<BluetoothGattService>()
    val reads = mutableListOf<BluetoothGattCharacteristic>()
    var mtuRequests = 0
    var mtuResult = true
    var readResult = true
    var closed = false

    @Implementation
    override fun getServices(): List<BluetoothGattService> = registered.toList()

    @Implementation
    override fun getService(uuid: UUID): BluetoothGattService? = registered.firstOrNull { it.uuid == uuid }

    @Implementation
    protected fun readCharacteristic(characteristic: BluetoothGattCharacteristic): Boolean {
        reads += characteristic
        return readResult
    }

    @Implementation
    override fun requestMtu(mtu: Int): Boolean {
        mtuRequests += 1
        return mtuResult
    }

    @Implementation
    override fun discoverServices(): Boolean = true

    @Implementation
    override fun disconnect() { closed = true }

    @Implementation
    override fun close() { closed = true }
}
