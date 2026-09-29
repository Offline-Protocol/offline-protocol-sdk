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
    fun parsesANumericOffset() {
        // The same instant as 2024-01-01T00:00:00.500Z, written east and west.
        assertEquals(1704067200500L, RelayTimestamps.parseToMsOrNull("2024-01-01T05:30:00.500+05:30"))
        assertEquals(1704067200500L, RelayTimestamps.parseToMsOrNull("2023-12-31T19:00:00.500-05:00"))
    }

    @Test
    fun truncatesAFractionToTheMillisecond() {
        assertEquals(1704067200123L, RelayTimestamps.parseToMsOrNull("2024-01-01T00:00:00.123987654Z"))
        assertEquals(1704067200100L, RelayTimestamps.parseToMsOrNull("2024-01-01T00:00:00.1Z"))
    }

    /**
     * What `java.time.Instant.parse` answered before the parse was written out
     * (it needs API 26 and minSdk is 24). Values from `Instant` on the JVM.
     */
    @Test
    fun agreesWithInstantAcrossTheCalendar() {
        val cases = listOf(
            "1970-01-01T00:00:00Z",
            "1999-12-31T23:59:59.999Z",
            "2000-02-29T12:00:00Z",
            "2024-02-29T23:59:59Z",
            "2100-03-01T00:00:00Z",
            "2026-09-29T18:06:18.042Z",
            "1969-12-31T23:59:59Z",
        )
        for (case in cases) {
            assertEquals(case, java.time.Instant.parse(case).toEpochMilli(), RelayTimestamps.parseToMsOrNull(case))
        }
    }

    @Test
    fun refusesADateThatDoesNotExist() {
        assertNull(RelayTimestamps.parseToMsOrNull("2023-02-29T00:00:00Z"))
        assertNull(RelayTimestamps.parseToMsOrNull("2100-02-29T00:00:00Z"))
        assertNull(RelayTimestamps.parseToMsOrNull("2024-04-31T00:00:00Z"))
        assertNull(RelayTimestamps.parseToMsOrNull("2024-01-01T24:00:00Z"))
    }

    @Test
    fun refusesWhatIsNotAnInternetDateTime() {
        // No zone: a local time, which names no instant.
        assertNull(RelayTimestamps.parseToMsOrNull("2024-01-01T00:00:00"))
        assertNull(RelayTimestamps.parseToMsOrNull("2024-01-01 00:00:00Z"))
        assertNull(RelayTimestamps.parseToMsOrNull("2024-01-01T00:00:00.Z"))
        assertNull(RelayTimestamps.parseToMsOrNull(" 2024-01-01T00:00:00Z"))
        assertNull(RelayTimestamps.parseToMsOrNull("2024-01-01T00:00:00+0530"))
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
