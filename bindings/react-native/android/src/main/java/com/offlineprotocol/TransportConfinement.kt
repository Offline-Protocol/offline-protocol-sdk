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
 * ## What may run here
 *
 * Anything whose worst case is the protocol mutex, and nothing whose worst
 * case is the network. That rule is not a preference — it is what makes
 * [runSync]'s unbounded background wait safe. The mutex is always being
 * drained by some other thread, so a wait behind it ends; a blocking socket
 * call is bounded by a peer that may never answer, and a `stop()` queued
 * behind one waits exactly as long, taking the React Native bridge thread
 * with it. `ReticulumManager` is the worked example: its `connect` blocks for
 * up to 60s and its TCP writes have no timeout at all, so they get a second
 * confinement of their own (`offline-reticulum-io`) and the lifecycle thread
 * stays answerable.
 *
 * The same reasoning bounds long *non*-blocking work when the thread is
 * shared with something that has its own deadline. A looper handed to
 * `WifiP2pManager.initialize` or to `registerReceiver` as a scheduler is
 * delivering framework callbacks, and a broadcast that misses Android's
 * dispatch budget is an ANR wherever its receiver lives — see
 * `WifiDirectManager.drainAndSendMessages`, which spends a batch budget and
 * reposts rather than draining the queue in one pass.
 *
 * ## What may never wait on this
 *
 * Anything the core calls *into*. The transport callbacks
 * (`onMessagesAvailable`, `onFragmentsAvailable`) are invoked while the core
 * holds the global protocol mutex, so a callback that waited on a confinement
 * thread would be waiting on a thread whose next act is to ask for that same
 * mutex — a permanent deadlock of the core rather than a slow path, and the
 * only rule here whose violation nothing recovers from. Every such callback
 * therefore hands its work to a handler and returns, which
 * `react_native_transport_callbacks_never_wait_on_a_confinement` in the
 * `offline-protocol-uniffi` crate pins. That rule is also what makes [runSync]
 * safe to leave unbounded for background callers.
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
        if (isCurrent()) action() else post(action)
    }

    /**
     * Posts [action] to the back of this thread's queue, always.
     *
     * Checked for the same reason [runSync] checks: a looper that has quit
     * accepts nothing and says so only in this return value. The failure is
     * quieter here — dropped work rather than a permanent park — which is
     * precisely why it needs saying out loud, since a silently discarded status
     * flip or teardown step is diagnosed months later if at all. Threads handed
     * out by [shared] never quit, so this cannot trip in production; it is here
     * because the constructor takes any [Looper].
     */
    fun post(action: () -> Unit) {
        check(handler.post(action)) { "$name is not accepting work: its looper has quit" }
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
     *
     * What an expiry must not do is lose a *failure*. Nothing reads the result
     * once the caller has given up, so an abandoned action that throws would
     * vanish with no log and no rethrow — a `start()` refused for a real reason
     * would look like a timeout and nothing else. The follow-up post below
     * reports it, and it is a post rather than a flag read inside the action
     * precisely so it cannot race: queued on this same thread, it runs strictly
     * after the action that recorded the outcome it reads.
     */
    fun <T> runSync(action: () -> T): T {
        if (isCurrent()) {
            return action()
        }

        val onMainThread = Looper.myLooper() === Looper.getMainLooper()
        val latch = CountDownLatch(1)
        var outcome: Result<T>? = null
        val accepted = handler.post {
            outcome = try {
                Result.success(action())
            } catch (t: Throwable) {
                Result.failure(t)
            }
            latch.countDown()
        }

        // A looper that has quit accepts nothing, and the background wait below
        // has no bound to rescue it from that: the latch would never count down
        // and the caller — React Native's native-modules thread, on the stop
        // path — would park for the life of the process. Threads handed out by
        // [shared] never quit, so this cannot trip in production; it is here
        // because the constructor takes any [Looper], and a loud failure beats
        // a silent hang.
        check(accepted) { "$name is not accepting work: its looper has quit" }

        try {
            if (onMainThread) {
                if (!latch.await(mainSyncTimeoutMs, TimeUnit.MILLISECONDS)) {
                    Log.w(
                        TAG,
                        "$name did not answer a main-thread caller within " +
                            "${mainSyncTimeoutMs}ms; continuing without it",
                    )
                    handler.post {
                        outcome?.exceptionOrNull()?.let { failure ->
                            Log.w(TAG, "$name failed an abandoned action", failure)
                        }
                    }
                    throw MainThreadSyncTimeout(name, mainSyncTimeoutMs)
                }
            } else {
                latch.await()
            }
        } catch (ie: InterruptedException) {
            Thread.currentThread().interrupt()
            throw RuntimeException("Interrupted while executing on $name", ie)
        }

        // Non-null once the latch has been counted down, and the count-down is
        // what the await above returned on — so this is a guarantee, not an
        // assumption. Spelled out rather than `!!` because the guarantee lives
        // in another block.
        val completed = outcome
            ?: error("$name signalled completion without recording an outcome")
        return completed.getOrThrow()
    }

    // There is deliberately no `assertConfined(...)` helper here, and the
    // omission is the considered answer rather than an oversight. A throwing
    // contract check is only useful where the throw is caught, and almost every
    // "transport-thread only" body in these managers is reachable from a posted
    // block, where an uncaught IllegalStateException takes the process down —
    // trading a state bug for a crash, which is exactly what the iOS
    // `dispatchPrecondition` in `drainProcessQueue` had to be walled behind
    // `#if DEBUG` to avoid. Kotlin has no equivalent compile-time gate here:
    // narrowing the check to debuggable builds needs `ApplicationInfo`, and
    // this class holds no Context on purpose (see [shared]). The invariants
    // stay documented at each site, and the source guards in
    // `offline-protocol-uniffi` pin the ones whose violation is not recoverable.

    /** This confinement's thread did not answer a main-thread caller in time. */
    class MainThreadSyncTimeout(threadName: String, timeoutMs: Long) :
        RuntimeException("$threadName did not respond within ${timeoutMs}ms")

    companion object {
        private const val TAG = "TransportConfinement"

        /**
         * Far below the 5s input-dispatch ANR budget, and far above a healthy
         * queue turnaround.
         *
         * Per call, which is not the same as per teardown, and the difference
         * is worth knowing before anyone raises this. Confinements are
         * per-transport, so the one main-reachable caller that stops several
         * in a row — `OfflineProtocolModule.invalidate`, five managers
         * including the BLE facade's own bound of the same size — can spend
         * this five times over. That is still strictly better than what it
         * replaced (from main the old helpers took an inline fast path
         * straight into an unbounded UniFFI call), and `invalidate` runs on a
         * React Native internal thread rather than main today, which is why
         * the bound is enforced rather than relied on. But the headroom the
         * number claims belongs to one call, not to the sequence.
         */
        const val MAIN_THREAD_SYNC_TIMEOUT_MS = 1_000L

        private val instances = HashMap<String, TransportConfinement>()

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
        fun shared(name: String): TransportConfinement = instances.getOrPut(name) {
            val thread = HandlerThread(name).apply { start() }
            TransportConfinement(name, thread.looper)
        }

        /**
         * Drops the confinement named [name] and quits its thread. Tests only.
         *
         * It exists so a test that takes a [shared] thread — the reuse contract
         * cannot be verified without taking one — does not leave it running for
         * the rest of the suite. Quitting the looper by hand instead is worse
         * than leaving it: the entry survives, so the next lookup of that name
         * returns a confinement that accepts nothing, and Robolectric reuses a
         * sandbox (and therefore these statics) across test classes with the
         * same config. Removing the entry and quitting the thread has to be one
         * step, which is what this is.
         */
        @Synchronized
        fun releaseForTests(name: String) {
            instances.remove(name)?.looper?.quitSafely()
        }
    }
}
