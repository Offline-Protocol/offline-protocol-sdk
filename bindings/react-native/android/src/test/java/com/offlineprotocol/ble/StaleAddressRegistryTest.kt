package com.offlineprotocol.ble

import org.junit.Assert.assertEquals
import org.junit.Assert.assertFalse
import org.junit.Assert.assertTrue
import org.junit.Test

/**
 * A peer that reconnects from a new BLE address (iOS rotates its random address
 * across a Bluetooth power-cycle) leaves its old address behind. Giving up on the
 * stale address must not orphan the live one or report the live peer as lost.
 */
class StaleAddressRegistryTest {
    @Test
    fun `dropping a stale address keeps the live mapping for the same peer`() {
        val registry = MeshConnectionRegistry()
        registry.setDeviceIdentifier("old", "peerA")
        registry.setDeviceIdentifier("new", "peerA")
        registry.trackServerConnection("new")

        assertTrue(registry.hasOtherLiveLink("peerA", excluding = "old"))
        registry.removeIdentifiersForAddress("old")

        assertEquals("new", registry.addressForDevice("peerA"))
        assertEquals("peerA", registry.deviceIdForAddress("new"))
    }

    @Test
    fun `a peer with no other live link is not kept alive`() {
        val registry = MeshConnectionRegistry()
        registry.setDeviceIdentifier("only", "peerB")
        registry.trackServerConnection("only")

        assertFalse(registry.hasOtherLiveLink("peerB", excluding = "only"))
        registry.removeIdentifiersForAddress("only")
        assertEquals(null, registry.addressForDevice("peerB"))
    }

    @Test
    fun `deviceIds lists each identified peer once`() {
        val registry = MeshConnectionRegistry()
        registry.setDeviceIdentifier("old", "peerA")
        registry.setDeviceIdentifier("new", "peerA")
        registry.setDeviceIdentifier("other", "peerB")

        assertEquals(setOf("peerA", "peerB"), registry.deviceIds())
        registry.clear()
        assertTrue(registry.deviceIds().isEmpty())
    }
}
