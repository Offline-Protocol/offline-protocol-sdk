package com.offlineprotocol.ble

import android.os.Handler
import android.os.Looper
import java.util.concurrent.TimeUnit
import org.junit.Assert.assertEquals
import org.junit.Assert.assertFalse
import org.junit.Assert.assertTrue
import org.junit.Test
import org.junit.runner.RunWith
import org.robolectric.RobolectricTestRunner
import org.robolectric.Shadows.shadowOf
import org.robolectric.annotation.Config
import org.robolectric.annotation.LooperMode

@RunWith(RobolectricTestRunner::class)
@Config(sdk = [34])
@LooperMode(LooperMode.Mode.PAUSED)
class BackpressureRetryPolicyTest {
    private val looper = shadowOf(Looper.getMainLooper())
    private val handler = Handler(Looper.getMainLooper())
    private var elapsedMs = 0L

    @Test
    fun `successive stalls climb and cap the backoff ladder`() {
        var runs = 0
        val policy = policy { runs++ }

        // 50ms floor.
        assertTrue(policy.schedule())
        advanceTo(49L)
        assertEquals(0, runs)
        advanceTo(50L)
        assertEquals(1, runs)

        // Doubling: 100, 200, 400.
        assertTrue(policy.schedule())
        advanceTo(149L)
        assertEquals(1, runs)
        advanceTo(150L)
        assertEquals(2, runs)

        assertTrue(policy.schedule())
        advanceTo(350L)
        assertEquals(3, runs)

        assertTrue(policy.schedule())
        advanceTo(750L)
        assertEquals(4, runs)
    }

    @Test
    fun `ladder saturates at the ceiling delay rather than growing without bound`() {
        var runs = 0
        // 50 -> 100 -> 200, then pinned at 200.
        val policy = policy(maxDelayMs = 200L) { runs++ }

        policy.schedule()
        advanceTo(50L)
        policy.schedule()
        advanceTo(150L)
        policy.schedule()
        advanceTo(350L)
        assertEquals(3, runs)

        // Two more rungs must both be 200ms, not 400 and 800.
        policy.schedule()
        advanceTo(550L)
        assertEquals(4, runs)

        policy.schedule()
        advanceTo(750L)
        assertEquals(5, runs)
    }

    @Test
    fun `the attempt ceiling stops re-arming so the polling floor takes over`() {
        var runs = 0
        val policy = policy(maxConsecutiveAttempts = 3) { runs++ }

        // Three stalls, each retry firing before the next one is armed — the
        // production shape, where the re-drain runs and then stalls again.
        assertTrue(policy.schedule())
        advanceTo(50L)
        assertTrue(policy.schedule())
        advanceTo(150L)
        assertTrue(policy.schedule())
        advanceTo(350L)
        assertEquals(3, runs)

        // Fourth stall is refused: the drain must fall through to the 2s poller
        // instead of holding the BLE thread at 20Hz forever.
        assertFalse(policy.schedule())
        assertEquals(3, policy.attempts)

        // And refusing must not leave a stray post behind.
        advanceTo(60_000L)
        assertEquals(3, runs)
    }

    @Test
    fun `progress re-arms the fast ladder from the floor`() {
        var runs = 0
        val policy = policy { runs++ }

        policy.schedule()
        advanceTo(50L)
        policy.schedule()
        advanceTo(150L)
        assertEquals(2, runs)

        // A fragment actually went out.
        policy.reset()

        // Next stall retries at the 50ms floor again, not the 400ms rung.
        assertTrue(policy.schedule())
        advanceTo(199L)
        assertEquals(2, runs)
        advanceTo(200L)
        assertEquals(3, runs)
    }

    @Test
    fun `progress clears an exhausted ceiling`() {
        var runs = 0
        val policy = policy(maxConsecutiveAttempts = 2) { runs++ }

        policy.schedule()
        policy.schedule()
        assertFalse(policy.schedule())

        // A peer that recovers must not be stuck on the polling floor forever.
        policy.reset()
        assertTrue(policy.schedule())
        assertEquals(1, policy.attempts)
    }

    @Test
    fun `scheduling again replaces the pending retry instead of stacking`() {
        var runs = 0
        val policy = policy { runs++ }

        policy.schedule()
        policy.schedule()
        policy.schedule()

        // Three schedules, one pending task: the ladder advanced to the 200ms
        // rung and only that last post survives.
        advanceTo(200L)
        assertEquals(1, runs)
        advanceTo(60_000L)
        assertEquals(1, runs)
    }

    @Test
    fun `cancel drops the pending retry and returns the ladder to its floor`() {
        var runs = 0
        val policy = policy { runs++ }

        policy.schedule()
        policy.schedule()
        policy.cancel()

        advanceTo(10_000L)
        assertEquals(0, runs)
        assertEquals(0, policy.attempts)

        // Back at the floor.
        policy.schedule()
        advanceTo(10_050L)
        assertEquals(1, runs)
    }

    private fun policy(
        maxDelayMs: Long = 2_000L,
        maxConsecutiveAttempts: Int = 12,
        block: () -> Unit,
    ) = BackpressureRetryPolicy(
        handler = handler,
        task = Runnable(block),
        minDelayMs = 50L,
        maxDelayMs = maxDelayMs,
        maxConsecutiveAttempts = maxConsecutiveAttempts,
    )

    private fun advanceTo(targetMs: Long) {
        looper.idleFor(targetMs - elapsedMs, TimeUnit.MILLISECONDS)
        elapsedMs = targetMs
    }
}
