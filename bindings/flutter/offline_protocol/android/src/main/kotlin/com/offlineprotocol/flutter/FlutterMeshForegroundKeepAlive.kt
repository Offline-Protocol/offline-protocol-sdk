package com.offlineprotocol.flutter

import android.content.Context
import android.util.Log
import com.offlineprotocol.MeshForegroundService

/**
 * Keeps the shared [MeshForegroundService] in sync with Flutter engine start/stop
 * (same role as [com.offlineprotocol.OfflineProtocolModule] on React Native).
 */
internal object FlutterMeshForegroundKeepAlive {
    private const val TAG = "FlutterMeshForeground"

    @Volatile
    private var stopCallback: (() -> Unit)? = null

    fun start(context: Context, onUserStopRequested: () -> Unit) {
        try {
            val callback = {
                try {
                    onUserStopRequested()
                } catch (e: Exception) {
                    Log.e(TAG, "User-requested mesh stop failed", e)
                }
            }
            stopCallback = callback
            MeshForegroundService.registerStopRequestCallback(callback)
            MeshForegroundService.start(context.applicationContext)
        } catch (e: Exception) {
            Log.w(TAG, "Failed to start mesh foreground service: ${e.message}", e)
        }
    }

    fun stop(context: Context) {
        try {
            val ours = stopCallback
            stopCallback = null
            MeshForegroundService.stop(context.applicationContext, ours)
        } catch (e: Exception) {
            Log.w(TAG, "Failed to stop mesh foreground service: ${e.message}", e)
        }
    }
}
