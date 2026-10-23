package com.offlineprotocol.ble

import org.junit.Assert.assertFalse
import org.junit.Assert.assertTrue
import org.junit.Test

class AdvertisingRefreshPolicyTest {
    @Test
    fun `with no identified link the advertisement is refreshed`() {
        assertTrue(AdvertisingRefreshPolicy.restartsAdvertising(identifiedLinks = 0, isAdvertising = true))
    }

    @Test
    fun `a running advertisement is left alone while a peer is linked`() {
        assertFalse(AdvertisingRefreshPolicy.restartsAdvertising(identifiedLinks = 1, isAdvertising = true))
    }

    @Test
    fun `a stopped advertisement is restarted even while linked`() {
        assertTrue(AdvertisingRefreshPolicy.restartsAdvertising(identifiedLinks = 2, isAdvertising = false))
    }
}
