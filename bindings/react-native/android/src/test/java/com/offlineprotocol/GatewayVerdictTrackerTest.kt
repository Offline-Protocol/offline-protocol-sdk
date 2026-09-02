package com.offlineprotocol

import java.util.concurrent.CountDownLatch
import java.util.concurrent.Executors
import java.util.concurrent.TimeUnit
import java.util.concurrent.atomic.AtomicInteger
import org.junit.Assert.assertEquals
import org.junit.Assert.assertFalse
import org.junit.Assert.assertTrue
import org.junit.Test

/**
 * Plain JUnit: this class touches no framework type, which is the whole reason
 * it is a class of its own rather than fields on the manager.
 */
class GatewayVerdictTrackerTest {

    @Test
    fun `a frame is tracked until it is settled`() {
        val tracker = GatewayVerdictTracker()
        assertEquals(0, tracker.count)

        assertTrue(tracker.begin("abc", 0))
        assertEquals(1, tracker.count)

        assertTrue(tracker.settle("abc"))
        assertEquals(0, tracker.count)
    }

    /**
     * The core re-queues an unconfirmed frame under the same id after its own
     * acknowledgement timeout, and a verdict can honestly take longer than that
     * over a radio backbone. Sending it again forwards the frame twice and,
     * when the second copy times out, fails an id the gateway already
     * confirmed.
     */
    @Test
    fun `an id already in flight is refused`() {
        val tracker = GatewayVerdictTracker()
        assertTrue(tracker.begin("abc", 0))

        assertFalse("the same id must not be sent twice", tracker.begin("abc", 30_000))
        assertEquals("and the first attempt still owns the slot", 1, tracker.count)
    }

    /**
     * A duplicate verdict must not report a second outcome for a frame the core
     * has already moved past.
     */
    @Test
    fun `settling twice reports only once`() {
        val tracker = GatewayVerdictTracker()
        tracker.begin("abc", 0)

        assertTrue(tracker.settle("abc"))
        assertFalse(tracker.settle("abc"))
    }

    @Test
    fun `settling something unknown is ignored`() {
        assertFalse(GatewayVerdictTracker().settle("never-sent"))
    }

    /**
     * A gateway that answers nothing is indistinguishable from a wedged socket,
     * and the core cannot retry a frame nobody failed. The sweep is what turns
     * silence back into a retry.
     */
    @Test
    fun `frames older than the timeout are expired`() {
        val tracker = GatewayVerdictTracker()
        tracker.begin("old", 0)
        tracker.begin("recent", 55_000)

        val expired = tracker.expired(61_000, 60_000)

        assertEquals(listOf("old"), expired)
        assertEquals("the recent frame is still outstanding", 1, tracker.count)
    }

    @Test
    fun `expiry is exclusive of the boundary`() {
        val tracker = GatewayVerdictTracker()
        tracker.begin("edge", 0)

        assertTrue(tracker.expired(60_000, 60_000).isEmpty())
        assertEquals(listOf("edge"), tracker.expired(60_001, 60_000))
    }

    /**
     * A connection going away owes an outcome on everything it was carrying. A
     * frame nobody reports on waits out the core's own 120s expiry.
     */
    @Test
    fun `draining hands back everything outstanding`() {
        val tracker = GatewayVerdictTracker()
        tracker.begin("a", 0)
        tracker.begin("b", 0)

        assertEquals(listOf("a", "b"), tracker.drainAll().sorted())
        assertEquals(0, tracker.count)
        assertTrue(tracker.drainAll().isEmpty())
    }

    /**
     * The manager touches this from its IO looper and from the receive thread,
     * so `begin` has to be a single atomic decision: two threads racing the
     * same id must not both be told to send it.
     */
    @Test
    fun `concurrent begins admit exactly one`() {
        val tracker = GatewayVerdictTracker()
        val admitted = AtomicInteger(0)
        val pool = Executors.newFixedThreadPool(8)
        val done = CountDownLatch(64)

        repeat(64) {
            pool.submit {
                if (tracker.begin("contended", 0)) admitted.incrementAndGet()
                done.countDown()
            }
        }
        assertTrue(done.await(10, TimeUnit.SECONDS))
        pool.shutdown()

        assertEquals(1, admitted.get())
        assertEquals(1, tracker.count)
    }

    /**
     * The three ways an id leaves the tracker race each other in the manager:
     * a verdict on the receive thread, the sweep on the IO looper, and a
     * teardown on the transport thread. Exactly one of them may own the
     * outcome, or the core is told twice about one frame.
     */
    @Test
    fun `an id leaves through exactly one door`() {
        val tracker = GatewayVerdictTracker()
        tracker.begin("contended", 0)
        val outcomes = AtomicInteger(0)
        val pool = Executors.newFixedThreadPool(8)
        val done = CountDownLatch(48)

        repeat(16) {
            pool.submit {
                if (tracker.settle("contended")) outcomes.incrementAndGet()
                done.countDown()
            }
            pool.submit {
                if (tracker.expired(120_000, 60_000).isNotEmpty()) outcomes.incrementAndGet()
                done.countDown()
            }
            pool.submit {
                if (tracker.drainAll().isNotEmpty()) outcomes.incrementAndGet()
                done.countDown()
            }
        }
        assertTrue(done.await(10, TimeUnit.SECONDS))
        pool.shutdown()

        assertEquals(1, outcomes.get())
        assertEquals(0, tracker.count)
    }
}
