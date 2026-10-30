package com.offlineprotocol.ble

import org.junit.Assert.assertFalse
import org.junit.Assert.assertTrue
import org.junit.Test

class BleScanPolicyTest {
    @Test
    fun `with the screen on the scan is unfiltered`() {
        assertFalse(BleScanPolicy.filtersOnService(screenOn = true))
    }

    @Test
    fun `with the screen off the scan names the mesh service`() {
        // The stack pauses an unfiltered scan while the screen is off.
        assertTrue(BleScanPolicy.filtersOnService(screenOn = false))
    }

    @Test
    fun `a quiet scan with the screen on may be restarted`() {
        assertTrue(BleScanPolicy.mayRestartQuietScan(screenOn = true))
    }

    @Test
    fun `a quiet scan with the screen off is not a stall`() {
        // Reading it as one restarted the advertisement every 90 seconds.
        assertFalse(BleScanPolicy.mayRestartQuietScan(screenOn = false))
    }
}
