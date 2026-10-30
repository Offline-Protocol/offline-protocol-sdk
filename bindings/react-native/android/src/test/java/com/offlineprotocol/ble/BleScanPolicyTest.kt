package com.offlineprotocol.ble

import android.bluetooth.le.ScanSettings
import org.junit.Assert.assertEquals
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
    fun `a quiet scan with the screen on is restarted on the watchdog's interval and escalates`() {
        assertEquals(30_000L, BleScanPolicy.quietRestartAfterMs(screenOn = true, screenOnMs = 30_000L))
        assertTrue(BleScanPolicy.quietRestartEscalates(screenOn = true))
    }

    @Test
    fun `a quiet scan with the screen off is restarted slowly and never escalates`() {
        // Escalating restarted the advertisement every 90 seconds; never
        // restarting left a scan Android refused dead for the whole pocket.
        assertEquals(
            BleScanPolicy.SCREEN_OFF_QUIET_RESTART_MS,
            BleScanPolicy.quietRestartAfterMs(screenOn = false, screenOnMs = 30_000L),
        )
        assertTrue(BleScanPolicy.SCREEN_OFF_QUIET_RESTART_MS >= 60_000L)
        assertFalse(BleScanPolicy.quietRestartEscalates(screenOn = false))
    }

    @Test
    fun `a screen-off scan runs at a lower duty than a screen-on one`() {
        assertEquals(ScanSettings.SCAN_MODE_LOW_LATENCY, BleScanPolicy.scanMode(screenOn = true))
        // Before the policy the stack paused this scan; full duty in a pocket
        // would listen continuously to every peer nearby.
        assertEquals(ScanSettings.SCAN_MODE_BALANCED, BleScanPolicy.scanMode(screenOn = false))
    }

    @Test
    fun `a screen transition waits long enough to absorb a glance`() {
        assertTrue(BleScanPolicy.SCREEN_TRANSITION_DEBOUNCE_MS in 1_000L..5_000L)
    }
}
