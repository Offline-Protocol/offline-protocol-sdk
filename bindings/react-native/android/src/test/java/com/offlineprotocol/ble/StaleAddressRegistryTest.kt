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
    fun `a live link at an address that resolved earlier still counts`() {
        // A hello maps the peer's central-role address last, so the peer's
        // address is the one our client link may be giving up on. Its server
        // link at the other address is live, and must keep it from being
        // reported lost.
        val registry = MeshConnectionRegistry()
        registry.setDeviceIdentifier("client-side", "peerC")
        registry.setDeviceIdentifier("server-side", "peerC")
        registry.trackServerConnection("server-side")
        registry.setDeviceIdentifier("client-side", "peerC")
        assertEquals("client-side", registry.addressForDevice("peerC"))

        assertTrue(registry.hasOtherLiveLink("peerC", excluding = "client-side"))

        registry.removeIdentifiersForAddress("client-side")
        assertEquals(
            "the surviving address becomes the peer's address",
            "server-side",
            registry.addressForDevice("peerC"),
        )
    }

    @Test
    fun `removing every address of a two-address peer leaves nothing mapped`() {
        // What eviction does: a peer known at its client-side address and at
        // the server-side address a hello mapped must lose both, or the one
        // left behind keeps a link the core was told is gone.
        val registry = MeshConnectionRegistry()
        registry.setDeviceIdentifier("client-side", "peerE")
        registry.setDeviceIdentifier("server-side", "peerE")
        registry.trackServerConnection("server-side")

        for (address in registry.addressesForDevice("peerE")) {
            registry.untrackServerConnection(address)
            registry.removeIdentifiersForAddress(address)
        }

        assertEquals(emptyList<String>(), registry.addressesForDevice("peerE"))
        assertEquals(null, registry.addressForDevice("peerE"))
        assertEquals(0, registry.serverConnectionCount())
        assertTrue(!registry.hasEstablishedLink("peerE"))
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

    @Test
    fun `an established link at any of a peer's addresses keeps it live`() {
        // deviceToAddress holds only the address that resolved last, so a
        // check through it misses a link from the peer's other address.
        val registry = MeshConnectionRegistry()
        registry.setDeviceIdentifier("server-side", "peerA")
        registry.trackServerConnection("server-side")
        registry.setDeviceIdentifier("client-side", "peerA")

        assertTrue(registry.hasEstablishedLink("peerA"))
        registry.untrackServerConnection("server-side")
        assertFalse(registry.hasEstablishedLink("peerA"))
        assertEquals(setOf("server-side", "client-side"), registry.addressesForDevice("peerA").toSet())
    }

    @Test
    fun `a hello-mapped link that closes gives the peer back its live address`() {
        // We hold a client link to peerA at its advertising address. Its
        // central then opened a link to our server from its central-role
        // address and wrote a hello, which made that address peerA's
        // send address. When that link closes, sending must go back to the
        // client link rather than to an address that does not advertise.
        val registry = MeshConnectionRegistry()
        registry.setDeviceIdentifier("advertising", "peerA")
        registry.trackServerConnection("central-role")
        registry.setDeviceIdentifier("central-role", "peerA")
        assertEquals("central-role", registry.addressForDevice("peerA"))

        registry.untrackServerConnection("central-role")
        // A live link at the advertising address. The registry's client map
        // needs a BluetoothGatt; a server link counts the same for liveness.
        registry.trackServerConnection("advertising")
        assertTrue(registry.hasOtherLiveLink("peerA", excluding = "central-role"))
        registry.removeIdentifiersForAddress("central-role")

        assertEquals("advertising", registry.addressForDevice("peerA"))
        assertEquals(null, registry.deviceIdForAddress("central-role"))
    }
}
