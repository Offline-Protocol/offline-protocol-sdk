package com.offlineprotocol.ble

import android.os.Handler

/**
 * Owns the capped backoff ladder for the outbound drain's backpressure retry.
 *
 * The drain re-arms itself whenever it had to leave fragments unsent — a peer
 * whose link is not ready, whose queue is backed up, or whose write failed.
 * That retry used to be a flat 50ms repost with no ceiling, which is fine for
 * the transient stall it was written for and pathological for a peer that is
 * *permanently* unable to accept writes: a half-open link whose CCCD is never
 * acked keeps `sendFragmentData` returning false forever, so the drain reposts
 * at 20Hz indefinitely. Each pass takes the core protocol mutex through
 * `bleGetNextFragment`, contending with the 100ms `process()` tick — and back
 * when the drain still ran on the app's main looper, that aggregate wait is
 * what surfaced as an ANR (OFF-2123) rather than any single long block. The
 * drain has since moved off main, so the ceiling no longer stands between the
 * app and an ANR; it stands between one wedged peer and a pointless 20Hz
 * contention loop on the shared protocol mutex, which still starves every
 * other FFI caller.
 *
 * The ladder keeps the fast first retries — a genuinely transient stall still
 * clears in tens of milliseconds — then decays to [maxDelayMs] and finally
 * stops re-arming altogether after [maxConsecutiveAttempts]. Stopping is safe
 * because it is not a delivery decision: the 2s `fragmentPollingRunnable` is an
 * unconditional floor of service that keeps flushing the outbound queue, so the
 * ceiling degrades the retry rate from ~20Hz to 0.5Hz without ever abandoning
 * the fragments. [reset] re-arms the fast ladder as soon as anything is
 * actually sent, so a peer that recovers is not punished for having stalled.
 * "Sent" means a fragment the stack accepted, never a queue that merely shrank
 * — see [FlushResult].
 *
 * BLE-thread only, like the drain it serves; it holds no lock of its own.
 */
internal class BackpressureRetryPolicy(
    private val handler: Handler,
    private val task: Runnable,
    private val minDelayMs: Long,
    private val maxDelayMs: Long,
    private val maxConsecutiveAttempts: Int,
) {
    init {
        require(minDelayMs > 0) { "minDelayMs must be positive" }
        require(maxDelayMs >= minDelayMs) { "maxDelayMs must be at least minDelayMs" }
        require(maxConsecutiveAttempts > 0) { "maxConsecutiveAttempts must be positive" }
    }

    private var nextDelayMs = minDelayMs
    private var consecutiveAttempts = 0

    /** Consecutive retries armed since the last [reset]. For diagnostics. */
    val attempts: Int get() = consecutiveAttempts

    /**
     * Arm the next retry, or report that the ceiling is reached.
     *
     * @return true if a retry was scheduled; false if [maxConsecutiveAttempts]
     *   is exhausted and the caller should fall through to the polling floor.
     *   The caller is expected to surface that as a diagnostic — a peer that
     *   burns the whole ladder is stuck in a way worth naming.
     */
    fun schedule(): Boolean {
        if (consecutiveAttempts >= maxConsecutiveAttempts) {
            return false
        }
        consecutiveAttempts++
        handler.removeCallbacks(task)
        handler.postDelayed(task, nextDelayMs)
        nextDelayMs = (nextDelayMs * 2).coerceAtMost(maxDelayMs)
        return true
    }

    /**
     * Progress was made — a fragment actually reached the stack. Return the
     * ladder to its floor so the next stall retries fast again.
     */
    fun reset() {
        nextDelayMs = minDelayMs
        consecutiveAttempts = 0
    }

    /** Drop any pending retry and return the ladder to its floor. */
    fun cancel() {
        handler.removeCallbacks(task)
        reset()
    }
}
