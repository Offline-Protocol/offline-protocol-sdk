package com.offlineprotocol.ble

import android.bluetooth.le.AdvertiseCallback
import android.bluetooth.le.AdvertiseData
import android.bluetooth.le.AdvertiseSettings
import android.bluetooth.le.BluetoothLeAdvertiser
import android.os.Handler
import android.util.Log
import java.util.concurrent.ThreadLocalRandom

/**
 * Wraps the platform BLE advertiser with:
 *   - a `pendingAdvertiseReason` gate so start calls can be deferred while
 *     the GATT service is still being registered,
 *   - a cooldown / jitter-based restart scheduler,
 *   - a single place for the [AdvertiseCallback] lifecycle.
 */
class LeAdvertiser(
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
         * Called just before a scheduled advertise restart runs so the host
         * can refresh the signed identity that will be served via the GATT
         * identity characteristic. This is load-bearing: centrals reading
         * identity on the reconnect would otherwise see stale bytes.
         * Must be safe to invoke on the main thread and must not block on
         * network or disk.
         */
        fun refreshPublishedIdentity()

        /** Throttled logging hook owned by the host. */
        fun shouldLog(key: String, intervalMs: Long): Boolean
    }

    companion object {
        private const val TAG = "LeAdvertiser"
        private const val MIN_ADVERTISE_INTERVAL_MS = 1500L
        private const val ADVERTISE_RESTART_MIN_MS = 200L
        private const val ADVERTISE_RESTART_MAX_MS = 1200L
    }

    private var advertiser: BluetoothLeAdvertiser? = null
    private var advertiseCallback: AdvertiseCallback? = null

    @Volatile
    var isAdvertising: Boolean = false
        private set

    /**
     * Re-entry gate for [start]. Flipped to true synchronously on every
     * accepted call and flipped back on [stop] or on a terminal
     * `onStartFailure`. This used to be inferred from
     * `advertiseCallback != null`, but that reference is cleared on a
     * main-handler post in the failure callback, which left a window where
     * a synchronous `start()` after a failure would be silently dropped by
     * the gate. Separating the gate from the stop-reference closes it.
     */
    @Volatile
    private var startInFlight: Boolean = false

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
        // Gate on a dedicated in-flight flag, not on `isAdvertising`. The
        // latter is only flipped to true once the platform `onStartSuccess`
        // callback arrives on a private Binder thread, which leaves a window
        // where a fast stop() → start() on the main thread would submit a
        // second startAdvertising while the first is still in flight and
        // earn itself ADVERTISE_FAILED_ALREADY_STARTED.
        if (startInFlight) return

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

        startInFlight = true
        try {
            val settings = AdvertiseSettings.Builder()
                .setAdvertiseMode(AdvertiseSettings.ADVERTISE_MODE_LOW_LATENCY)
                .setConnectable(true)
                .setTimeout(0)
                .setTxPowerLevel(AdvertiseSettings.ADVERTISE_TX_POWER_HIGH)
                .build()

            val advertiseData = host.buildAdvertiseData()
            // Include scan response with service UUID for iOS compatibility.
            // iOS's CoreBluetooth actively queries for scan responses and has
            // known issues recognizing 128-bit service UUIDs from Android's
            // main advertisement packet format.
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
                    Log.e(TAG, "BLE advertising failed: $errorMsg (code=$errorCode)")
                    isAdvertising = false
                    // Release the in-flight gate synchronously. `startInFlight`
                    // is @Volatile so a subsequent start() on any thread
                    // observes the clear immediately — no main-handler hop
                    // required, which closes the retry window.
                    startInFlight = false
                    // Drop the stop-reference on main so a concurrent stop()
                    // doesn't race our clear. stop() is main-thread only, so
                    // posting here is sufficient.
                    mainHandler.post {
                        if (advertiseCallback === this) {
                            advertiseCallback = null
                        }
                    }
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
            advertiseCallback = null
            startInFlight = false
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
        val cb = advertiseCallback
        advertiseCallback = null
        startInFlight = false
        isAdvertising = false
        pendingAdvertiseRestart?.let {
            mainHandler.removeCallbacks(it)
            pendingAdvertiseRestart = null
        }
        if (cb == null) return
        try {
            advertiser?.stopAdvertising(cb)
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
        host.refreshPublishedIdentity()
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

    /** Call when [PeripheralGattServer] signals the service registration has completed. */
    fun onGattServerReady() {
        val reason = pendingAdvertiseReason ?: return
        pendingAdvertiseReason = null
        Log.i(TAG, "Starting deferred advertising after GATT service ready (reason=$reason)")
        // Caller is already on the main thread (the facade posts onReady
        // listener callbacks there), so start inline — no extra hop.
        start(reason)
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
        startInFlight = false
        advertiser = null
    }
}
