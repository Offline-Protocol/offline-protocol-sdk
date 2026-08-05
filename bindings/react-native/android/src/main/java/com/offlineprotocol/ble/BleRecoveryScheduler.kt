package com.offlineprotocol.ble

import android.os.Handler

/**
 * Owns the single pending BLE-recovery task and its capped backoff ladder.
 * Cancelling deliberate lifecycle work resets the ladder; transient teardown
 * can remove the pending task while preserving the next retry rung.
 */
internal class BleRecoveryScheduler(
    private val handler: Handler,
    private val task: Runnable,
    private val minDelayMs: Long,
    private val maxDelayMs: Long,
) {
    init {
        require(minDelayMs > 0) { "minDelayMs must be positive" }
        require(maxDelayMs >= minDelayMs) { "maxDelayMs must be at least minDelayMs" }
    }

    private var nextDelayMs = minDelayMs

    fun schedule() {
        handler.removeCallbacks(task)
        handler.postDelayed(task, nextDelayMs)
        nextDelayMs = (nextDelayMs * 2).coerceAtMost(maxDelayMs)
    }

    /**
     * A lifecycle resume can get scanning going after [cancel] removed the
     * pending adapter-recovery task. Keep the peripheral repair alive until
     * the caller clears its adapter-off latch.
     */
    fun onScanStarted(adapterRecoveryPending: Boolean) {
        if (adapterRecoveryPending) {
            schedule()
        }
    }

    fun cancel(resetBackoff: Boolean = true) {
        handler.removeCallbacks(task)
        if (resetBackoff) {
            nextDelayMs = minDelayMs
        }
    }

    fun resetBackoff() {
        nextDelayMs = minDelayMs
    }
}
