package com.offlineprotocol

import org.json.JSONObject
import org.junit.Assert.assertEquals
import org.junit.Assert.assertFalse
import org.junit.Assert.assertNull
import org.junit.Assert.assertTrue
import org.junit.Test

class SupersededLatchPolicyTest {

    @Test
    fun close4000LatchesWhenSocketIsCurrent() {
        val policy = SupersededLatchPolicy()
        // The Android close funnel's identity guard already dropped stale
        // sockets, so hasNewerSuccessor is always false there.
        assertTrue(policy.shouldMark(closeCode = 4000, hasNewerSuccessor = false))
    }

    @Test
    fun nonSupersedeCloseDoesNotLatch() {
        val policy = SupersededLatchPolicy()
        assertFalse(policy.shouldMark(closeCode = 1000, hasNewerSuccessor = false))
        assertFalse(policy.shouldMark(closeCode = -1, hasNewerSuccessor = false))
        assertFalse(policy.shouldMark(closeCode = null, hasNewerSuccessor = false))
    }

    @Test
    fun newerSuccessorSocketIsNeverLatchedByAStale4000() {
        // The cd9fa39 regression: old socket displaced → app re-enabled via
        // start() → new socket B up → a LATE 4000 for the bygone generation
        // must not re-latch and stop B.
        val policy = SupersededLatchPolicy()
        assertFalse(policy.shouldMark(closeCode = 4000, hasNewerSuccessor = true))
    }

    @Test
    fun successorGuardWinsEvenWhenAlreadyLatched() {
        val policy = SupersededLatchPolicy()
        policy.mark()
        // A stale latch bit must still never stop a live successor socket.
        assertFalse(policy.shouldMark(closeCode = 1000, hasNewerSuccessor = true))
    }

    @Test
    fun onceLatchedAnyCloseKeepsLatching() {
        // A non-4000 close arriving after a SessionSuperseded notice already
        // latched (on the live socket) must still stop, not reconnect.
        val policy = SupersededLatchPolicy()
        policy.mark()
        assertTrue(policy.shouldMark(closeCode = 1000, hasNewerSuccessor = false))
        assertTrue(policy.shouldMark(closeCode = null, hasNewerSuccessor = false))
    }

    @Test
    fun markIsIdempotentAndReportsOnlyTheFirstTransition() {
        // The relay emits a notice AND close 4000 (each fanning into several
        // terminal signals); the one-shot event must fire exactly once.
        val policy = SupersededLatchPolicy()
        assertTrue(policy.mark())   // false -> true: fire the event
        assertFalse(policy.mark())  // already latched: no re-fire
        assertFalse(policy.mark())
        assertTrue(policy.isSuperseded)
    }

    @Test
    fun startClearsTheLatchAndReArmsMark() {
        val policy = SupersededLatchPolicy()
        policy.mark()
        assertTrue(policy.isSuperseded)

        // A fresh start() clears it; a subsequent displacement fires again.
        policy.clear()
        assertFalse(policy.isSuperseded)
        assertFalse(policy.shouldMark(closeCode = 1000, hasNewerSuccessor = false))
        assertTrue(policy.mark())
    }

    @Test
    fun closeCodeConstantMatchesRelayContract() {
        assertTrue(SupersededLatchPolicy.SUPERSEDED_CLOSE_CODE == 4000)
    }

    // --- Event tag, payload and restatement ---------------------------------

    @Test
    fun eventTypeMatchesTheTagAppsMatchOn() {
        // Pinned literally, and identically on iOS. Apps switch on this string
        // (src/types.ts InternetSessionSupersededEvent) and it is the sticky
        // buffer's collapsing key; a drift is an event nobody receives.
        assertEquals("internet_session_superseded", SupersededLatchPolicy.EVENT_TYPE)
    }

    @Test
    fun eventJsonCarriesTypeAndReason() {
        val parsed = JSONObject(SupersededLatchPolicy.eventJson("connected elsewhere"))
        assertEquals("internet_session_superseded", parsed.getString("type"))
        assertEquals("connected elsewhere", parsed.getString("reason"))
    }

    @Test
    fun eventJsonOmitsReasonWhenAbsent() {
        // Omitted, not null: the shape both bridges have emitted since 0.16.2.
        val parsed = JSONObject(SupersededLatchPolicy.eventJson(null))
        assertEquals("internet_session_superseded", parsed.getString("type"))
        assertFalse(parsed.has("reason"))
    }

    @Test
    fun eventJsonEscapesRelaySuppliedReason() {
        // The reason is relay-supplied and reaches JS as a JSON string inside
        // an event envelope; this pins that it is built by a serializer.
        val hostile = "he said \"hi\"\n\\ tab\there"
        val parsed = JSONObject(SupersededLatchPolicy.eventJson(hostile))
        assertEquals(hostile, parsed.getString("reason"))
    }

    @Test
    fun restatementIsNullUntilSuperseded() {
        assertNull(SupersededLatchPolicy().restatementEventJson())
    }

    @Test
    fun restatementReportsTheLatchedReason() {
        val policy = SupersededLatchPolicy()
        policy.mark("newer session took the slot")

        val parsed = JSONObject(policy.restatementEventJson()!!)
        assertEquals("internet_session_superseded", parsed.getString("type"))
        assertEquals("newer session took the slot", parsed.getString("reason"))
    }

    @Test
    fun restatementRepeatsForAsLongAsTheTransportIsSuperseded() {
        // Deriving from state rather than buffering the emit: every foreground
        // while latched restates, so a drop heals on the next one instead of
        // being a single chance already spent.
        val policy = SupersededLatchPolicy()
        policy.mark("displaced")
        assertTrue(policy.restatementEventJson() != null)
        assertTrue(policy.restatementEventJson() != null)
        assertTrue(policy.restatementEventJson() != null)
    }

    @Test
    fun reasonIsFirstWinsAcrossTheSignalsOfOneDisplacement() {
        // The relay sends a SessionSuperseded notice (carrying the
        // explanation) and then closes 4000 (which does not). Last-wins would
        // overwrite the reason with nothing.
        val policy = SupersededLatchPolicy()
        assertTrue(policy.mark("session superseded by newer login"))
        assertFalse(policy.mark(null))
        assertFalse(policy.mark("a later, different story"))
        assertEquals("session superseded by newer login", policy.supersedeReason)
    }

    @Test
    fun clearDropsTheReasonWithTheLatch() {
        val policy = SupersededLatchPolicy()
        policy.mark("displaced")
        policy.clear()

        assertNull(policy.supersedeReason)
        assertNull(policy.restatementEventJson())

        policy.mark("displaced again")
        assertEquals("displaced again", policy.supersedeReason)
    }

    @Test
    fun reEnableStopsRestatementWithoutAnyDiscardBookkeeping() {
        // Both paths that clear the latch reach InternetManager.start() ->
        // clear(), which is why re-deriving needs no discard site: after
        // either, there is simply nothing to restate.
        val policy = SupersededLatchPolicy()
        policy.mark("displaced")
        assertTrue(policy.restatementEventJson() != null)

        policy.clear()

        assertNull(policy.restatementEventJson())
    }

    @Test
    fun markWithoutAReasonStillRestates() {
        // The close-4000 path latches with no relay explanation. The report is
        // still the point; only the reason is missing.
        val policy = SupersededLatchPolicy()
        policy.mark()
        assertEquals("""{"type":"internet_session_superseded"}""", policy.restatementEventJson())
    }
}
