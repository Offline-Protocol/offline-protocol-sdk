package com.offlineprotocol

import org.junit.Assert.assertFalse
import org.junit.Assert.assertTrue
import org.junit.Assert.assertEquals
import org.junit.Test

/**
 * Pins the shared foreground-reconnect gate (see ForegroundReconnectPolicy.kt).
 * The iOS side has a paired ForegroundReconnectPolicyTests.swift asserting the
 * same rule. Times are millisecond longs from the caller's monotonic clock; the
 * tests drive them directly.
 */
class ForegroundReconnectPolicyTest {

    @Test
    fun coldLaunchForegroundDoesNotReconnect() {
        // No background was ever recorded → foreground must not reconnect.
        val policy = ForegroundReconnectPolicy(minBackgroundIntervalMs = 4_000L)
        assertFalse(policy.shouldReconnectOnForeground(nowMs = 1_000_000L))
    }

    @Test
    fun backgroundBelowThresholdDoesNotReconnect() {
        // Quick app-switch: 3.999s away → keep the live socket.
        val policy = ForegroundReconnectPolicy(minBackgroundIntervalMs = 4_000L)
        policy.didEnterBackground(nowMs = 1_000L)
        assertFalse(policy.shouldReconnectOnForeground(nowMs = 1_000L + 3_999L))
    }

    @Test
    fun backgroundExactlyAtThresholdReconnects() {
        // Boundary: at-or-above the window reconnects (>= threshold).
        val policy = ForegroundReconnectPolicy(minBackgroundIntervalMs = 4_000L)
        policy.didEnterBackground(nowMs = 0L)
        assertTrue(policy.shouldReconnectOnForeground(nowMs = 4_000L))
    }

    @Test
    fun backgroundPastThresholdReconnects() {
        val policy = ForegroundReconnectPolicy(minBackgroundIntervalMs = 4_000L)
        policy.didEnterBackground(nowMs = 0L)
        assertTrue(policy.shouldReconnectOnForeground(nowMs = 60_000L))
    }

    @Test
    fun foregroundConsumesTheTimestamp() {
        // A second foreground with no intervening background must not re-fire,
        // even if time has moved well past the threshold.
        val policy = ForegroundReconnectPolicy(minBackgroundIntervalMs = 4_000L)
        policy.didEnterBackground(nowMs = 0L)
        assertTrue(policy.shouldReconnectOnForeground(nowMs = 10_000L))
        assertFalse(policy.shouldReconnectOnForeground(nowMs = 20_000L))
    }

    @Test
    fun reArmingAfterConsumeReconnectsAgain() {
        // background → foreground (fires) → background → foreground (fires again).
        val policy = ForegroundReconnectPolicy(minBackgroundIntervalMs = 4_000L)
        policy.didEnterBackground(nowMs = 0L)
        assertTrue(policy.shouldReconnectOnForeground(nowMs = 5_000L))
        policy.didEnterBackground(nowMs = 6_000L)
        assertTrue(policy.shouldReconnectOnForeground(nowMs = 6_000L + 4_000L))
    }

    @Test
    fun sleepInclusiveElapsedIsHonoured() {
        // The caller supplies sleep-inclusive time; a large jump (device slept
        // in the background) counts toward the window like any other elapsed ms.
        val policy = ForegroundReconnectPolicy(minBackgroundIntervalMs = 4_000L)
        policy.didEnterBackground(nowMs = 100L)
        assertTrue(policy.shouldReconnectOnForeground(nowMs = 100L + 3_600_000L))
    }

    @Test
    fun defaultThresholdIsFourSeconds() {
        assertEquals(4_000L, ForegroundReconnectPolicy.DEFAULT_MIN_BACKGROUND_INTERVAL_MS)
        val policy = ForegroundReconnectPolicy()
        policy.didEnterBackground(nowMs = 0L)
        assertFalse(policy.shouldReconnectOnForeground(nowMs = 3_999L))
        policy.didEnterBackground(nowMs = 0L)
        assertTrue(policy.shouldReconnectOnForeground(nowMs = 4_000L))
    }
}
