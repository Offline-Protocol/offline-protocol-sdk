package com.offlineprotocol.ble

import org.junit.Assert.assertEquals
import org.junit.Assert.assertNull
import org.junit.Test

/**
 * Unit tests for [resolveSubscribedAddress] — the device-scoped resolution that
 * ties the per-peer MTU floor ([BleTransportFacade.flushPeerMtu]) to the notify
 * egress ([BleTransportFacade.sendFragmentData]). Both call sites resolve
 * notify-reachability through this one predicate, so the floor is applied for
 * exactly the peers the notify path can reach.
 *
 * The regression these guard against: if the floor and the egress ever disagreed
 * about which peers are notify-subscribed, a multi-fragment MLS Welcome egressed
 * over an unobserved notify link would be sized for the larger central link,
 * overflow the notify link, and be silently truncated on air — stalling 1:1 MLS
 * convergence. Resolution is by DEVICE ID because the two links can use different
 * BLE addresses for the same peer (iOS uses distinct handles per direction).
 */
class SubscribedAddressResolutionTest {
    /**
     * Stand-in for [MeshConnectionRegistry.deviceIdForAddress]: an in-memory
     * address -> deviceId map where an unregistered address resolves to null.
     */
    private fun resolverOf(vararg pairs: Pair<String, String>): (String) -> String? {
        val map = pairs.toMap()
        return { address -> map[address] }
    }

    @Test
    fun `resolves the subscribed address that maps to the device by identity`() {
        // The peer subscribed under a peripheral-link address ("periph") distinct
        // from the central link we hold ("central"); both resolve to the same id.
        val resolve = resolverOf("central" to "peerA", "periph" to "peerA")
        assertEquals(
            "periph",
            resolveSubscribedAddress(
                deviceId = "peerA",
                subscribedAddresses = listOf("periph"),
                resolveDeviceId = resolve,
            ),
        )
    }

    @Test
    fun `null when no subscribed address maps to the device`() {
        // Every subscribed address belongs to OTHER peers -> not notify-reachable.
        val resolve = resolverOf("p1" to "peerB", "p2" to "peerC")
        assertNull(
            resolveSubscribedAddress(
                deviceId = "peerA",
                subscribedAddresses = listOf("p1", "p2"),
                resolveDeviceId = resolve,
            ),
        )
    }

    @Test
    fun `a subscribed address that has not resolved to any device never matches`() {
        // The address subscribed but its device-id read has not completed
        // (deviceIdForAddress -> null); it must not match any real peer.
        val resolve = resolverOf() // every lookup returns null
        assertNull(
            resolveSubscribedAddress(
                deviceId = "peerA",
                subscribedAddresses = listOf("unresolved"),
                resolveDeviceId = resolve,
            ),
        )
    }

    @Test
    fun `picks the matching address among non-matching distractors`() {
        val resolve = resolverOf(
            "other1" to "peerB",
            "mine" to "peerA",
            "other2" to "peerC",
        )
        assertEquals(
            "mine",
            resolveSubscribedAddress(
                deviceId = "peerA",
                subscribedAddresses = listOf("other1", "mine", "other2"),
                resolveDeviceId = resolve,
            ),
        )
    }

    @Test
    fun `null when nothing is subscribed`() {
        // No CCCD subscribers at all -> the peer is not notify-reachable even
        // though its central link is known.
        assertNull(
            resolveSubscribedAddress(
                deviceId = "peerA",
                subscribedAddresses = emptyList(),
                resolveDeviceId = resolverOf("central" to "peerA"),
            ),
        )
    }
}
