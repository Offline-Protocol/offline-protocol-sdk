package com.offlineprotocol

import android.app.Notification
import android.app.NotificationChannel
import android.app.NotificationManager
import android.app.PendingIntent
import android.app.Service
import android.content.Context
import android.content.Intent
import android.content.pm.PackageManager
import android.os.Binder
import android.os.Build
import android.os.Bundle
import android.os.Handler
import android.os.IBinder
import android.os.Looper
import android.util.Log
import androidx.core.app.NotificationCompat

/**
 * Foreground Service that keeps the mesh networking process alive
 * when the app is in the background.
 *
 * Lifecycle:
 *   - Started when protocol.start() is called and BLE/WiFi/Internet transports are active.
 *   - Stopped when protocol.stop() is called or the user explicitly disables mesh.
 *   - Uses START_STICKY so the system hands the service back after a process
 *     kill, but that restart only survives if a host has a mesh running again
 *     by the time it lands, or the app opted in to having JavaScript woken so
 *     one can be — see [handleStickyRestart].
 *
 * The service itself does NOT own the protocol or transport instances.
 * It exists solely to prevent the OS from killing the process while mesh
 * networking is active. Protocol and transport lifecycle remains in
 * OfflineProtocolModule — including the notification's Stop action, which
 * hands off to [onStopRequestedByUser] instead of tearing down here.
 */
class MeshForegroundService : Service() {

    companion object {
        private const val TAG = "MeshForegroundService"
        private const val NOTIFICATION_ID = 9001
        private const val CHANNEL_ID = "mesh_foreground_channel"
        private const val CHANNEL_NAME = "Mesh Networking"

        private const val ACTION_START = "com.offlineprotocol.action.START_MESH"
        private const val ACTION_STOP = "com.offlineprotocol.action.STOP_MESH"

        /**
         * Delivered by the notification's Stop action. Deliberately distinct
         * from [ACTION_STOP]: that one is the host telling the service that
         * mesh is *already* going down, while this one is the user asking for
         * a teardown the host has not started yet and must run itself.
         */
        private const val ACTION_STOP_FROM_NOTIFICATION =
            "com.offlineprotocol.action.STOP_MESH_FROM_NOTIFICATION"

        @Volatile
        var isRunning: Boolean = false
            private set

        /**
         * Set by [start], cleared by [stop] and [onDestroy]. Tracks intent
         * rather than state: the service is created asynchronously, so a
         * `stop()` racing a just-issued `start()` sees [isRunning] still false
         * while a service is on its way up.
         */
        @Volatile
        private var startRequested: Boolean = false

        /**
         * Callback invoked when the user taps "Stop" on the service
         * notification. The host module must register one and run the same
         * teardown as its JS-facing `stop()`: this service is only a
         * keep-alive, so dropping it alone would leave BLE/WiFi-Direct/Nostr
         * and the process scheduler running with no foreground protection and
         * no JS-visible state change — the user sees "mesh off" while the
         * radios keep draining until the OS reaps the process.
         *
         * Carries a second meaning that constrains when it may be set: a
         * non-null slot is the only signal in the process that a live host
         * believes mesh is running, which is what [handleStickyRestart] gates
         * on. So it is registered as mesh comes up and surrendered as mesh goes
         * down — see [stop] — rather than tracking the module's own lifetime,
         * which outlives the mesh.
         *
         * Held in a companion field, so a host that captures itself here must
         * drop it on teardown or it pins that host for the process lifetime.
         * Mutated only through [registerStopRequestCallback] and
         * [clearStopRequestCallback] — see the latter for why a host may not
         * simply null it.
         */
        @Volatile
        var onStopRequestedByUser: (() -> Unit)? = null
            private set

        /**
         * Serializes register against clear so the compare-and-clear in
         * [clearStopRequestCallback] cannot lose a registration that lands
         * between its read and its write.
         */
        private val stopCallbackLock = Any()

        /** Installs [callback] as the host to hand a user-initiated stop to. */
        fun registerStopRequestCallback(callback: () -> Unit) {
            synchronized(stopCallbackLock) {
                onStopRequestedByUser = callback
            }
        }

        /**
         * Drops [callback], but only while it is still the registered one.
         *
         * The slot is process-global while hosts are per-ReactContext, and the
         * two overlap: during a React reload a new module can register before
         * the old one tears down. An unconditional null there would disarm the
         * *new* host's Stop action, and the button would then drop the
         * keep-alive while the mesh it belongs to keeps running — the exact
         * failure [onStopRequestedByUser] exists to prevent.
         */
        fun clearStopRequestCallback(callback: () -> Unit) {
            synchronized(stopCallbackLock) {
                if (onStopRequestedByUser === callback) {
                    onStopRequestedByUser = null
                }
            }
        }

        fun start(context: Context) {
            startRequested = true
            val intent = Intent(context, MeshForegroundService::class.java).apply {
                action = ACTION_START
            }
            if (Build.VERSION.SDK_INT >= Build.VERSION_CODES.O) {
                context.startForegroundService(intent)
            } else {
                context.startService(intent)
            }
        }

        /**
         * Brings the keep-alive down, and with it [host]'s registration when
         * that is still the live one.
         *
         * Surrendering the registration is part of stopping rather than a
         * courtesy. The slot doubles as the sticky-restart liveness signal (see
         * [handleStickyRestart]), so leaving it set over a mesh the host has
         * already torn down re-arms the exact restart that gate exists to
         * refuse — "Mesh Active" and START_STICKY over a protocol that is gone.
         * It has to fall with the mesh, which is here, not with the module,
         * which outlives it and may never be torn down at all.
         *
         * The clear runs ahead of the nothing-to-stop guard below, so a host
         * whose service is already down still deregisters, and it clears by
         * identity, so a departing host cannot disarm its replacement — see
         * [clearStopRequestCallback]. [host] is nullable only for callers that
         * hold no registration to surrender; one that does must pass it.
         */
        fun stop(context: Context, host: (() -> Unit)?) {
            host?.let { clearStopRequestCallback(it) }
            // Skip when there is nothing to stop. Callers invoke this from
            // both stop() and invalidate(), so a stop-after-stop would
            // otherwise create an instance purely to tear it down — and since
            // onCreate now promotes to the foreground, that means a visible
            // notification flash during app teardown.
            //
            // Both flags are needed: startRequested covers a stop racing a
            // service that is still coming up (isRunning not yet set), and
            // isRunning covers an instance we never requested. Process death
            // resets the statics, so a sticky restart that found a host and a
            // tap on a stale notification re-creating us both leave a running
            // service that no start() in this process accounts for.
            if (!startRequested && !isRunning) return
            startRequested = false
            val intent = Intent(context, MeshForegroundService::class.java).apply {
                action = ACTION_STOP
            }
            context.startService(intent)
        }
    }

    private val binder = LocalBinder()

    /**
     * Posts the wake watchdog. Main looper, because every other member of this
     * class is main-confined and the watchdog reads [onStopRequestedByUser]
     * against writes the host makes from its own start path.
     */
    private val watchdogHandler = Handler(Looper.getMainLooper())

    /** The armed watchdog, or null when no wake is outstanding. */
    private var wakeWatchdog: Runnable? = null

    inner class LocalBinder : Binder() {
        fun getService(): MeshForegroundService = this@MeshForegroundService
    }

    override fun onBind(intent: Intent?): IBinder = binder

    override fun onCreate() {
        super.onCreate()
        createNotificationChannel()
        // Enter the foreground here rather than waiting for onStartCommand.
        // Android gives an app 5 seconds after startForegroundService() to
        // call startForeground(); if onStartCommand is delayed past that
        // window (JS-thread initialization on cold start, main-thread work
        // during app resume on mid-range devices), the OS terminates the
        // process with a fatal RemoteServiceException. Promoting in
        // onCreate — which runs before any onStartCommand dispatch — makes
        // the deadline unreachable. Subsequent startForeground() calls in
        // onStartCommand are idempotent on the same service instance and
        // remain safe re-promotes.
        promoteToForeground()
    }

    override fun onStartCommand(intent: Intent?, flags: Int, startId: Int): Int {
        when (intent?.action) {
            ACTION_STOP -> {
                Log.i(TAG, "Stopping mesh foreground service")
                cancelWakeWatchdog()
                stopForegroundAndSelf()
                return START_NOT_STICKY
            }
            ACTION_STOP_FROM_NOTIFICATION -> {
                Log.i(TAG, "User requested mesh stop from the notification")
                cancelWakeWatchdog()
                return handleUserStopRequest()
            }
            ACTION_START -> {
                Log.i(TAG, "Starting mesh foreground service")
                // A host has brought a mesh up — which is the outcome a pending
                // wake was waiting for, whether or not the wake is what produced
                // it. Disarming here rather than leaving the watchdog to observe
                // the registered slot keeps the success path from depending on
                // timer ordering.
                cancelWakeWatchdog()
                promoteToForeground()
            }
            null -> return handleStickyRestart()
        }
        return START_STICKY
    }

    /**
     * Handle a null-intent re-delivery — the system restarting us under
     * START_STICKY after the process was killed.
     *
     * The old process took the protocol, every transport and the host module
     * with it, so this instance is only worth keeping if a host in the *new*
     * process has since brought a mesh back up. [onStopRequestedByUser] is
     * exactly that signal, because it is scoped to the mesh and not to the
     * host: the module registers it immediately before starting this service
     * and surrenders it to [stop] as the mesh comes down, so a non-null slot
     * means a host that believes mesh is running *now* — not merely one that
     * started it at some point. A slot that outlived its mesh would send this
     * gate straight back into the behaviour it exists to refuse. That ordering
     * does happen — an app that boots React Native from Application.onCreate
     * can have re-started the mesh before this re-delivered intent lands.
     *
     * With no host and no opted-in wake, staying up is strictly worse than
     * dying. The notification claims "Mesh Active" over a protocol nothing is
     * going to rebuild, and survives until the user taps Stop or the OS reaps
     * us. Worse still where the promotion in [onCreate] did not take: what is
     * left is an empty process squatting with no notification at all. Stopping
     * is the honest outcome for both, and START_NOT_STICKY keeps the system from
     * handing the same restart straight back — an exit, not a loop.
     *
     * That promotion is normally allowed, and the earlier version of this
     * comment was wrong to say otherwise. The API 31+ ban on entering the
     * foreground from the background carves out exactly this case: per the
     * `Service.START_STICKY` reference, "the restriction doesn't impact restarts
     * of a sticky foreground service" — we were foreground when the process
     * died, so the system's own restart may promote. [promoteToForeground] still
     * swallows what it catches, because a `connectedDevice` promotion can fail
     * for unrelated reasons (revoked Nearby-Devices permissions raise
     * SecurityException on targetSdk 34) and OEM behaviour varies. So a refused
     * promotion is the uncommon case rather than the expected one — but it is
     * still a case, and [isRunning] is what tells the two apart below.
     *
     * Stopping without a successful promotion is legal here: the five-second
     * startForeground() obligation attaches to Context.startForegroundService()
     * calls, which a system restart is not.
     *
     * **Rebuilding the protocol here is refused, not merely unimplemented.**
     * Issue #291 proposed persisting the ProtocolConfig natively so this branch
     * could re-create the protocol without JS. It cannot: a protocol running
     * with no React context does not fail to deliver, it *destroys* messages,
     * and tells their senders they arrived. The receive path ACKs before it
     * emits — `send_delivery_ack` runs in `protocol/receive.rs` a dozen lines
     * ahead of the `Event::MessageReceived` it will hand to the event callback —
     * and that ACK makes the sender drop its outbox entry (`remove_outbox_entry`
     * in `protocol/send.rs`), retiring the retry ladder that is otherwise what
     * recovers this whole situation. The event then reaches
     * `OfflineProtocolModule.sendEvent`, whose `canEmitToJs()` gate requires a
     * live React instance *and* a subscribed listener, and is dropped. Nothing
     * catches it on the way past: the core persists outbound and session state,
     * never inbound content, so the message exists only in that dropped event.
     * The MLS ratchet generation is spent by then too, so a resend cannot
     * reconstruct it either.
     *
     * Holding the event natively instead — the way [OfflineProtocolModule]'s
     * sticky dispatcher holds `mesh_stopped_by_user` for a JS that was not
     * listening — does not rescue it. That buffer keeps one entry per key, so
     * it does not scale to one per message, and it is in memory, so it dies
     * with the process whose death is the entire premise of this branch.
     *
     * So the choice on a hostless restart is not "mesh back" versus "mesh
     * down". It is a recoverable outage — sender retries, parks, and pushes for
     * up to seven days, and delivers when this device is next actually running
     * — versus permanent silent loss under a delivery receipt. Stopping is the
     * cheaper of the two by a wide margin.
     *
     * **Waking JavaScript is the sound direction, and is built** — issue #307,
     * opt-in only. It is not an exception to the refusal above but the reason
     * the refusal is affordable: it puts a receiver in place *before* the
     * protocol exists rather than after. Nothing here re-creates anything. The
     * app declares [MeshWakePolicy.META_DATA_ENABLED] in its manifest and
     * registers a wake task with `registerMeshWakeTask`; this branch starts
     * [MeshHeadlessWakeService], which boots React Native and hands over; the
     * task runs the app's ordinary `start()` with the app's own config and
     * credentials, which is what registers [onStopRequestedByUser] again.
     *
     * So the same slot that gates this branch is also what reports the wake
     * landed, and no new state is needed to track one. A wake that never lands —
     * no task registered, JavaScript failed to boot, or the app looked at its
     * own state and declined — leaves the slot empty, and the watchdog armed
     * alongside the wake ([armWakeWatchdog]) stops the service. Without it the
     * "Mesh Active" lie #294 removed would come straight back, since this branch
     * now returns START_STICKY on a bet rather than on an observation.
     */
    private fun handleStickyRestart(): Int {
        val hostPresent = onStopRequestedByUser != null
        val settings = MeshWakePolicy.settingsFrom(readWakeMetaData())
        // isRunning is the onCreate promotion's verdict: it ran before any
        // onStartCommand dispatch, so by here it says whether this service is
        // actually in the foreground.
        return when (MeshWakePolicy.decideRestart(hostPresent, settings.enabled, isRunning)) {
            MeshRestartAction.KEEP_ALIVE -> {
                Log.i(TAG, "Service restarted after process kill; host present")
                promoteToForeground()
                START_STICKY
            }
            MeshRestartAction.WAKE -> {
                Log.i(TAG, "Service restarted after process kill; waking JavaScript")
                if (dispatchWake(settings)) {
                    START_STICKY
                } else {
                    stopForegroundAndSelf()
                    START_NOT_STICKY
                }
            }
            MeshRestartAction.STOP -> {
                Log.i(TAG, "Service restarted after process kill with no mesh host; stopping")
                stopForegroundAndSelf()
                START_NOT_STICKY
            }
        }
    }

    /**
     * Starts the wake service and arms the watchdog, reporting whether the wake
     * is genuinely under way. A false return means the caller must fall back to
     * stopping — an armed START_STICKY with no wake behind it is the failure
     * this whole gate exists to prevent.
     *
     * A plain `startService` is correct and deliberate. This process hosts a
     * foreground service that [MeshWakePolicy.decideRestart] has just confirmed
     * is promoted, which is what makes the start legal; and
     * `HeadlessJsTaskService` never promotes itself, so a `startForegroundService`
     * here would arm the five-second `startForeground` deadline against a
     * service that will never satisfy it.
     *
     * The target is named as a string rather than a class reference — see
     * [MeshWakePolicy.WAKE_SERVICE_CLASS] for why that is load-bearing rather
     * than stylistic.
     */
    private fun dispatchWake(settings: MeshWakeSettings): Boolean = try {
        val intent = Intent().apply {
            setClassName(packageName, MeshWakePolicy.WAKE_SERVICE_CLASS)
            putExtra(MeshWakePolicy.EXTRA_TIMEOUT_MS, settings.timeoutMs)
            putExtra(MeshWakePolicy.EXTRA_REASON, MeshWakePolicy.REASON_STICKY_RESTART)
        }
        startService(intent)
        armWakeWatchdog(MeshWakePolicy.watchdogDelayMs(settings))
        true
    } catch (e: Exception) {
        Log.w(TAG, "Could not start the mesh wake service: ${e.message}", e)
        false
    }

    /**
     * Schedules the check that keeps a failed wake from leaving a notification
     * over a mesh that never came back.
     *
     * Fires strictly after the wake task's own deadline
     * ([MeshWakePolicy.WATCHDOG_GRACE_MS]), so a task still inside its budget is
     * never reaped, and re-reads [onStopRequestedByUser] at that moment rather
     * than trusting anything captured now.
     */
    private fun armWakeWatchdog(delayMs: Long) {
        cancelWakeWatchdog()
        val runnable = Runnable {
            wakeWatchdog = null
            if (MeshWakePolicy.shouldStopOnWatchdog(onStopRequestedByUser != null)) {
                Log.w(TAG, "Mesh wake did not land within its budget; stopping the keep-alive")
                stopForegroundAndSelf()
            } else {
                Log.i(TAG, "Mesh wake landed; keeping the service up")
            }
        }
        wakeWatchdog = runnable
        watchdogHandler.postDelayed(runnable, delayMs)
    }

    /** Disarms the watchdog. Idempotent, and safe with none armed. */
    private fun cancelWakeWatchdog() {
        wakeWatchdog?.let { watchdogHandler.removeCallbacks(it) }
        wakeWatchdog = null
    }

    /**
     * Reads the app's `<application>` meta-data, or null when it cannot be read.
     *
     * Null is treated as "not opted in" by [MeshWakePolicy.settingsFrom], which
     * is the safe direction: an unreadable manifest must not be the reason a
     * notification outlives its mesh.
     */
    @Suppress("DEPRECATION") // The ApplicationInfoFlags overload is API 33+ only.
    private fun readWakeMetaData(): Bundle? = try {
        packageManager.getApplicationInfo(packageName, PackageManager.GET_META_DATA).metaData
    } catch (e: Exception) {
        Log.w(TAG, "Could not read mesh wake meta-data: ${e.message}", e)
        null
    }

    override fun onDestroy() {
        isRunning = false
        startRequested = false
        // The handler outlives this instance and its Runnable captures it, so a
        // watchdog left armed here both leaks and fires against a dead service.
        cancelWakeWatchdog()
        Log.i(TAG, "Mesh foreground service destroyed")
        super.onDestroy()
    }

    /**
     * Enter (or re-enter) the foreground with the mesh notification.
     *
     * Shared by all three promotion sites so none of them can throw where the
     * caller does not expect it. A `connectedDevice`-typed promotion is not
     * failure-free on targetSdk 34: it raises SecurityException once the
     * Nearby-Devices runtime permissions are revoked, and a START_STICKY
     * restart while the app is backgrounded raises
     * ForegroundServiceStartNotAllowedException on Android 12+. Neither is
     * worth taking the process down for by itself, and the ACTION_START path
     * re-promotes afterwards.
     *
     * The residual is worth stating plainly: when the instance was created by
     * startForegroundService() and promotion genuinely fails, swallowing here
     * only defers the kill — the system still raises its own 5-second-timeout
     * RemoteServiceException. This buys survival for transient OEM failures,
     * not immunity.
     */
    private fun promoteToForeground() {
        try {
            startForeground(NOTIFICATION_ID, buildNotification())
            isRunning = true
        } catch (e: Exception) {
            Log.w(TAG, "startForeground failed: ${e.message}", e)
        }
    }

    /**
     * Hand a user-initiated stop to the host, which owns protocol and
     * transport lifecycle. Its teardown ends in [stop], so this service comes
     * down through the ACTION_STOP branch once the mesh is actually off —
     * keeping the notification truthful rather than clearing it while the
     * radios still run.
     */
    private fun handleUserStopRequest(): Int {
        val callback = onStopRequestedByUser
        if (callback != null) {
            try {
                callback.invoke()
                return START_NOT_STICKY
            } catch (e: Exception) {
                Log.w(TAG, "Host stop callback failed; stopping the service directly: ${e.message}", e)
            }
        } else {
            Log.w(TAG, "No host stop callback registered; stopping the service only — transports may still be running")
        }
        // Fallback: nothing to defer to, so at least honour the button by
        // dropping the keep-alive.
        stopForegroundAndSelf()
        return START_NOT_STICKY
    }

    private fun stopForegroundAndSelf() {
        isRunning = false
        stopForeground(STOP_FOREGROUND_REMOVE)
        stopSelf()
    }

    private fun createNotificationChannel() {
        if (Build.VERSION.SDK_INT >= Build.VERSION_CODES.O) {
            val channel = NotificationChannel(
                CHANNEL_ID,
                CHANNEL_NAME,
                NotificationManager.IMPORTANCE_LOW
            ).apply {
                description = "Keeps mesh networking active in the background"
                setShowBadge(false)
            }
            val manager = getSystemService(NotificationManager::class.java)
            manager.createNotificationChannel(channel)
        }
    }

    private fun buildNotification(): Notification {
        // Route the notification's stop action back through this service's
        // own ACTION_STOP_FROM_NOTIFICATION handler. Using
        // PendingIntent.getForegroundService on O+ keeps the delivery legal
        // under the background service-start restrictions that apply when the
        // user taps the action from the notification shade while the app
        // itself is in the background; pre-O falls back to getService which
        // has no such restriction.
        //
        // Note the coupling with the onCreate promotion: if this fires against
        // a dead service instance (a stale notification during a sticky-restart
        // gap), startForegroundService() re-creates the service, and that
        // promotion is the only thing satisfying the 5-second startForeground
        // obligation before the stop is handled. Do not remove one without
        // the other.
        val stopIntent = Intent(this, MeshForegroundService::class.java).apply {
            action = ACTION_STOP_FROM_NOTIFICATION
        }
        val pendingIntentFlags = PendingIntent.FLAG_UPDATE_CURRENT or PendingIntent.FLAG_IMMUTABLE
        val stopPendingIntent = if (Build.VERSION.SDK_INT >= Build.VERSION_CODES.O) {
            PendingIntent.getForegroundService(this, 0, stopIntent, pendingIntentFlags)
        } else {
            PendingIntent.getService(this, 0, stopIntent, pendingIntentFlags)
        }

        return NotificationCompat.Builder(this, CHANNEL_ID)
            .setContentTitle("Mesh Active")
            .setContentText("Offline mesh networking is running")
            .setSmallIcon(android.R.drawable.stat_sys_data_bluetooth)
            .setOngoing(true)
            .setPriority(NotificationCompat.PRIORITY_LOW)
            .setCategory(NotificationCompat.CATEGORY_SERVICE)
            .addAction(android.R.drawable.ic_media_pause, "Stop", stopPendingIntent)
            .build()
    }
}
