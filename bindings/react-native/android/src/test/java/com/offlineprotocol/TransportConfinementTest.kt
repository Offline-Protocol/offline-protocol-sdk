package com.offlineprotocol

import android.os.HandlerThread
import android.os.Looper
import java.time.Duration
import java.util.concurrent.CountDownLatch
import java.util.concurrent.TimeUnit
import org.junit.After
import org.junit.Assert.assertEquals
import org.junit.Assert.assertFalse
import org.junit.Assert.assertNotEquals
import org.junit.Assert.assertSame
import org.junit.Assert.assertTrue
import org.junit.Assert.fail
import org.junit.Test
import org.junit.runner.RunWith
import org.robolectric.RobolectricTestRunner
import org.robolectric.Shadows.shadowOf
import org.robolectric.annotation.Config
import org.robolectric.shadows.ShadowLog

/**
 * Pins the properties the transport managers took from the main looper and now
 * take from here: ordered one-at-a-time execution, and a wait that cannot turn
 * a contended protocol mutex into an ANR.
 *
 * The timeout asymmetry is the part worth pinning. A flat bound for every
 * caller — what Nostr and Reticulum used to have — fails a background `stop()`
 * exactly when the mutex is most contended, leaving a transport half-down; no
 * bound at all — what Internet used to have — parks the main thread behind that
 * same mutex, which is OFF-2123 itself. Only main is bounded, and the test
 * drives both halves against a thread that is deliberately blocked.
 *
 * Runs on real (unpaused) loopers: the contract under test is about threads
 * actually racing, which a paused Robolectric looper cannot express.
 */
@RunWith(RobolectricTestRunner::class)
@Config(sdk = [34])
class TransportConfinementTest {

    private val threads = mutableListOf<HandlerThread>()

    /**
     * Loopers taken from [TransportConfinement.shared], which are process-wide
     * and never quit — that being the contract the reuse test verifies, so the
     * test cannot avoid starting one. It can avoid leaving it running for the
     * rest of the suite, which is all this list is for. Safe because the name
     * used there belongs to this test alone, so nothing looks the stale entry
     * up afterwards.
     */
    private val sharedLoopers = mutableListOf<Looper>()

    @After
    fun tearDown() {
        threads.forEach { it.quitSafely() }
        threads.clear()
        sharedLoopers.forEach { it.quitSafely() }
        sharedLoopers.clear()
    }

    private fun confinement(
        name: String = "test-confinement",
        mainSyncTimeoutMs: Long = 120L,
    ): TransportConfinement {
        val thread = HandlerThread(name).apply { start() }
        threads += thread
        return TransportConfinement(name, thread.looper, mainSyncTimeoutMs)
    }

    /** Occupies the confinement thread until the returned latch is counted down. */
    private fun block(confinement: TransportConfinement): CountDownLatch {
        val release = CountDownLatch(1)
        val blocking = CountDownLatch(1)
        confinement.post {
            blocking.countDown()
            release.await()
        }
        assertTrue("blocker never started", blocking.await(2, TimeUnit.SECONDS))
        return release
    }

    @Test
    fun `posted actions run on the confinement thread in order`() {
        val confinement = confinement()
        val seen = mutableListOf<Int>()
        val threadNames = mutableListOf<String>()
        val done = CountDownLatch(3)

        repeat(3) { i ->
            confinement.post {
                seen += i
                threadNames += Thread.currentThread().name
                done.countDown()
            }
        }

        assertTrue(done.await(2, TimeUnit.SECONDS))
        assertEquals(listOf(0, 1, 2), seen)
        assertTrue(threadNames.all { it == "test-confinement" })
    }

    @Test
    fun `runSync on the confinement thread runs inline without re-posting`() {
        val confinement = confinement()

        // Nesting is the case that would deadlock if the fast path were missing:
        // the outer runSync owns the thread the inner one would wait for.
        val result = confinement.runSync {
            assertTrue(confinement.isCurrent())
            confinement.runSync { "inner" }
        }

        assertEquals("inner", result)
    }

    @Test
    fun `runSync returns the value and rethrows the failure of its action`() {
        val confinement = confinement()

        assertEquals(7, confinement.runSync { 7 })

        val boom = IllegalStateException("boom")
        try {
            confinement.runSync { throw boom }
            fail("expected the action's exception to propagate")
        } catch (e: IllegalStateException) {
            assertSame(boom, e)
        }
    }

    @Test
    fun `a main-thread caller gives up rather than parking behind a busy transport`() {
        val confinement = confinement(mainSyncTimeoutMs = 120L)
        val release = block(confinement)

        try {
            assertSame(Looper.getMainLooper(), Looper.myLooper())
            val startedAt = System.nanoTime()
            try {
                confinement.runSync { "never observed" }
                fail("expected MainThreadSyncTimeout")
            } catch (e: TransportConfinement.MainThreadSyncTimeout) {
                val waitedMs = (System.nanoTime() - startedAt) / 1_000_000
                assertTrue("gave up too early: ${waitedMs}ms", waitedMs >= 100)
                assertTrue("waited far past the bound: ${waitedMs}ms", waitedMs < 2_000)
            }
        } finally {
            release.countDown()
        }
    }

    @Test
    fun `the timed-out action still runs, it is only no longer awaited`() {
        val confinement = confinement(mainSyncTimeoutMs = 80L)
        val release = block(confinement)
        val ran = CountDownLatch(1)

        try {
            confinement.runSync { ran.countDown() }
            fail("expected MainThreadSyncTimeout")
        } catch (e: TransportConfinement.MainThreadSyncTimeout) {
            // Expected: main stopped waiting.
        }

        assertEquals("action must not have run while the thread was blocked", 1L, ran.count)
        release.countDown()
        assertTrue("abandoned action never completed", ran.await(2, TimeUnit.SECONDS))
    }

    /**
     * An abandoned action still runs, and if it throws there is no longer
     * anyone positioned to see that: nothing reads the outcome and the caller
     * already has its [TransportConfinement.MainThreadSyncTimeout]. A `start()`
     * refused for a real reason would otherwise be indistinguishable from a
     * slow one, which is the diagnosis this thread exists to make possible.
     */
    @Test
    fun `a failure the main caller stopped waiting for is still reported`() {
        val confinement = confinement(mainSyncTimeoutMs = 80L)
        val release = block(confinement)
        ShadowLog.clear()

        try {
            confinement.runSync<Unit> { throw IllegalStateException("refused-after-timeout") }
            fail("expected MainThreadSyncTimeout")
        } catch (e: TransportConfinement.MainThreadSyncTimeout) {
            // Expected: main stopped waiting.
        }

        // The report is posted behind the action on this same thread, so a
        // marker behind both is enough to know it has run — no polling.
        val reported = CountDownLatch(1)
        confinement.post { reported.countDown() }
        release.countDown()
        assertTrue("the queue never drained", reported.await(2, TimeUnit.SECONDS))

        assertTrue(
            "the abandoned action's failure was dropped",
            ShadowLog.getLogs().any { it.throwable?.message == "refused-after-timeout" },
        )
    }

    @Test
    fun `a background caller waits as long as it takes`() {
        val confinement = confinement(mainSyncTimeoutMs = 80L)
        val release = block(confinement)
        val outcome = arrayOfNulls<Any>(1)
        val finished = CountDownLatch(1)

        val caller = Thread {
            outcome[0] = try {
                confinement.runSync { "delivered" }
            } catch (t: Throwable) {
                t
            }
            finished.countDown()
        }
        caller.start()

        // Well past the main-thread bound: a background caller must not be
        // subject to it. This is the `stop()` that has to actually finish.
        assertFalse(
            "background caller was cut off at the main bound",
            finished.await(400, TimeUnit.MILLISECONDS),
        )

        release.countDown()
        assertTrue(finished.await(2, TimeUnit.SECONDS))
        assertEquals("delivered", outcome[0])
    }

    /**
     * A quit looper accepts nothing, and the background wait has no bound to
     * rescue it: without the post check the caller — the React Native
     * native-modules thread, on the stop path — parks for the life of the
     * process. Threads from [TransportConfinement.shared] never quit, so this
     * is unreachable through the shipped entry point; it is reachable through
     * the constructor, which is what this drives.
     */
    @Test
    fun `runSync fails loudly rather than parking forever on a quit looper`() {
        val thread = HandlerThread("test-quit").apply { start() }
        val confinement = TransportConfinement("test-quit", thread.looper)
        thread.quitSafely()
        thread.join(2_000)

        try {
            confinement.runSync { "never reached" }
            fail("expected a quit looper to be reported, not waited on")
        } catch (e: IllegalStateException) {
            assertTrue(e.message!!.contains("test-quit"))
        }
    }

    /**
     * The hazard `ReticulumManager`'s poll lifecycle posts around, and the
     * reason it hops onto the IO thread instead of reaching across from the
     * lifecycle one.
     *
     * A self-reposting runnable is not in the queue while it is running, so a
     * `removeCallbacks` issued from another thread finds nothing to remove and
     * the repost that follows it survives — a `pause()` that leaves the
     * transport polling for the whole background stay. Posting the removal
     * onto the runnable's own thread queues it behind that repost, which is
     * what actually stops the loop.
     *
     * Both halves run against a runnable held mid-flight on purpose, so the
     * window is the test's rather than the scheduler's. The repost is delayed
     * because that is what the real one is (a 5s poll interval), and the delay
     * is load-bearing: a cancel already sitting in the queue is due *now* and
     * therefore runs first, which is the whole reason posting it works. It
     * also means the clock has to be driven by hand — Robolectric freezes
     * `SystemClock`, and under a frozen clock the repost never comes due and
     * "did not run again" would pass without proving anything.
     */
    @Test
    fun `cancelling a self-reposting runnable requires posting to its own thread`() {
        val repostDelayMs = 50L

        for (postTheCancel in listOf(false, true)) {
            val io = confinement(name = "test-io-$postTheCancel")
            val entered = CountDownLatch(1)
            val release = CountDownLatch(1)
            val reposted = CountDownLatch(1)
            val ranAgain = CountDownLatch(1)
            var first = true

            val poll = object : Runnable {
                override fun run() {
                    if (!first) {
                        ranAgain.countDown()
                        return
                    }
                    first = false
                    entered.countDown()
                    release.await()
                    // The repost is what a cancel has to beat.
                    io.handler.postDelayed(this, repostDelayMs)
                    reposted.countDown()
                }
            }

            io.handler.post(poll)
            assertTrue("poll never started", entered.await(2, TimeUnit.SECONDS))

            if (postTheCancel) {
                io.post { io.handler.removeCallbacks(poll) }
            } else {
                io.handler.removeCallbacks(poll)
            }

            // Let the runnable finish and re-arm before advancing time, so the
            // repost is scheduled against the pre-advance clock rather than
            // racing past it.
            release.countDown()
            assertTrue("poll never re-armed", reposted.await(2, TimeUnit.SECONDS))
            shadowOf(io.looper).idleFor(Duration.ofMillis(repostDelayMs * 4))

            if (postTheCancel) {
                assertEquals(
                    "a cancel posted to the runnable's own thread must land behind the " +
                        "repost and remove it",
                    1L,
                    ranAgain.count,
                )
            } else {
                assertEquals(
                    "a cross-thread cancel cannot stop a runnable that is mid-flight — if " +
                        "this ever stops holding, the posted form is no longer needed",
                    0L,
                    ranAgain.count,
                )
            }
        }
    }

    @Test
    fun `a shared confinement is reused by name so a rebuilt manager keeps one queue`() {
        val first = TransportConfinement.shared("offline-test-shared")
        val second = TransportConfinement.shared("offline-test-shared")
        sharedLoopers += first.looper

        assertSame(first, second)
        assertNotEquals(Looper.getMainLooper(), first.looper)
        assertEquals("delivered", second.runSync { "delivered" })
    }
}
