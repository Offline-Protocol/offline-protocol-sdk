package com.offlineprotocol

import com.offlineprotocol.RecipientInFlightTracker.Plane
import org.junit.Assert.assertEquals
import org.junit.Assert.assertTrue
import org.junit.Test

class RecipientInFlightTrackerTest {

    @Test
    fun drainReturnsLiveIdsAndClearsRecipient() {
        val tracker = RecipientInFlightTracker(ttlMs = 60_000, maxPerRecipient = 32)
        tracker.recordSent("bob", "m1", Plane.DATA, nowMs = 1_000)
        tracker.recordSent("bob", "m2", Plane.DATA, nowMs = 2_000)
        tracker.recordSent("carol", "m3", Plane.DATA, nowMs = 2_000)

        assertEquals(listOf("m1", "m2"), tracker.drainRecipient("bob", Plane.DATA, nowMs = 3_000))
        // Drained — a second DeliveryError must not double-fail the same ids.
        assertTrue(tracker.drainRecipient("bob", Plane.DATA, nowMs = 3_000).isEmpty())
        // Other recipients untouched.
        assertEquals(listOf("m3"), tracker.drainRecipient("carol", Plane.DATA, nowMs = 3_000))
    }

    @Test
    fun drainSkipsExpiredEntries() {
        val tracker = RecipientInFlightTracker(ttlMs = 1_000, maxPerRecipient = 32)
        tracker.recordSent("bob", "old", Plane.DATA, nowMs = 0)
        tracker.recordSent("bob", "fresh", Plane.DATA, nowMs = 1_500)

        assertEquals(listOf("fresh"), tracker.drainRecipient("bob", Plane.DATA, nowMs = 2_000))
    }

    @Test
    fun capDropsOldestFirst() {
        val tracker = RecipientInFlightTracker(ttlMs = 60_000, maxPerRecipient = 2)
        tracker.recordSent("bob", "m1", Plane.DATA, nowMs = 1)
        tracker.recordSent("bob", "m2", Plane.DATA, nowMs = 2)
        tracker.recordSent("bob", "m3", Plane.DATA, nowMs = 3)

        assertEquals(listOf("m2", "m3"), tracker.drainRecipient("bob", Plane.DATA, nowMs = 4))
    }

    @Test
    fun pruneEvictsExpiredEntriesAndEmptyRecipients() {
        val tracker = RecipientInFlightTracker(ttlMs = 1_000, maxPerRecipient = 32)
        tracker.recordSent("bob", "old", Plane.DATA, nowMs = 0)
        tracker.recordSent("carol", "live", Plane.DATA, nowMs = 1_800)

        tracker.prune(nowMs = 2_000)

        assertTrue(tracker.drainRecipient("bob", Plane.DATA, nowMs = 2_000).isEmpty())
        assertEquals(listOf("live"), tracker.drainRecipient("carol", Plane.DATA, nowMs = 2_000))
    }

    @Test
    fun pruneAppliesPerEntryRegardlessOfPlane() {
        val tracker = RecipientInFlightTracker(ttlMs = 1_000, maxPerRecipient = 32)
        tracker.recordSent("bob", "cr-old", Plane.CONN_REQ, nowMs = 0)
        tracker.recordSent("bob", "m-live", Plane.DATA, nowMs = 1_800)

        tracker.prune(nowMs = 2_000)

        assertTrue(tracker.drainRecipient("bob", Plane.CONN_REQ, nowMs = 2_000).isEmpty())
        assertEquals(listOf("m-live"), tracker.drainRecipient("bob", Plane.DATA, nowMs = 2_000))
    }

    @Test
    fun relayAcceptedResolvesExactIdWhenItMatches() {
        val tracker = RecipientInFlightTracker(ttlMs = 60_000, maxPerRecipient = 32)
        tracker.recordSent("bob", "m1", Plane.DATA, nowMs = 1_000)
        tracker.recordSent("bob", "m2", Plane.DATA, nowMs = 2_000)

        tracker.resolveOnRelayAccepted("bob", "m2", nowMs = 3_000)

        // Only the accepted frame left the tracker; a later DeliveryError
        // still fails the genuinely unresolved one.
        assertEquals(listOf("m1"), tracker.drainRecipient("bob", Plane.DATA, nowMs = 4_000))
    }

    @Test
    fun relayAcceptedResolvesOldestDataEntryNeverConnReq() {
        val tracker = RecipientInFlightTracker(ttlMs = 60_000, maxPerRecipient = 32)
        // The CONN_REQ entry is the oldest overall — a MessageSent must
        // still skip it: only SendMessage frames earn a MessageSent, so
        // oldest-first is only sound within the DATA plane.
        tracker.recordSent("bob", "cr1", Plane.CONN_REQ, nowMs = 500)
        tracker.recordSent("bob", "m1", Plane.DATA, nowMs = 1_000)
        tracker.recordSent("bob", "m2", Plane.DATA, nowMs = 2_000)

        // The relay echoes a server-generated id: resolves the oldest DATA
        // entry (data sends per recipient are FIFO on one socket).
        tracker.resolveOnRelayAccepted("bob", "server-id", nowMs = 3_000)
        tracker.resolveOnRelayAccepted("bob", null, nowMs = 3_000)

        assertTrue(tracker.drainRecipient("bob", Plane.DATA, nowMs = 4_000).isEmpty())
        // The CONN_REQ entry survives for its own error channel.
        assertEquals(listOf("cr1"), tracker.drainRecipient("bob", Plane.CONN_REQ, nowMs = 4_000))
    }

    @Test
    fun connReqEntryCannotAbsorbTheDataFramesMessageSent() {
        val tracker = RecipientInFlightTracker(ttlMs = 60_000, maxPerRecipient = 32)
        // Regression: a conn_req primary used to share the FIFO with data
        // sends, absorb the data frame's MessageSent, and leave the
        // delivered message tracked for a later DeliveryError to
        // false-fail (the core honors that even for wire-confirmed
        // welcomes and parks a delivered lifecycle).
        tracker.recordSent("bob", "cr1", Plane.CONN_REQ, nowMs = 1_000)
        tracker.recordSent("bob", "m1", Plane.DATA, nowMs = 2_000)

        tracker.resolveOnRelayAccepted("bob", "server-generated-id", nowMs = 3_000)

        // The accepted data frame left the tracker: the DeliveryError has
        // nothing to false-fail on the data plane.
        assertTrue(tracker.drainRecipient("bob", Plane.DATA, nowMs = 4_000).isEmpty())
    }

    @Test
    fun eachErrorSignalDrainsItsOwnPlaneOnly() {
        val tracker = RecipientInFlightTracker(ttlMs = 60_000, maxPerRecipient = 32)
        tracker.recordSent("bob", "cr1", Plane.CONN_REQ, nowMs = 1_000)
        tracker.recordSent("bob", "m1", Plane.DATA, nowMs = 2_000)

        // DeliveryError (DATA) leaves the CONN_REQ entry untouched.
        assertEquals(listOf("m1"), tracker.drainRecipient("bob", Plane.DATA, nowMs = 3_000))
        // ConnectionRequestError (CONN_REQ) then finds its own entry live.
        assertEquals(listOf("cr1"), tracker.drainRecipient("bob", Plane.CONN_REQ, nowMs = 3_000))

        // And in the reverse order: ConnectionRequestError leaves DATA
        // entries untouched.
        tracker.recordSent("bob", "m2", Plane.DATA, nowMs = 4_000)
        tracker.recordSent("bob", "cr2", Plane.CONN_REQ, nowMs = 5_000)
        assertEquals(listOf("cr2"), tracker.drainRecipient("bob", Plane.CONN_REQ, nowMs = 6_000))
        assertEquals(listOf("m2"), tracker.drainRecipient("bob", Plane.DATA, nowMs = 6_000))
    }

    @Test
    fun relayAcceptedIgnoresExpiredEntriesAndUnknownRecipients() {
        val tracker = RecipientInFlightTracker(ttlMs = 1_000, maxPerRecipient = 32)
        tracker.recordSent("bob", "stale", Plane.DATA, nowMs = 0)
        tracker.recordSent("bob", "fresh", Plane.DATA, nowMs = 2_500)

        // The stale entry is expired housekeeping, not the oldest live send:
        // the answer must resolve "fresh", not be eaten by "stale".
        tracker.resolveOnRelayAccepted("bob", null, nowMs = 3_000)
        assertTrue(tracker.drainRecipient("bob", Plane.DATA, nowMs = 3_000).isEmpty())

        // No-ops, never throws.
        tracker.resolveOnRelayAccepted("nobody", "m1", nowMs = 3_000)
        tracker.resolveOnRelayAccepted("", "m1", nowMs = 3_000)
    }

    @Test
    fun ignoresEmptyInputsAndClearForgetsEverything() {
        val tracker = RecipientInFlightTracker()
        tracker.recordSent("", "m1", Plane.DATA, nowMs = 1)
        tracker.recordSent("bob", "", Plane.DATA, nowMs = 1)
        assertTrue(tracker.drainRecipient("", Plane.DATA, nowMs = 2).isEmpty())
        assertTrue(tracker.drainRecipient("bob", Plane.DATA, nowMs = 2).isEmpty())

        tracker.recordSent("bob", "m1", Plane.DATA, nowMs = 1)
        tracker.recordSent("bob", "cr1", Plane.CONN_REQ, nowMs = 1)
        tracker.clear()
        assertTrue(tracker.drainRecipient("bob", Plane.DATA, nowMs = 2).isEmpty())
        assertTrue(tracker.drainRecipient("bob", Plane.CONN_REQ, nowMs = 2).isEmpty())
    }
}
