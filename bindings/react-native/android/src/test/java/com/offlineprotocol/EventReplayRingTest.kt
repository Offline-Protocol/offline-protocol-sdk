package com.offlineprotocol

import org.junit.Assert.assertEquals
import org.junit.Assert.assertFalse
import org.junit.Assert.assertTrue
import org.junit.Test
import java.util.concurrent.CountDownLatch
import kotlin.concurrent.thread

/**
 * Pins what the TypeScript layer's gap recovery relies on (#514): a number it
 * expected and never saw is served back with the exact event handed over
 * under it, or is absent because it aged out, never served wrong.
 */
class EventReplayRingTest {

    private fun ev(seq: Int) = """{"type":"message_received","message_id":"m$seq"}"""

    @Test
    fun aMissingNumberIsServedWithItsOwnEvent() {
        val ring = EventReplayRing()
        (0..4).forEach { ring.record(it, ev(it)) }

        val replayed = ring.replay(2, 2)

        assertEquals(listOf(EventReplayRing.Entry(2, ev(2))), replayed)
    }

    @Test
    fun aRangeComesBackInOrder() {
        val ring = EventReplayRing()
        (0..9).forEach { ring.record(it, ev(it)) }

        assertEquals(listOf(3, 4, 5, 6), ring.replay(3, 6).map { it.seq })
    }

    @Test
    fun pastTheCapacityTheOldestGoes() {
        val ring = EventReplayRing(capacity = 4)
        (0..9).forEach { ring.record(it, ev(it)) }

        assertEquals(4, ring.size)
        // 0..5 aged out; only the tail of the range is served, and the caller
        // reports the rest as a gap it could not recover.
        assertEquals(listOf(6, 7), ring.replay(4, 7).map { it.seq })
    }

    @Test
    fun aRangeEntirelyAgedOutReturnsNothing() {
        val ring = EventReplayRing(capacity = 2)
        (0..9).forEach { ring.record(it, ev(it)) }

        assertTrue(ring.replay(0, 5).isEmpty())
    }

    @Test
    fun anEmptyOrInvertedRangeReturnsNothing() {
        val ring = EventReplayRing()
        (0..3).forEach { ring.record(it, ev(it)) }

        assertTrue(ring.replay(3, 2).isEmpty())
        assertTrue(ring.replay(10, 12).isEmpty())
    }

    @Test
    fun clearForgetsAnEarlierSession() {
        val ring = EventReplayRing()
        (0..3).forEach { ring.record(it, ev(it)) }

        ring.clear()
        ring.record(0, """{"type":"diagnostic"}""")

        assertEquals(listOf(EventReplayRing.Entry(0, """{"type":"diagnostic"}""")), ring.replay(0, 3))
    }

    @Test
    fun concurrentRecordsKeepEveryNumberWithItsOwnPayload() {
        // The module records under its own lock, so numbers arrive in order;
        // this checks the ring itself never pairs a number with another
        // thread's payload when writers race.
        val ring = EventReplayRing(capacity = 10_000)
        val start = CountDownLatch(1)
        val writers = (0 until 4).map { w ->
            thread {
                start.await()
                for (i in 0 until 1_000) {
                    val seq = w * 1_000 + i
                    ring.record(seq, ev(seq))
                }
            }
        }
        start.countDown()
        writers.forEach { it.join() }

        val all = ring.replay(0, 3_999)
        assertEquals(4_000, all.size)
        all.forEach { assertEquals(ev(it.seq), it.eventJson) }
    }

    @Test
    fun aSignalReadingIsNeitherNumberedNorKept() {
        // One numbered per quarter second per peer would push the messages
        // this ring exists for out of it within seconds.
        assertFalse(EventReplayRing.isReplayed("""{"type":"peer_rssi","peer_id":"p","transport":"BLE","rssi":-50}"""))
        assertTrue(EventReplayRing.isReplayed("""{"type":"message_received","message_id":"m"}"""))
        assertTrue(EventReplayRing.isReplayed("""{"type":"neighbor_discovered","peer_id":"p","transport":"BLE","rssi":-50}"""))
        assertTrue("an event with no JSON keeps today's numbering", EventReplayRing.isReplayed(null))
    }
}
