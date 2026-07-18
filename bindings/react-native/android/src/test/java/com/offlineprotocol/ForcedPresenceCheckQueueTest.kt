package com.offlineprotocol

import org.junit.Assert.assertEquals
import org.junit.Assert.assertFalse
import org.junit.Assert.assertTrue
import org.junit.Test

/**
 * Mirrors ios/tests/ForcedPresenceCheckQueueTests.swift — keep in sync.
 *
 * The queue only ever sees checks that could not be sent right now; a
 * sendable check completes in the manager without touching it. These
 * tests pin the decision policy: park before the deadline, fail fast on a
 * stopping/stopped transport, expire at/past the deadline, reject at
 * capacity, and resolve every callback exactly once.
 */
class ForcedPresenceCheckQueueTest {

    private class Recorder {
        var invocations = 0
        var lastResult: Boolean? = null
        val callback: (Boolean) -> Unit = { result ->
            invocations += 1
            lastResult = result
        }
    }

    private fun entry(recorder: Recorder, deadlineMs: Long, userId: String = "peer") =
        ForcedPresenceCheckQueue.Entry(userId, deadlineMs, recorder.callback)

    @Test
    fun parksBeforeTheDeadlineWithoutResolving() {
        val queue = ForcedPresenceCheckQueue()
        val recorder = Recorder()
        assertTrue(queue.parkOrExpire(entry(recorder, deadlineMs = 8_000), transportStopped = false, nowMs = 1_000))
        assertEquals(0, recorder.invocations)
        assertFalse(queue.isEmpty)
    }

    @Test
    fun failsFastOnAStoppedTransport() {
        val queue = ForcedPresenceCheckQueue()
        val recorder = Recorder()
        // Even far from the deadline: no reconnect is coming.
        assertFalse(queue.parkOrExpire(entry(recorder, deadlineMs = 8_000), transportStopped = true, nowMs = 1_000))
        assertEquals(1, recorder.invocations)
        assertEquals(false, recorder.lastResult)
        assertTrue(queue.isEmpty)
    }

    @Test
    fun expiresExactlyAtTheDeadline() {
        val queue = ForcedPresenceCheckQueue()
        val recorder = Recorder()
        assertFalse(queue.parkOrExpire(entry(recorder, deadlineMs = 8_000), transportStopped = false, nowMs = 8_000))
        assertEquals(1, recorder.invocations)
        assertEquals(false, recorder.lastResult)
        assertTrue(queue.isEmpty)
    }

    @Test
    fun expiresPastTheDeadline() {
        val queue = ForcedPresenceCheckQueue()
        val recorder = Recorder()
        assertFalse(queue.parkOrExpire(entry(recorder, deadlineMs = 8_000), transportStopped = false, nowMs = 9_500))
        assertEquals(1, recorder.invocations)
        assertEquals(false, recorder.lastResult)
    }

    @Test
    fun rejectsNewEntriesAtCapacityWithoutEvictingParkedOnes() {
        val queue = ForcedPresenceCheckQueue(capacity = 2)
        val first = Recorder()
        val second = Recorder()
        val third = Recorder()
        assertTrue(queue.parkOrExpire(entry(first, 8_000, "a"), transportStopped = false, nowMs = 0))
        assertTrue(queue.parkOrExpire(entry(second, 8_000, "b"), transportStopped = false, nowMs = 0))
        assertFalse(queue.parkOrExpire(entry(third, 8_000, "c"), transportStopped = false, nowMs = 0))
        assertEquals(1, third.invocations)
        assertEquals(false, third.lastResult)
        // The parked entries survive, in arrival order.
        assertEquals(listOf("a", "b"), queue.takeAll().map { it.userId })
        assertEquals(0, first.invocations)
        assertEquals(0, second.invocations)
    }

    @Test
    fun takeAllEmptiesTheQueueAndFreesCapacity() {
        val queue = ForcedPresenceCheckQueue(capacity = 1)
        val first = Recorder()
        val second = Recorder()
        assertTrue(queue.parkOrExpire(entry(first, 8_000, "a"), transportStopped = false, nowMs = 0))
        assertEquals(1, queue.takeAll().size)
        assertTrue(queue.isEmpty)
        // A serviced (taken) entry no longer occupies its slot.
        assertTrue(queue.parkOrExpire(entry(second, 8_000, "b"), transportStopped = false, nowMs = 0))
    }

    @Test
    fun drainAllResolvesEveryEntryFalseExactlyOnce() {
        val queue = ForcedPresenceCheckQueue()
        val recorders = List(3) { Recorder() }
        recorders.forEachIndexed { i, r ->
            assertTrue(queue.parkOrExpire(entry(r, 8_000, "peer$i"), transportStopped = false, nowMs = 0))
        }
        queue.drainAll()
        assertTrue(queue.isEmpty)
        for (r in recorders) {
            assertEquals(1, r.invocations)
            assertEquals(false, r.lastResult)
        }
        // Idempotent on an empty queue.
        queue.drainAll()
        for (r in recorders) {
            assertEquals(1, r.invocations)
        }
    }

    @Test
    fun callbackFiresExactlyOnceAcrossParkTakeReparkAndDrain() {
        val queue = ForcedPresenceCheckQueue()
        val recorder = Recorder()
        val check = entry(recorder, deadlineMs = 8_000)
        // Park, get serviced, still unsendable, re-park (the retry-tick
        // lifecycle), then the transport stops.
        assertTrue(queue.parkOrExpire(check, transportStopped = false, nowMs = 0))
        val taken = queue.takeAll()
        assertEquals(1, taken.size)
        assertEquals(0, recorder.invocations)
        assertTrue(queue.parkOrExpire(taken[0], transportStopped = false, nowMs = 4_000))
        queue.drainAll()
        assertEquals(1, recorder.invocations)
        assertEquals(false, recorder.lastResult)
    }
}
