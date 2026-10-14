package com.offlineprotocol

import org.junit.Assert.assertEquals
import org.junit.Assert.assertNull
import org.junit.Test

/**
 * The record a LAN peer is found by. Browsing, resolving and dialing need a
 * device and a network; this is what the manager reads off a record.
 */
class LanPeerDiscoveryTest {
    private val address = "off1qysluvwl5922yctzd0u9gpr06gn3k7ldfvgtwgvn"

    private fun record(vararg entries: Pair<String, String?>): Map<String, ByteArray?> =
        entries.associate { (key, value) -> key to value?.encodeToByteArray() }

    @Test
    fun `the instance name is the one iOS and Python give the same address`() {
        assertEquals("op-b701bfc1c92768b5", LanPeerDiscovery.instanceName(address))
    }

    @Test
    fun `an iOS or Python record names its address`() {
        assertEquals(address, LanPeerDiscovery.lanAdvertAddress(record("txtvers" to "1", "addr" to address)))
    }

    @Test
    fun `an app entry is allowed, not required`() {
        assertEquals(
            address,
            LanPeerDiscovery.lanAdvertAddress(record("txtvers" to "1", "addr" to address, "app" to "0a1b2c3d")),
        )
    }

    @Test
    fun `records that are not peer hints are ignored`() {
        // A service instance under the DNS-SD mapping chapter.
        assertNull(LanPeerDiscovery.lanAdvertAddress(record("txtvers" to "1", "addr" to address, "sid" to "x")))
        assertNull(LanPeerDiscovery.lanAdvertAddress(record("txtvers" to "2", "addr" to address)))
        assertNull(LanPeerDiscovery.lanAdvertAddress(record("addr" to address)))
        assertNull(LanPeerDiscovery.lanAdvertAddress(record("txtvers" to "1")))
        assertNull(LanPeerDiscovery.lanAdvertAddress(record("txtvers" to "1", "addr" to null)))
        assertNull(LanPeerDiscovery.lanAdvertAddress(record("txtvers" to "1", "addr" to "not-an-address")))
    }
}
