package com.offlineprotocol

import org.junit.Assert.assertFalse
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
}
