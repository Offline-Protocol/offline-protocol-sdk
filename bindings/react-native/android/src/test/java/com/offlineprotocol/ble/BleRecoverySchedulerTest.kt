package com.offlineprotocol.ble

import android.os.Handler
import android.os.Looper
import java.util.concurrent.TimeUnit
import org.junit.Assert.assertEquals
import org.junit.Test
import org.junit.runner.RunWith
import org.robolectric.RobolectricTestRunner
import org.robolectric.Shadows.shadowOf
import org.robolectric.annotation.Config
import org.robolectric.annotation.LooperMode

@RunWith(RobolectricTestRunner::class)
@Config(sdk = [34])
@LooperMode(LooperMode.Mode.PAUSED)
class BleRecoverySchedulerTest {
    private val looper = shadowOf(Looper.getMainLooper())
    private val handler = Handler(Looper.getMainLooper())
    private var elapsedMs = 0L

    @Test
    fun `successive recovery attempts climb and cap the backoff ladder`() {
        var runs = 0
        val scheduler = scheduler { runs++ }

        scheduler.schedule()
        advanceTo(9_999L)
        assertEquals(0, runs)
        advanceTo(10_000L)
        assertEquals(1, runs)

        scheduler.schedule()
        advanceTo(29_999L)
        assertEquals(1, runs)
        advanceTo(30_000L)
        assertEquals(2, runs)

        scheduler.schedule()
        advanceTo(59_999L)
        assertEquals(2, runs)
        advanceTo(60_000L)
        assertEquals(3, runs)

        scheduler.schedule()
        advanceTo(90_000L)
        assertEquals(4, runs)
    }

    @Test
    fun `transient teardown preserves the next retry rung`() {
        var runs = 0
        val scheduler = scheduler { runs++ }

        scheduler.schedule()
        scheduler.cancel(resetBackoff = false)
        scheduler.schedule()

        advanceTo(19_999L)
        assertEquals(0, runs)
        advanceTo(20_000L)
        assertEquals(1, runs)
    }

    @Test
    fun `deliberate cancellation resets the next retry to the floor`() {
        var runs = 0
        val scheduler = scheduler { runs++ }

        scheduler.schedule()
        scheduler.cancel()
        scheduler.schedule()

        advanceTo(9_999L)
        assertEquals(0, runs)
        advanceTo(10_000L)
        assertEquals(1, runs)
    }

    @Test
    fun `scan resume re-arms a cancelled adapter recovery episode`() {
        var runs = 0
        val scheduler = scheduler { runs++ }

        // Adapter-off detection arms recovery, then app backgrounding cancels
        // it. A successful scan start on resume must not strand the remaining
        // GATT and advertising repair.
        scheduler.schedule()
        scheduler.cancel()
        scheduler.onScanStarted(adapterRecoveryPending = true)

        advanceTo(9_999L)
        assertEquals(0, runs)
        advanceTo(10_000L)
        assertEquals(1, runs)
    }

    @Test
    fun `ordinary scan start does not arm adapter recovery`() {
        var runs = 0
        val scheduler = scheduler { runs++ }

        scheduler.onScanStarted(adapterRecoveryPending = false)
        advanceTo(30_000L)

        assertEquals(0, runs)
    }

    private fun scheduler(block: () -> Unit) = BleRecoveryScheduler(
        handler = handler,
        task = Runnable(block),
        minDelayMs = 10_000L,
        maxDelayMs = 30_000L,
    )

    private fun advanceTo(targetMs: Long) {
        looper.idleFor(targetMs - elapsedMs, TimeUnit.MILLISECONDS)
        elapsedMs = targetMs
    }
}
