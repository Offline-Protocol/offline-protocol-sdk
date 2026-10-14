package com.offlineprotocol.ble

import org.junit.Assert.assertEquals
import org.junit.Assert.assertFalse
import org.junit.Assert.assertTrue
import org.junit.Test
import org.junit.runner.RunWith
import org.robolectric.RobolectricTestRunner
import org.robolectric.annotation.Config
import org.robolectric.shadows.ShadowBluetoothDevice
import org.robolectric.shadows.ShadowBluetoothGatt

/**
 * The connection cap counts links, not map entries. Android reports a client
 * link of ours to the GATT server as a central connecting, so one link to a
 * peer can sit in both maps; counted twice, two dials filled a cap of four and
 * the cap check then cancelled the server-side half of our own link. A dial
 * still in flight is not a completed link, and must not turn a peer away.
 */
@RunWith(RobolectricTestRunner::class)
@Config(sdk = [34])
class ConnectionCapCountTest {

    private fun gatt(address: String) =
        ShadowBluetoothGatt.newInstance(ShadowBluetoothDevice.newInstance(address))

    @Test
    fun `a dial and its server-side report count as one link`() {
        val registry = MeshConnectionRegistry()
        registry.registerGatt("AA:AA:AA:AA:AA:01", gatt("AA:AA:AA:AA:AA:01"))
        registry.trackServerConnection("AA:AA:AA:AA:AA:01")

        assertEquals(1, registry.connectionCount())
    }

    @Test
    fun `links at different addresses each count`() {
        val registry = MeshConnectionRegistry()
        registry.registerGatt("AA:AA:AA:AA:AA:01", gatt("AA:AA:AA:AA:AA:01"))
        registry.trackServerConnection("AA:AA:AA:AA:AA:02")

        assertEquals(2, registry.connectionCount())
    }

    @Test
    fun `a dial in flight is not an established link`() {
        val registry = MeshConnectionRegistry()
        registry.registerGatt("BB:BB:BB:BB:BB:01", gatt("BB:BB:BB:BB:BB:01"))
        registry.trackServerConnection("BB:BB:BB:BB:BB:02")

        assertEquals(1, registry.establishedLinkCount())
        assertFalse(registry.isClientEstablished("BB:BB:BB:BB:BB:01"))

        registry.markClientEstablished("BB:BB:BB:BB:BB:01")
        assertTrue(registry.isClientEstablished("BB:BB:BB:BB:BB:01"))
        assertEquals(2, registry.establishedLinkCount())
    }

    @Test
    fun `an established link in both directions at one address counts once`() {
        val registry = MeshConnectionRegistry()
        registry.registerGatt("CC:CC:CC:CC:CC:01", gatt("CC:CC:CC:CC:CC:01"))
        registry.markClientEstablished("CC:CC:CC:CC:CC:01")
        registry.trackServerConnection("CC:CC:CC:CC:CC:01")

        assertEquals(1, registry.establishedLinkCount())
    }

    @Test
    fun `closing a dial frees its slot`() {
        val registry = MeshConnectionRegistry()
        registry.registerGatt("DD:DD:DD:DD:DD:01", gatt("DD:DD:DD:DD:DD:01"))
        assertEquals(1, registry.connectionCount())

        registry.removeGatt("DD:DD:DD:DD:DD:01")

        assertEquals(0, registry.connectionCount())
    }
}
