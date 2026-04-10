package com.offlineprotocol.ble.nordic

import android.bluetooth.le.AdvertiseCallback
import android.bluetooth.le.AdvertiseData
import android.bluetooth.le.AdvertiseSettings
import android.bluetooth.le.BluetoothLeAdvertiser
import android.os.Handler
import android.util.Log
import java.util.concurrent.ThreadLocalRandom

/**
 * Wraps the platform BLE advertiser with the bookkeeping previously inlined
 * in [BleTransportFacade]: a `pendingAdvertiseReason` gate so start calls
 * can be deferred while the GATT service is still being registered, a
 * cooldown / jitter-based restart scheduler, and a single place for the
 * [AdvertiseCallback] lifecycle.
 *
 * Note: the `Nordic*` class name prefix (and enclosing package) is a
 * naming legacy from an earlier migration plan that considered depending
 * on Nordic's Android-BLE-Library. This implementation does not use that
 * library and is a pure `android.bluetooth.le` wrapper.
 */
class NordicAdvertiser(
    private val mainHandler: Handler,
    private val host: Host,
    private val diagnosticEmitter: (level: String, message: String, context: Map<String, Any?>) -> Unit =
        { _, _, _ -> },
) {

    /** Callbacks the advertiser needs to get work done without knowing about mesh state. */
    interface Host {
        /** Is the GATT service registration complete? Controls whether start can proceed. */
        fun isGattServerReady(): Boolean

        /** Build the primary advertisement data (called just-in-time on start). */
        fun buildAdvertiseData(): AdvertiseData

        /** Build the scan response data (called just-in-time on start). */
        fun buildScanResponse(): AdvertiseData

        /**
         * Called before a scheduled restart runs, so the host can refresh any
         * state that feeds the advertisement (e.g. signed identity in the GATT
         * server's identity characteristic).
         */
        fun onBeforeRefresh()

        /** Throttled logging hook owned by the host. */
        fun shouldLog(key: String, intervalMs: Long): Boolean
    }

    companion object {
        private const val TAG = "NordicAdvertiser"
        private const val MIN_ADVERTISE_INTERVAL_MS = 1500L
        private const val ADVERTISE_RESTART_MIN_MS = 200L
        private const val ADVERTISE_RESTART_MAX_MS = 1200L
    }

    private var advertiser: BluetoothLeAdvertiser? = null
    private var advertiseCallback: AdvertiseCallback? = null

    @Volatile
    var isAdvertising: Boolean = false
        private set

    private var lastAdvertiseRestartAt: Long = 0L
    private var pendingAdvertiseRestart: Runnable? = null
    private var pendingAdvertiseReason: String? = null

    /** Provide (or replace) the platform BluetoothLeAdvertiser. */
    fun attachAdvertiser(advertiser: BluetoothLeAdvertiser?) {
        this.advertiser = advertiser
    }

    fun hasAdvertiser(): Boolean = advertiser != null

    /**
     * Start advertising. If the GATT service is not yet ready, the reason is
     * latched in [pendingAdvertiseReason] and [onGattServerReady] will kick
     * off the actual start when the service registration lands.
     */
    fun start(reason: String) {
        if (isAdvertising) return

        if (!host.isGattServerReady()) {
            pendingAdvertiseReason = reason
            if (host.shouldLog("advert_waiting_gatt", 5000L)) {
                Log.i(TAG, "Waiting for GATT service to be ready before advertising (reason: $reason)")
                diagnosticEmitter(
                    "info",
                    "Waiting for GATT service registration",
                    mapOf("reason" to reason),
                )
            }
            return
        }

        try {
            val settings = AdvertiseSettings.Builder()
                .setAdvertiseMode(AdvertiseSettings.ADVERTISE_MODE_LOW_LATENCY)
                .setConnectable(true)
                .setTimeout(0)
                .setTxPowerLevel(AdvertiseSettings.ADVERTISE_TX_POWER_HIGH)
                .build()

            val advertiseData = host.buildAdvertiseData()
            // Include scan response with service UUID for iOS compatibility.
            // iOS's CoreBluetooth actively queries for scan responses and has known
            // issues recognizing 128-bit service UUIDs from Android's main
            // advertisement packet format.
            val scanResponse = host.buildScanResponse()

            val cb = object : AdvertiseCallback() {
                override fun onStartSuccess(settingsInEffect: AdvertiseSettings) {
                    Log.i(TAG, "BLE advertising started successfully (reason=$reason)")
                    isAdvertising = true
                    lastAdvertiseRestartAt = System.currentTimeMillis()
                    diagnosticEmitter(
                        "info",
                        "BLE advertising started",
                        mapOf("reason" to reason),
                    )
                }

                override fun onStartFailure(errorCode: Int) {
                    val errorMsg = when (errorCode) {
                        ADVERTISE_FAILED_DATA_TOO_LARGE -> "Data too large"
                        ADVERTISE_FAILED_TOO_MANY_ADVERTISERS -> "Too many advertisers"
                        ADVERTISE_FAILED_ALREADY_STARTED -> "Already started"
                        ADVERTISE_FAILED_INTERNAL_ERROR -> "Internal error"
                        ADVERTISE_FAILED_FEATURE_UNSUPPORTED -> "Feature unsupported"
                        else -> "Unknown error $errorCode"
                    }
                    Log.e(TAG, "❌ BLE advertising failed: $errorMsg (code=$errorCode)")
                    isAdvertising = false
                    diagnosticEmitter(
                        "error",
                        "BLE advertising failed",
                        mapOf("errorCode" to errorCode, "errorMessage" to errorMsg),
                    )
                }
            }
            advertiseCallback = cb

            advertiser?.startAdvertising(settings, advertiseData, scanResponse, cb)
        } catch (e: SecurityException) {
            Log.e(TAG, "Permission denied while starting advertising", e)
            diagnosticEmitter(
                "error",
                "Permission denied while starting advertising",
                mapOf("exception" to e.javaClass.simpleName, "message" to (e.message ?: "unknown")),
            )
            throw e
        }
    }

    fun stop() {
        if (!isAdvertising) return
        try {
            val cb = advertiseCallback
            if (cb != null) {
                advertiser?.stopAdvertising(cb)
            }
            advertiseCallback = null
            isAdvertising = false
            pendingAdvertiseRestart?.let {
                mainHandler.removeCallbacks(it)
                pendingAdvertiseRestart = null
            }
            Log.i(TAG, "Stopped advertising")
            diagnosticEmitter("info", "Stopped BLE advertising", emptyMap())
        } catch (e: SecurityException) {
            Log.e(TAG, "Permission denied while stopping advertising", e)
            diagnosticEmitter(
                "error",
                "Permission denied while stopping advertising",
                mapOf("exception" to e.javaClass.simpleName, "message" to (e.message ?: "unknown")),
            )
        }
    }

    fun refresh(reason: String) {
        stop()
        host.onBeforeRefresh()
        scheduleRestart(reason)
    }

    private fun scheduleRestart(reason: String) {
        val now = System.currentTimeMillis()
        val elapsed = now - lastAdvertiseRestartAt
        val cooldownDelay = if (elapsed < MIN_ADVERTISE_INTERVAL_MS) {
            MIN_ADVERTISE_INTERVAL_MS - elapsed
        } else {
            0L
        }
        val jitter = ThreadLocalRandom.current()
            .nextLong(ADVERTISE_RESTART_MIN_MS, ADVERTISE_RESTART_MAX_MS + 1)
        val delay = cooldownDelay + jitter
        pendingAdvertiseRestart?.let { mainHandler.removeCallbacks(it) }
        val runnable = Runnable {
            pendingAdvertiseRestart = null
            start(reason)
        }
        pendingAdvertiseRestart = runnable
        mainHandler.postDelayed(runnable, delay)
    }

    /** Call when NordicGattServer signals the service registration has completed. */
    fun onGattServerReady() {
        val reason = pendingAdvertiseReason
        if (reason != null) {
            pendingAdvertiseReason = null
            Log.i(TAG, "📡 Starting deferred advertising after GATT service ready")
            mainHandler.post { start("gatt_service_ready") }
        }
    }

    fun clearPendingReason() {
        pendingAdvertiseReason = null
    }

    fun shutdown() {
        stop()
        pendingAdvertiseRestart?.let { mainHandler.removeCallbacks(it) }
        pendingAdvertiseRestart = null
        pendingAdvertiseReason = null
        lastAdvertiseRestartAt = 0L
        advertiser = null
    }
}
