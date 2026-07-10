package com.offlineprotocol

import org.junit.Assert.assertEquals
import org.junit.Assert.assertNull
import org.junit.Test

/**
 * Mirrors ios/tests/RelayTimestampsTests.swift — keep in sync.
 */
class RelayTimestampsTest {

    @Test
    fun parsesEpochMilliseconds() {
        assertEquals(1720000000000L, RelayTimestamps.parseToMsOrNull("1720000000000"))
    }

    @Test
    fun parsesIso8601WithFractionalSeconds() {
        assertEquals(
            1704067200500L,
            RelayTimestamps.parseToMsOrNull("2024-01-01T00:00:00.500Z")
        )
    }

    @Test
    fun parsesIso8601WithoutFractionalSeconds() {
        assertEquals(
            1704067200000L,
            RelayTimestamps.parseToMsOrNull("2024-01-01T00:00:00Z")
        )
    }

    @Test
    fun absentOrUnparseableReturnsNullInsteadOfInventingNow() {
        assertNull(RelayTimestamps.parseToMsOrNull(""))
        assertNull(RelayTimestamps.parseToMsOrNull("not-a-timestamp"))
        assertNull(RelayTimestamps.parseToMsOrNull("2024-13-45T99:99:99Z"))
    }

    @Test
    fun epochSecondsAreScaledToMilliseconds() {
        // ~2024-07 as epoch seconds; without the heuristic this would render
        // as January 1970 in a last-seen display.
        assertEquals(1720000000000L, RelayTimestamps.parseToMsOrNull("1720000000"))
        assertEquals(1720000000000L, RelayTimestamps.normalizeEpochToMs(1720000000L))
        // Already-milliseconds values pass through untouched.
        assertEquals(1720000000000L, RelayTimestamps.normalizeEpochToMs(1720000000000L))
    }
}
