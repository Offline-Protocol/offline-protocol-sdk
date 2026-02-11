package com.offlineprotocol

import android.app.Notification
import android.app.NotificationChannel
import android.app.NotificationManager
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
 * OfflineProtocolModule.
 */
class MeshForegroundService : Service() {

    companion object {
        private const val TAG = "MeshForegroundService"
        private const val NOTIFICATION_ID = 9001
        private const val CHANNEL_ID = "mesh_foreground_channel"
        private const val CHANNEL_NAME = "Mesh Networking"

        private const val ACTION_START = "com.offlineprotocol.action.START_MESH"
        private const val ACTION_STOP = "com.offlineprotocol.action.STOP_MESH"

        @Volatile
        var isRunning: Boolean = false
            private set

        /**
         * Callback invoked when the service is restarted after a process kill
         * (START_STICKY re-delivery). The host module should set this so it can
         * re-initialize the protocol.
         */
        @Volatile
        var onServiceRestarted: (() -> Unit)? = null

        fun start(context: Context) {
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
    }

    override fun onStartCommand(intent: Intent?, flags: Int, startId: Int): Int {
        when (intent?.action) {
            ACTION_STOP -> {
                Log.i(TAG, "Stopping mesh foreground service")
                isRunning = false
                stopForeground(STOP_FOREGROUND_REMOVE)
                stopSelf()
                return START_NOT_STICKY
            }
            ACTION_START -> {
                Log.i(TAG, "Starting mesh foreground service")
                startForeground(NOTIFICATION_ID, buildNotification())
                isRunning = true
            }
            null -> {
                // Service restarted by the system after process kill (START_STICKY).
                // Re-enter foreground immediately to prevent ANR, then notify host.
                Log.i(TAG, "Service restarted after process kill")
                startForeground(NOTIFICATION_ID, buildNotification())
                isRunning = true
                onServiceRestarted?.invoke()
            }
        }
        return START_STICKY
    }

    override fun onDestroy() {
        isRunning = false
        Log.i(TAG, "Mesh foreground service destroyed")
        super.onDestroy()
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
        return NotificationCompat.Builder(this, CHANNEL_ID)
            .setContentTitle("Mesh Active")
            .setContentText("Offline mesh networking is running")
            .setSmallIcon(android.R.drawable.stat_sys_data_bluetooth)
            .setOngoing(true)
            .setPriority(NotificationCompat.PRIORITY_LOW)
            .setCategory(NotificationCompat.CATEGORY_SERVICE)
            .build()
    }
}
