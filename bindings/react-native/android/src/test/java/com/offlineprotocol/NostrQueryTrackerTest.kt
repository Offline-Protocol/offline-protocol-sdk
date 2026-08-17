package com.offlineprotocol

import org.junit.Assert.assertEquals
import org.junit.Assert.assertFalse
import org.junit.Assert.assertNull
import org.junit.Assert.assertTrue
import org.junit.Test

/**
 * The completion rule for a broadcast resolution query.
 *
 * Mirrors ios/tests/NostrQueryTrackerTests.swift case for case. The behaviour
 * under test is a correctness property rather than a latency one: a username
 * claim is meant to need only one honest relay to survive, so a query that
 * completed on the first end-of-stored-events would hand the whole answer to
 * whichever relay was fastest, and a relay holding nothing wins that race by
 * having nothing to send.
 */
class NostrQueryTrackerTest {

    private val relayA = "wss://a.example"
    private val relayB = "wss://b.example"
    private val relayC = "wss://c.example"

    /**
     * **The finding this class exists for.**
     *
     * The first relay's EOSE must not complete the query. If it did, every
     * record the slower relays are still sending would be discarded, which for
     * a username resolution is the answer itself.
     */
    @Test
    fun `first end of stored events does not complete a broadcast query`() {
        val tracker = NostrQueryTracker()
        tracker.issue("q1", listOf(relayA, relayB, relayC), nowMs = 0)

        assertFalse(
            "one relay answering is not the whole answer",
            tracker.noteEndOfStoredEvents("q1", relayA)
        )
        assertFalse(tracker.noteEndOfStoredEvents("q1", relayB))
        assertTrue(
            "the last relay owed completes it",
            tracker.noteEndOfStoredEvents("q1", relayC)
        )
        assertFalse("and it is no longer in flight", tracker.isActive("q1"))
    }

    /** A single-relay query completes on that relay's EOSE. */
    @Test
    fun `a query sent to one relay completes on its answer`() {
        val tracker = NostrQueryTracker()
        tracker.issue("q1", listOf(relayA), nowMs = 0)
        assertTrue(tracker.noteEndOfStoredEvents("q1", relayA))
    }

    /**
     * Events under an active subscription are resolution records and go to a
     * different entry point than inbound messages.
     */
    @Test
    fun `only issued queries are active`() {
        val tracker = NostrQueryTracker()
        assertFalse(tracker.isActive("q1"))
        tracker.issue("q1", listOf(relayA), nowMs = 0)
        assertTrue(tracker.isActive("q1"))
    }

    /**
     * A relay that went away will never send its EOSE, so it must stop being
     * waited on or the query sits until its deadline for an answer that cannot
     * arrive.
     */
    @Test
    fun `a disconnecting relay stops being awaited and can complete a query`() {
        val tracker = NostrQueryTracker()
        tracker.issue("q1", listOf(relayA, relayB), nowMs = 0)
        tracker.noteEndOfStoredEvents("q1", relayA)

        assertEquals(
            "dropping the last relay owed finishes the query",
            listOf("q1"),
            tracker.dropRelay(relayB)
        )
        assertFalse(tracker.isActive("q1"))
    }

    /** A disconnect that still leaves relays owed must not complete anything. */
    @Test
    fun `a disconnecting relay leaves a query that others still owe`() {
        val tracker = NostrQueryTracker()
        tracker.issue("q1", listOf(relayA, relayB), nowMs = 0)

        assertTrue(tracker.dropRelay(relayA).isEmpty())
        assertTrue("still waiting on the other relay", tracker.isActive("q1"))
        assertTrue(tracker.noteEndOfStoredEvents("q1", relayB))
    }

    /** One disconnect settles every query that relay owed, and only those. */
    @Test
    fun `a disconnecting relay is dropped from every query it owed`() {
        val tracker = NostrQueryTracker()
        tracker.issue("q1", listOf(relayA), nowMs = 0)
        tracker.issue("q2", listOf(relayA), nowMs = 0)
        tracker.issue("q3", listOf(relayB), nowMs = 0)

        assertEquals(setOf("q1", "q2"), tracker.dropRelay(relayA).toSet())
        assertTrue("a query that never asked this relay is untouched", tracker.isActive("q3"))
    }

    /**
     * A relay is free never to send EOSE, so a query with no deadline holds its
     * subscription for the life of the connection while its caller waits on the
     * engine's much later sweep.
     */
    @Test
    fun `a silent relay expires the query at the deadline`() {
        val tracker = NostrQueryTracker()
        tracker.issue("q1", listOf(relayA), nowMs = 1_000)

        assertTrue(
            "not stale before the deadline",
            tracker.staleQueries(1_000 + NostrQueryTracker.COMPLETION_TIMEOUT_MS).isEmpty()
        )
        assertEquals(
            listOf("q1"),
            tracker.staleQueries(1_000 + NostrQueryTracker.COMPLETION_TIMEOUT_MS + 1)
        )
    }

    /**
     * Expiry is non-destructive: the caller still has to finish the query, which
     * is what sends CLOSE to the relays that never answered.
     */
    @Test
    fun `expiry reports without removing so finishing stays one path`() {
        val tracker = NostrQueryTracker()
        tracker.issue("q1", listOf(relayA, relayB), nowMs = 0)
        tracker.noteEndOfStoredEvents("q1", relayA)

        val stale = tracker.staleQueries(NostrQueryTracker.COMPLETION_TIMEOUT_MS + 1)
        assertEquals(listOf("q1"), stale)
        assertTrue("still in flight until finished", tracker.isActive("q1"))

        assertEquals(
            "only the relay that never answered is still owed a CLOSE",
            setOf(relayB),
            tracker.finish("q1")
        )
        assertFalse(tracker.isActive("q1"))
    }

    /** Finishing an unknown or already-finished query is a no-op. */
    @Test
    fun `finishing a query twice releases it once`() {
        val tracker = NostrQueryTracker()
        tracker.issue("q1", listOf(relayA), nowMs = 0)

        assertEquals(setOf(relayA), tracker.finish("q1"))
        assertNull("a second finish must not release it again", tracker.finish("q1"))
        assertNull(tracker.finish("never-issued"))
    }

    /**
     * A duplicate or stray EOSE must not complete a query. Reporting completion
     * twice would hand the transport the same query id twice, and the second
     * release can land after a later query reused the entry.
     */
    @Test
    fun `a duplicate or unknown end of stored events completes nothing`() {
        val tracker = NostrQueryTracker()
        tracker.issue("q1", listOf(relayA, relayB), nowMs = 0)

        assertFalse(tracker.noteEndOfStoredEvents("q1", relayA))
        assertFalse("the same relay answering twice adds nothing", tracker.noteEndOfStoredEvents("q1", relayA))
        assertFalse("a relay never asked adds nothing", tracker.noteEndOfStoredEvents("q1", relayC))
        assertTrue("still owed by the relay that has not answered", tracker.isActive("q1"))

        assertTrue(tracker.noteEndOfStoredEvents("q1", relayB))
        assertFalse(
            "an EOSE for a finished query completes nothing",
            tracker.noteEndOfStoredEvents("q1", relayA)
        )
    }

    /**
     * When the relays are gone entirely nothing will ever answer, so every
     * query is released rather than pinning subscription state.
     */
    @Test
    fun `clear releases every query`() {
        val tracker = NostrQueryTracker()
        tracker.issue("q1", listOf(relayA), nowMs = 0)
        tracker.issue("q2", listOf(relayB), nowMs = 0)

        assertEquals(setOf("q1", "q2"), tracker.clear().toSet())
        assertFalse(tracker.isActive("q1"))
        assertFalse(tracker.isActive("q2"))
        assertTrue("a second clear has nothing to release", tracker.clear().isEmpty())
    }

    /**
     * A query is recorded against the relays its REQ actually went to, so a
     * relay that connects later is never waited on for an answer it was never
     * asked for.
     */
    @Test
    fun `a relay that was never asked cannot hold a query open`() {
        val tracker = NostrQueryTracker()
        tracker.issue("q1", listOf(relayA), nowMs = 0)

        assertTrue(
            "a relay outside the query completes nothing by disconnecting",
            tracker.dropRelay(relayC).isEmpty()
        )
        assertTrue(tracker.noteEndOfStoredEvents("q1", relayA))
    }

    /** Re-issuing under the same id restarts the query rather than merging. */
    @Test
    fun `re-issuing a query id replaces its progress`() {
        val tracker = NostrQueryTracker()
        tracker.issue("q1", listOf(relayA), nowMs = 0)
        tracker.issue("q1", listOf(relayB), nowMs = 0)

        assertFalse(
            "the replaced query's relay is no longer owed",
            tracker.noteEndOfStoredEvents("q1", relayA)
        )
        assertTrue(tracker.noteEndOfStoredEvents("q1", relayB))
    }
}
