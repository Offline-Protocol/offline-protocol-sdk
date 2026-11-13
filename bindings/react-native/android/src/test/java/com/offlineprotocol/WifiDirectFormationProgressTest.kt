package com.offlineprotocol

import org.junit.Assert.assertEquals
import org.junit.Assert.assertNull
import org.junit.Test

/**
 * [WifiDirectFormationProgress]: when the "formation still running"
 * diagnostic is reported. The manager decides whether a tick is live (a peer
 * of this application heard, P2P up, not paused, no group and no stream yet);
 * this pins what happens to the attempt across ticks.
 */
class WifiDirectFormationProgressTest {

    private val tickMs = 10_000L

    /** Runs live ticks from [fromMs] to [toMs] inclusive and collects the reports. */
    private fun run(
        state: WifiDirectFormationProgress.State,
        fromMs: Long,
        toMs: Long,
        live: Boolean = true,
    ): Pair<WifiDirectFormationProgress.State, List<WifiDirectFormationProgress.Report>> {
        var s = state
        val reports = mutableListOf<WifiDirectFormationProgress.Report>()
        var now = fromMs
        while (now <= toMs) {
            val (next, report) = WifiDirectFormationProgress.step(s, live, now)
            s = next
            report?.let(reports::add)
            now += tickMs
        }
        return s to reports
    }

    @Test
    fun `nothing is reported while no attempt is live`() {
        val (state, reports) = run(WifiDirectFormationProgress.IDLE, 0, 3_600_000, live = false)
        assertEquals(WifiDirectFormationProgress.IDLE, state)
        assertEquals(emptyList<WifiDirectFormationProgress.Report>(), reports)
    }

    @Test
    fun `a live attempt is reported at one, two and four minutes, then never again`() {
        val (_, reports) = run(WifiDirectFormationProgress.IDLE, 0, 3_600_000)
        assertEquals(
            listOf(
                WifiDirectFormationProgress.Report(60_000, 1),
                WifiDirectFormationProgress.Report(120_000, 2),
                WifiDirectFormationProgress.Report(240_000, 3),
            ),
            reports,
        )
    }

    @Test
    fun `elapsed time counts from the attempt, not from the transport start`() {
        // Not live for an hour (a group was up, say), then live again.
        val (idle, none) = run(WifiDirectFormationProgress.IDLE, 0, 3_600_000, live = false)
        assertEquals(0, none.size)
        val (_, reports) = run(idle, 3_610_000, 3_700_000)
        assertEquals(listOf(WifiDirectFormationProgress.Report(60_000, 1)), reports)
    }

    @Test
    fun `an attempt that ends starts over, reports included`() {
        val (afterFirst, first) = run(WifiDirectFormationProgress.IDLE, 0, 250_000)
        assertEquals(3, first.size)
        val (ended, report) = WifiDirectFormationProgress.step(afterFirst, live = false, nowMs = 260_000)
        assertNull(report)
        assertEquals(WifiDirectFormationProgress.IDLE, ended)
        val (_, second) = run(ended, 270_000, 340_000)
        assertEquals(listOf(WifiDirectFormationProgress.Report(60_000, 1)), second)
    }

    @Test
    fun `a late tick reports once however many marks it crossed`() {
        val (started, _) = WifiDirectFormationProgress.step(WifiDirectFormationProgress.IDLE, true, 0)
        val (afterLate, report) = WifiDirectFormationProgress.step(started, true, 300_000)
        assertEquals(WifiDirectFormationProgress.Report(300_000, 3), report)
        val (_, after) = run(afterLate, 310_000, 3_600_000)
        assertEquals(emptyList<WifiDirectFormationProgress.Report>(), after)
    }
}
