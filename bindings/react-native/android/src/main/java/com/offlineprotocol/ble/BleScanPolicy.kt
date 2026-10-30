package com.offlineprotocol.ble

/**
 * What a Bluetooth LE scan asks the stack for, and what its silence means,
 * by screen state.
 *
 * Since Android 8.1 an unfiltered scan is paused while the screen is off,
 * and a filtered one keeps running. The facade scanned unfiltered whatever
 * the screen did, so a phone in a pocket discovered nobody, and the scan
 * watchdog read the silence as a stalled stack: it restarted the scan every
 * 30 seconds and, every third time, reset the stack, which restarted the
 * advertisement. A restarted advertisement has a new random address, and a
 * peer already linked then dials an address pair the Link Layer never
 * connects (ADR 0030). Measured on 2026-10-30 with the screen locked: a
 * restart every 30 seconds and a reset every 90.
 *
 * So with the screen off the scan names the mesh service, which every SDK
 * advertiser puts in its primary packet, and its silence is not a stall: a
 * pocket in a quiet place is quiet. iOS pins the same two rules against the
 * app's background state in BleScanPolicy.swift.
 */
internal object BleScanPolicy {
    /** Whether the scan filters on the mesh service: only with the screen off. */
    fun filtersOnService(screenOn: Boolean): Boolean = !screenOn

    /** Whether a scan that has delivered nothing may be read as stalled. */
    fun mayRestartQuietScan(screenOn: Boolean): Boolean = screenOn
}
