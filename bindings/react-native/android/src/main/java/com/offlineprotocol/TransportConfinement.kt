package com.offlineprotocol

import android.os.Handler
import android.os.HandlerThread
import android.os.Looper
import android.util.Log
import java.util.concurrent.CountDownLatch
import java.util.concurrent.TimeUnit

/**
 * The single thread one transport manager runs on, and the primitive it uses
 * to keep its own state ordered.
 *
 * Every transport manager here needs exactly one property from a thread: that
 * its own posts run one at a time, in order. All four used to take that from
 * the app's main looper, and that is what OFF-2123 is: every call into UniFFI
 * serialises on the core protocol's single global mutex, which is held across
 * MLS work and AndroidKeyStore-backed storage callbacks, so a manager posting
 * FFI to main charges those multi-second waits to the thread Android watches
 * for ANRs. None of these managers touches the UI — main was only ever "one
 * thread, ordered posts", and a private looper is the same primitive without
 * the app's responsiveness in its blast radius.
 *
 * The BLE facade reached this shape first (see `ble.BleTransportFacade`'s
 * `bleLooper`); this is that remedy extracted so the remaining transports share
 * one implementation instead of four subtly different ones. They *were* four
 * different ones: BLE bounded only main-thread callers, Nostr and Reticulum
 * bounded every caller at a flat 5s — which turns a slow mutex into a failed
 * `stop()` — and Internet bounded nobody at all, so an RN bridge call could
 * park forever behind a stalled main looper.
 *
 * ## Lifecycle
 *
 * Threads obtained through [shared] are process-wide and never quit. That is
 * deliberate and matches what the main looper they replace did: no teardown
 * path can strand a pending post on a dead looper, and a manager rebuilt after
 * `stop()` inherits the same ordered queue instead of racing a fresh one
 * against the old one's backlog. One idle thread per transport the app has
 * actually started is the entire cost.
 */
internal class TransportConfinement(
    /** Thread name, for stack traces and ANR/hang reports. */
    private val name: String,
    /** The looper every action runs on. */
    val looper: Looper,
    /** How long a main-thread caller waits in [runSync] before giving up. */
    private val mainSyncTimeoutMs: Long = MAIN_THREAD_SYNC_TIMEOUT_MS,
) {

    val handler = Handler(looper)

    /** True when the calling thread is this confinement's thread. */
    fun isCurrent(): Boolean = Looper.myLooper() === looper

    /** Runs [action] inline when already confined, otherwise posts it. */
    fun run(action: () -> Unit) {
        if (isCurrent()) action() else handler.post(action)
    }

    /** Posts [action] to the back of this thread's queue, always. */
    fun post(action: () -> Unit) {
        handler.post(action)
    }

    /**
     * Run [action] on this thread and wait for it.
     *
     * The wait is unbounded for every caller except the app's main thread, and
     * that exception is the whole point rather than a defensive nicety. This
     * thread can legitimately sit for seconds inside a UniFFI call waiting on
     * the core protocol mutex; blocking main on it would rebuild the very ANR
     * the confinement exists to escape, just through the lifecycle door
     * instead of the message pump.
     *
     * The bound is only for main. Background callers — React Native's
     * native-modules thread, the mesh stop thread — keep an unbounded wait on
     * purpose: they are the ones that rely on `stop()` having actually
     * finished before teardown continues, and starving *them* is not an ANR.
     * A flat timeout for everyone, which two of these managers used to have,
     * fails a `stop()` precisely when the mutex is most contended and leaves
     * the transport half-down.
     *
     * On expiry the work is not cancelled: it still runs here, we simply stop
     * waiting, and the caller gets [MainThreadSyncTimeout]. Every action
     * routed through this is self-contained and idempotent, so completing late
     * is safe, and the main-reachable lifecycle paths run their transport stops
     * through [TeardownSequence], which records a throwing step and carries on.
     * One late-completing action beats an ANR.
     */
    fun <T> runSync(action: () -> T): T {
        if (isCurrent()) {
            return action()
        }

        val onMainThread = Looper.myLooper() === Looper.getMainLooper()
        val latch = CountDownLatch(1)
        var outcome: Result<T>? = null
        handler.post {
            outcome = try {
                Result.success(action())
            } catch (t: Throwable) {
                Result.failure(t)
            }
            latch.countDown()
        }

        try {
            if (onMainThread) {
                if (!latch.await(mainSyncTimeoutMs, TimeUnit.MILLISECONDS)) {
                    Log.w(
                        TAG,
                        "$name did not answer a main-thread caller within " +
                            "${mainSyncTimeoutMs}ms; continuing without it",
                    )
                    throw MainThreadSyncTimeout(name, mainSyncTimeoutMs)
                }
            } else {
                latch.await()
            }
        } catch (ie: InterruptedException) {
            Thread.currentThread().interrupt()
            throw RuntimeException("Interrupted while executing on $name", ie)
        }

        return outcome!!.getOrThrow()
    }

    /**
     * Runtime contract check for state documented as confined to this thread.
     * These invariants used to live only in comments, and comments drift; a
     * check fails loud instead of corrupting state silently.
     */
    fun assertConfined(reason: String) {
        check(isCurrent()) {
            "$reason must run on $name (was ${Thread.currentThread().name})"
        }
    }

    /** This confinement's thread did not answer a main-thread caller in time. */
    class MainThreadSyncTimeout(threadName: String, timeoutMs: Long) :
        RuntimeException("$threadName did not respond within ${timeoutMs}ms")

    companion object {
        private const val TAG = "TransportConfinement"

        /**
         * Far below the 5s input-dispatch ANR budget, and far above a healthy
         * queue turnaround.
         */
        const val MAIN_THREAD_SYNC_TIMEOUT_MS = 1_000L

        private val shared = HashMap<String, TransportConfinement>()

        /**
         * The process-wide confinement named [name], started on first use and
         * never quit — see the lifecycle note on the class.
         *
         * Keyed by name so a manager rebuilt after `stop()` re-attaches to the
         * queue its predecessor was posting to, rather than racing a fresh
         * thread against the old one's backlog.
         *
         * Deliberately takes nothing from the manager that asks for it. These
         * instances outlive every manager, so capturing one — a diagnostic
         * emitter, a listener — would pin it, and with it the React context
         * and Android [android.content.Context] it holds, for the life of the
         * process. A timeout therefore reports through the log and the thrown
         * [MainThreadSyncTimeout], both of which reach the caller's own
         * reporting without a retained reference.
         */
        @Synchronized
        fun shared(name: String): TransportConfinement = shared.getOrPut(name) {
            val thread = HandlerThread(name).apply { start() }
            TransportConfinement(name, thread.looper)
        }
    }
}
