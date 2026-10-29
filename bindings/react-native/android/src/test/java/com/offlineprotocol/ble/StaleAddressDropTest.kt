package com.offlineprotocol.ble

import org.junit.Assert.assertFalse
import org.junit.Assert.assertTrue
import org.junit.Test

/**
 * [dropsAsStaleAddress] decides whether a dropped client link is redialled.
 * The failure it exists for: a silent link to an iPhone that also wrote to our
 * server was disconnected to redial it, and the disconnect path dropped its
 * address as stale instead, so the redial waited on an advertisement.
 */
class StaleAddressDropTest {
    @Test
    fun `an address the peer has left for another link is dropped`() {
        assertTrue(dropsAsStaleAddress(redialRequested = false, peerHasOtherLiveLink = true))
    }

    @Test
    fun `a requested redial is kept even when the peer holds another link`() {
        assertFalse(dropsAsStaleAddress(redialRequested = true, peerHasOtherLiveLink = true))
    }

    @Test
    fun `the peer's only link is redialled`() {
        assertFalse(dropsAsStaleAddress(redialRequested = false, peerHasOtherLiveLink = false))
    }
}
