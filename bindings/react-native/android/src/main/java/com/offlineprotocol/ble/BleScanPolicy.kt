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
 * pocket in a quiet place is quiet. It is still restarted, slowly and without
 * escalating, because a scan can also be dead rather than quiet: Android
 * refuses an app's sixth scan start within 30 seconds without a callback, and
 * each screen transition is a start. Nothing else would notice a dead scan
 * with the screen off, since the proactive refresh runs only on a result.
 *
 * iOS names the service in the background too (BleScanPolicy.swift), and
 * never restarts there: it refuses no start, and a restart costs a
 * backgrounded app its next wake-up.
 */
internal object BleScanPolicy {
    /** How long a screen-off scan may deliver nothing before it is restarted. */
    const val SCREEN_OFF_QUIET_RESTART_MS = 5 * 60_000L

    /** How long a screen transition waits before moving the scan, so a glance
     *  at the phone (on, then off again) costs no scan starts. */
    const val SCREEN_TRANSITION_DEBOUNCE_MS = 2_000L

    /** Whether the scan filters on the mesh service: only with the screen off. */
    fun filtersOnService(screenOn: Boolean): Boolean = !screenOn

    /** How long a scan may deliver nothing before the watchdog restarts it:
     *  the watchdog's own interval with the screen on, and minutes with it off. */
    fun quietRestartAfterMs(screenOn: Boolean, screenOnMs: Long): Long =
        if (screenOn) screenOnMs else SCREEN_OFF_QUIET_RESTART_MS

    /** Whether a quiet restart counts toward the stack reset: only with the
     *  screen on. A reset restarts the advertisement, and a pocket's quiet is
     *  not evidence of a broken stack. */
    fun quietRestartEscalates(screenOn: Boolean): Boolean = screenOn
}
