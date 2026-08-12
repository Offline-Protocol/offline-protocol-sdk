package com.offlineprotocol

import android.os.HandlerThread
import android.os.Looper
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
import org.robolectric.annotation.Config

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

    @After
    fun tearDown() {
        threads.forEach { it.quitSafely() }
        threads.clear()
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

    @Test
    fun `assertConfined accepts the confinement thread and rejects every other`() {
        val confinement = confinement()

        confinement.runSync { confinement.assertConfined("state mutation") }

        try {
            confinement.assertConfined("state mutation")
            fail("expected the off-thread caller to be rejected")
        } catch (e: IllegalStateException) {
            assertTrue(e.message!!.contains("state mutation"))
        }
    }

    @Test
    fun `a shared confinement is reused by name so a rebuilt manager keeps one queue`() {
        val first = TransportConfinement.shared("offline-test-shared")
        val second = TransportConfinement.shared("offline-test-shared")

        assertSame(first, second)
        assertNotEquals(Looper.getMainLooper(), first.looper)
        assertEquals("delivered", second.runSync { "delivered" })
    }
}
