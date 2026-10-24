package com.offlineprotocol

import android.util.Log
import java.util.concurrent.Executors
import java.util.concurrent.ScheduledExecutorService
import java.util.concurrent.TimeUnit
import uniffi.offline_protocol.OfflineProtocol

/**
 * Fallback tick that drives [OfflineProtocol.process] and drains [OfflineProtocol.receiveMessage].
 *
 * Shared by React Native and Flutter hosts. Latency-sensitive work is still
 * event-driven through transport callbacks; this scheduler is the safety net.
 */
class ProtocolBackgroundProcessor(
    private val protocolProvider: () -> OfflineProtocol?,
    private val onDiagnostic: (level: String, message: String, context: Map<String, Any?>) -> Unit,
    private val logTag: String = "ProtocolBackgroundProcessor",
) {
    private var processScheduler: ScheduledExecutorService? = null

    /**
     * Start background process scheduler.
     * Uses [PROCESS_INTERVAL_MS] as a fallback tick; latency-sensitive work is
     * driven by transport callbacks (event-driven), not by this timer.
     *
     * Fixed *delay*, not fixed rate. `scheduleAtFixedRate` targets a constant
     * frequency, so when a tick overruns the period the executor makes up the
     * lost ground by running the next ones back-to-back with no gap. Every tick
     * takes the core protocol mutex for its whole body — `process()` plus a
     * `receiveMessage()` drain, both of which re-enter the JVM through
     * `MlsSecureStorage` and so through AndroidKeyStore — so an overrunning tick
     * on a slow device holds that mutex essentially continuously and starves
     * every other FFI caller, the BLE fragment drain included (OFF-2123).
     * Fixed delay guarantees a full [PROCESS_INTERVAL_MS] of slack between ticks
     * no matter how long one takes.
     */
    fun startProcessScheduler() {
        stopProcessScheduler()
        processScheduler = Executors.newSingleThreadScheduledExecutor().apply {
            scheduleWithFixedDelay(
                { processProtocol() },
                0,
                PROCESS_INTERVAL_MS,
                TimeUnit.MILLISECONDS,
            )
        }
    }

    /** Stop background process scheduler and wait for an in-flight tick to finish. */
    fun stopProcessScheduler() {
        val scheduler = processScheduler
        processScheduler = null
        scheduler?.shutdown()
        try {
            scheduler?.awaitTermination(PROCESS_SHUTDOWN_TIMEOUT_MS, TimeUnit.MILLISECONDS)
        } catch (_: InterruptedException) {
            Thread.currentThread().interrupt()
        }
    }

    private fun processProtocol() {
        try {
            val instance = protocolProvider() ?: return
            if (System.currentTimeMillis() % LOG_INTERVAL_MS < LOG_INTERVAL_THRESHOLD_MS) {
                Log.d(logTag, "Processing protocol...")
            }
            instance.process()
            var drained = 0
            while (drained < MAX_RECEIVE_DRAIN_PER_TICK) {
                val message = instance.receiveMessage() ?: break
                drained++
                if (System.currentTimeMillis() % LOG_INTERVAL_MS < LOG_INTERVAL_THRESHOLD_MS) {
                    Log.d(logTag, "Drained protocol message #$drained: $message")
                }
            }
            if (drained >= MAX_RECEIVE_DRAIN_PER_TICK) {
                onDiagnostic(
                    "warning",
                    "Capped receiveMessage drain for this process tick",
                    mapOf("maxBatch" to MAX_RECEIVE_DRAIN_PER_TICK),
                )
            }
        } catch (e: Exception) {
            Log.e(logTag, "Process error: ${e.message}", e)
            onDiagnostic(
                "error",
                "Protocol process error",
                mapOf(
                    "error" to (e.message ?: "unknown"),
                    "exception" to e.javaClass.simpleName,
                ),
            )
        }
    }

    companion object {
        /** Matches iOS 100ms tick. */
        const val PROCESS_INTERVAL_MS = 100L
        private const val PROCESS_SHUTDOWN_TIMEOUT_MS = 2_000L
        private const val MAX_RECEIVE_DRAIN_PER_TICK = 100
        private const val LOG_INTERVAL_MS = 5000L
        private const val LOG_INTERVAL_THRESHOLD_MS = 100L
    }
}
