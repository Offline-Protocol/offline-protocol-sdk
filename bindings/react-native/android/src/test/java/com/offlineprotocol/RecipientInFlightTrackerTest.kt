package com.offlineprotocol

import org.junit.Assert.assertEquals
import org.junit.Assert.assertTrue
import org.junit.Test

class RecipientInFlightTrackerTest {

    @Test
    fun drainReturnsLiveIdsAndClearsRecipient() {
        val tracker = RecipientInFlightTracker(ttlMs = 60_000, maxPerRecipient = 32)
        tracker.recordSent("bob", "m1", nowMs = 1_000)
        tracker.recordSent("bob", "m2", nowMs = 2_000)
        tracker.recordSent("carol", "m3", nowMs = 2_000)

        assertEquals(listOf("m1", "m2"), tracker.drainRecipient("bob", nowMs = 3_000))
        // Drained — a second DeliveryError must not double-fail the same ids.
        assertTrue(tracker.drainRecipient("bob", nowMs = 3_000).isEmpty())
        // Other recipients untouched.
        assertEquals(listOf("m3"), tracker.drainRecipient("carol", nowMs = 3_000))
    }

    @Test
    fun drainSkipsExpiredEntries() {
        val tracker = RecipientInFlightTracker(ttlMs = 1_000, maxPerRecipient = 32)
        tracker.recordSent("bob", "old", nowMs = 0)
        tracker.recordSent("bob", "fresh", nowMs = 1_500)

        assertEquals(listOf("fresh"), tracker.drainRecipient("bob", nowMs = 2_000))
    }

    @Test
    fun capDropsOldestFirst() {
        val tracker = RecipientInFlightTracker(ttlMs = 60_000, maxPerRecipient = 2)
        tracker.recordSent("bob", "m1", nowMs = 1)
        tracker.recordSent("bob", "m2", nowMs = 2)
        tracker.recordSent("bob", "m3", nowMs = 3)

        assertEquals(listOf("m2", "m3"), tracker.drainRecipient("bob", nowMs = 4))
    }

    @Test
    fun pruneEvictsExpiredEntriesAndEmptyRecipients() {
        val tracker = RecipientInFlightTracker(ttlMs = 1_000, maxPerRecipient = 32)
        tracker.recordSent("bob", "old", nowMs = 0)
        tracker.recordSent("carol", "live", nowMs = 1_800)

        tracker.prune(nowMs = 2_000)

        assertTrue(tracker.drainRecipient("bob", nowMs = 2_000).isEmpty())
        assertEquals(listOf("live"), tracker.drainRecipient("carol", nowMs = 2_000))
    }

    @Test
    fun relayAcceptedResolvesExactIdWhenItMatches() {
        val tracker = RecipientInFlightTracker(ttlMs = 60_000, maxPerRecipient = 32)
        tracker.recordSent("bob", "m1", nowMs = 1_000)
        tracker.recordSent("bob", "m2", nowMs = 2_000)

        tracker.resolveOnRelayAccepted("bob", "m2", nowMs = 3_000)

        // Only the accepted frame left the tracker; a later DeliveryError
        // still fails the genuinely unresolved one.
        assertEquals(listOf("m1"), tracker.drainRecipient("bob", nowMs = 4_000))
    }

    @Test
    fun relayAcceptedFallsBackToOldestForUnknownOrMissingId() {
        val tracker = RecipientInFlightTracker(ttlMs = 60_000, maxPerRecipient = 32)
        tracker.recordSent("bob", "m1", nowMs = 1_000)
        tracker.recordSent("bob", "m2", nowMs = 2_000)
        tracker.recordSent("bob", "m3", nowMs = 3_000)

        // The relay echoes a server-generated id: sends per recipient are
        // FIFO on one socket, so the answer belongs to the oldest in-flight.
        tracker.resolveOnRelayAccepted("bob", "server-id", nowMs = 4_000)
        tracker.resolveOnRelayAccepted("bob", null, nowMs = 4_000)

        assertEquals(listOf("m3"), tracker.drainRecipient("bob", nowMs = 5_000))
    }

    @Test
    fun relayAcceptedIgnoresExpiredEntriesAndUnknownRecipients() {
        val tracker = RecipientInFlightTracker(ttlMs = 1_000, maxPerRecipient = 32)
        tracker.recordSent("bob", "stale", nowMs = 0)
        tracker.recordSent("bob", "fresh", nowMs = 2_500)

        // The stale entry is expired housekeeping, not the oldest live send:
        // the answer must resolve "fresh", not be eaten by "stale".
        tracker.resolveOnRelayAccepted("bob", null, nowMs = 3_000)
        assertTrue(tracker.drainRecipient("bob", nowMs = 3_000).isEmpty())

        // No-ops, never throws.
        tracker.resolveOnRelayAccepted("nobody", "m1", nowMs = 3_000)
        tracker.resolveOnRelayAccepted("", "m1", nowMs = 3_000)
    }

    @Test
    fun ignoresEmptyInputsAndClearForgetsEverything() {
        val tracker = RecipientInFlightTracker()
        tracker.recordSent("", "m1", nowMs = 1)
        tracker.recordSent("bob", "", nowMs = 1)
        assertTrue(tracker.drainRecipient("", nowMs = 2).isEmpty())
        assertTrue(tracker.drainRecipient("bob", nowMs = 2).isEmpty())

        tracker.recordSent("bob", "m1", nowMs = 1)
        tracker.clear()
        assertTrue(tracker.drainRecipient("bob", nowMs = 2).isEmpty())
    }
}
