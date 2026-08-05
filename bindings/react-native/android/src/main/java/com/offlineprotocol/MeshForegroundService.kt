package com.offlineprotocol

import android.app.Notification
import android.app.NotificationChannel
import android.app.NotificationManager
import android.app.PendingIntent
import android.app.Service
import android.content.Context
import android.content.Intent
import android.os.Binder
import android.os.Build
import android.os.IBinder
import android.util.Log
import androidx.core.app.NotificationCompat

/**
 * Foreground Service that keeps the mesh networking process alive
 * when the app is in the background.
 *
 * Lifecycle:
 *   - Started when protocol.start() is called and BLE/WiFi/Internet transports are active.
 *   - Stopped when protocol.stop() is called or the user explicitly disables mesh.
 *   - Uses START_STICKY so the system restarts the service after a process kill.
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
         * Callback invoked when the service is restarted after a process kill
         * (START_STICKY re-delivery). The host module should set this so it can
         * re-initialize the protocol.
         */
        @Volatile
        var onServiceRestarted: (() -> Unit)? = null

        /**
         * Callback invoked when the user taps "Stop" on the service
         * notification. The host module must set this and run the same
         * teardown as its JS-facing `stop()`: this service is only a
         * keep-alive, so dropping it alone would leave BLE/WiFi-Direct/Nostr
         * and the process scheduler running with no foreground protection and
         * no JS-visible state change — the user sees "mesh off" while the
         * radios keep draining until the OS reaps the process.
         *
         * Held in a companion field, so a host that captures itself here must
         * null it out on teardown or it pins that host for the process
         * lifetime.
         */
        @Volatile
        var onStopRequestedByUser: (() -> Unit)? = null

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

        fun stop(context: Context) {
            // Skip when there is nothing to stop. Callers invoke this from
            // both stop() and invalidate(), so a stop-after-stop would
            // otherwise create an instance purely to tear it down — and since
            // onCreate now promotes to the foreground, that means a visible
            // notification flash during app teardown.
            //
            // Both flags are needed: startRequested covers a stop racing a
            // service that is still coming up (isRunning not yet set), and
            // isRunning covers an instance we never requested — a START_STICKY
            // restart after process death, which resets the statics.
            if (!startRequested && !isRunning) return
            startRequested = false
            val intent = Intent(context, MeshForegroundService::class.java).apply {
                action = ACTION_STOP
            }
            context.startService(intent)
        }
    }

    private val binder = LocalBinder()

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
                stopForegroundAndSelf()
                return START_NOT_STICKY
            }
            ACTION_STOP_FROM_NOTIFICATION -> {
                Log.i(TAG, "User requested mesh stop from the notification")
                return handleUserStopRequest()
            }
            ACTION_START -> {
                Log.i(TAG, "Starting mesh foreground service")
                promoteToForeground()
            }
            null -> {
                // Service restarted by the system after process kill (START_STICKY).
                // Re-enter foreground immediately to prevent ANR, then notify host.
                Log.i(TAG, "Service restarted after process kill")
                promoteToForeground()
                onServiceRestarted?.invoke()
            }
        }
        return START_STICKY
    }

    override fun onDestroy() {
        isRunning = false
        startRequested = false
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
