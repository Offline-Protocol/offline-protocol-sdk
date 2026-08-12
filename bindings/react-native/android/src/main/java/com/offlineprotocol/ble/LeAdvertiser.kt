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
    private val bleHandler: Handler,
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
         * Must be safe to invoke on the BLE thread and must not block on
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
        private const val ADVERTISE_RETRY_MIN_MS = 10_000L
        private const val ADVERTISE_RETRY_MAX_MS = 30_000L
        private const val ADVERTISE_RETRY_REASON = "advertise_retry"
    }

    private var advertiser: BluetoothLeAdvertiser? = null
    private var advertiseCallback: AdvertiseCallback? = null

    /**
     * The one path back to a live advertisement after the platform *accepts* a
     * start and then refuses it asynchronously at `onStartFailure`.
     *
     * Every other thing that revives advertising is a passenger on something
     * else: the facade's `bleRecoveryRunnable` repairs advertising only inside
     * an adapter-off episode (it re-reads the advertiser, replaces the GATT
     * server and restarts), and [refresh] runs only when the published identity
     * changes. A terminal failure while the adapter is on and the scan is
     * healthy trips none of them — it clears [isAdvertising] and the in-flight
     * gate and schedules nothing — so the device stays discoverable to nobody
     * for the rest of the process lifetime while the transport still reports
     * RUNNING. This runnable is the only thing that closes that.
     *
     * Deliberately a bare `start`: it needs no state check of its own because
     * [startInFlight] already refuses a start against a live or in-flight
     * advertisement, which is what makes a retry that races a recovery
     * elsewhere a no-op rather than a second `startAdvertising`.
     *
     * It does **not** re-acquire the platform advertiser. A null advertiser
     * means the adapter is down, and healing that — scanner, GATT server and
     * advertising together — belongs to the facade's adapter-off episode. The
     * retry lands on [start]'s null-advertiser bail and is inert, which is the
     * hand-over, not a gap.
     */
    private val advertiseRetryRunnable = Runnable { start(ADVERTISE_RETRY_REASON) }

    private val advertiseRetryScheduler = BleRecoveryScheduler(
        handler = bleHandler,
        task = advertiseRetryRunnable,
        minDelayMs = ADVERTISE_RETRY_MIN_MS,
        maxDelayMs = ADVERTISE_RETRY_MAX_MS,
    )

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
     *
     * Note it is deliberately *not* lowered by `onStartSuccess`: it guards a
     * live advertisement as well as an in-flight one, since a second
     * `startAdvertising` against a running one earns
     * `ADVERTISE_FAILED_ALREADY_STARTED`, whose handler nulls
     * [advertiseCallback] and so discards the only reference that could stop
     * the advertisement that is actually running.
     *
     * The consequence, which callers must own: an adapter switched off stops
     * advertising at the platform without delivering any callback, so the gate
     * stays raised over an advertisement that is already dead and every later
     * `start()` returns at it. Recovering from an adapter-off therefore means
     * calling [stop] first — re-attaching an advertiser and re-calling [start]
     * is silently a no-op. Pinned by `LeAdvertiserTest`.
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
        // where a fast stop() → start() on the BLE thread would submit a
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

        // Nothing to start against: getBluetoothLeAdvertiser() returns null
        // while the adapter is off, and the facade's adapter-reset path
        // re-attaches whatever it reads — so this is null exactly when the user
        // has Bluetooth switched off. Bail before raising the gate below: the
        // start call itself is a no-op on a null advertiser, so no callback
        // would ever arrive to lower it again, and every later start() would
        // return at the in-flight guard above with advertising wedged off for
        // good.
        //
        // Deliberately below the GATT-readiness check rather than above it. A
        // null advertiser must still latch [pendingAdvertiseReason], so an
        // attach that lands while the service registration is in flight is
        // picked up by [onGattServerReady] instead of being dropped silently.
        if (advertiser == null) {
            if (host.shouldLog("advertiser_unavailable", 60_000L)) {
                Log.i(TAG, "Deferring advertising — BLE advertiser unavailable (reason: $reason)")
                diagnosticEmitter(
                    "info",
                    "Deferring BLE advertising — advertiser unavailable",
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
                // Bind the callback identity explicitly rather than leaning on
                // `this` resolving through the posted lambdas below. Same shape
                // the facade's onScanFailed uses, and load-bearing in both
                // callbacks here: a result belonging to a callback that has
                // already been replaced or stopped must not disturb its
                // successor's state.
                private val self = this

                override fun onStartSuccess(settingsInEffect: AdvertiseSettings) {
                    Log.i(TAG, "BLE advertising started successfully (reason=$reason)")
                    isAdvertising = true
                    lastAdvertiseRestartAt = System.currentTimeMillis()
                    // A live advertisement retires any retry armed by an earlier
                    // failure, and puts the ladder back on its bottom rung so
                    // the next outage is retried fast rather than at the cap.
                    // Posted because this arrives on a private Binder thread
                    // while the scheduler's ladder is main-confined.
                    bleHandler.post {
                        if (advertiseCallback === self) {
                            advertiseRetryScheduler.cancel()
                        }
                    }
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
                    val willRetry = isRetriableFailure(errorCode)
                    // Drop the stop-reference on the BLE thread so a concurrent stop()
                    // doesn't race our clear. stop() is BLE-thread only, so
                    // posting here is sufficient.
                    bleHandler.post {
                        if (advertiseCallback === self) {
                            advertiseCallback = null
                            // Arm inside the identity check, not beside it. A
                            // failure from a callback already replaced by a
                            // newer start belongs to that start, and one whose
                            // reference stop() has already cleared must not be
                            // resurrected by a retry the app never asked for.
                            if (willRetry) {
                                advertiseRetryScheduler.schedule()
                            }
                        }
                    }
                    diagnosticEmitter(
                        "error",
                        "BLE advertising failed",
                        mapOf(
                            "errorCode" to errorCode,
                            "errorMessage" to errorMsg,
                            "willRetry" to willRetry,
                        ),
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
        } catch (e: IllegalStateException) {
            // The platform refuses both LE entry points while the adapter is
            // off by throwing IllegalStateException("BT Adapter is not turned
            // ON") — the advertiser exactly as the scanner does. This one is
            // reached from bare handler posts ([scheduleRestart], the facade's
            // adapter-reset path), where nothing above would catch it, so the
            // user switching Bluetooth off mid-session takes the host app down.
            //
            // Clearing the in-flight gate is the load-bearing half: it is
            // raised before the call and otherwise only lowered by stop() or a
            // terminal onStartFailure, neither of which runs when the call
            // throws — so leaving it raised would make every later start()
            // return early and wedge advertising off permanently.
            advertiseCallback = null
            startInFlight = false
            Log.i(TAG, "Skipping startAdvertising — BT adapter not on: ${e.message}")
            diagnosticEmitter(
                "info",
                "Skipping startAdvertising — BT adapter not on",
                mapOf("exception" to e.javaClass.simpleName, "message" to (e.message ?: "unknown")),
            )
        }
    }

    /**
     * Whether a terminal `onStartFailure` is worth another attempt.
     *
     * Codes and their meanings are from the platform
     * [AdvertiseCallback] contract:
     *
     *  - `TOO_MANY_ADVERTISERS` — "no advertising instance is available", i.e.
     *    every hardware slot is held, usually by other apps. Nothing about this
     *    device is broken and slots are released all the time, so this is the
     *    case the retry exists for.
     *  - `INTERNAL_ERROR` — an unattributed stack failure, routinely transient.
     *  - `ALREADY_STARTED` — an advertisement *is* running, so a retry earns
     *    the same code forever. (The handler above has already dropped the only
     *    reference that could stop it; that is a separate, pre-existing hazard
     *    documented on [startInFlight] and not something a retry can repair.)
     *  - `DATA_TOO_LARGE` — the payload is built by this SDK and does not vary
     *    between attempts, so this is a bug in what we advertise, not a
     *    condition that clears. It stays a loud error diagnostic instead.
     *  - `FEATURE_UNSUPPORTED` — hardware truth. Retrying only burns wakeups
     *    for the process lifetime; the scan path treats its own
     *    `FEATURE_UNSUPPORTED` as terminal for the same reason.
     *
     * Unknown codes retry: an unrecognized value is more likely a
     * vendor-specific transient than a new permanent class, and the ladder's
     * cap bounds the cost of being wrong.
     *
     * Note this deliberately reports nothing to the transport's availability
     * signal, even for the permanent codes. A device that cannot advertise but
     * can still scan keeps the central role — it discovers peers and connects
     * out — so BLE is degraded, not unusable, and saying otherwise would take
     * a working transport out of DORS.
     */
    private fun isRetriableFailure(errorCode: Int): Boolean = when (errorCode) {
        AdvertiseCallback.ADVERTISE_FAILED_TOO_MANY_ADVERTISERS,
        AdvertiseCallback.ADVERTISE_FAILED_INTERNAL_ERROR,
        -> true
        AdvertiseCallback.ADVERTISE_FAILED_ALREADY_STARTED,
        AdvertiseCallback.ADVERTISE_FAILED_DATA_TOO_LARGE,
        AdvertiseCallback.ADVERTISE_FAILED_FEATURE_UNSUPPORTED,
        -> false
        else -> true
    }

    fun stop() {
        val cb = advertiseCallback
        advertiseCallback = null
        startInFlight = false
        isAdvertising = false
        pendingAdvertiseRestart?.let {
            bleHandler.removeCallbacks(it)
            pendingAdvertiseRestart = null
        }
        // Cancel the failure retry *above* the early return below, not beside
        // the stopAdvertising call. A retry is armed precisely when a failure
        // has already nulled [advertiseCallback], so the `cb == null` return is
        // the common path out of a deliberate stop that follows one — leaving
        // the cancel below it would let a paused or stopped transport put
        // itself back on air. This is the single point every deliberate
        // teardown (stop, shutdown, refresh, the facade's adapter-off repair
        // and its BLE reset) routes through, which is what makes them all
        // authoritative over the retry without knowing it exists.
        advertiseRetryScheduler.cancel()
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
        } catch (e: IllegalStateException) {
            // Same adapter-off refusal as start(). State is already reset above
            // — the gate and the callback reference come down before the call —
            // so there is nothing to repair here; the throw just must not
            // escape, because refresh() reaches this from evict and
            // membership-change paths that run on bare handler posts.
            Log.i(TAG, "Skipping stopAdvertising — BT adapter not on: ${e.message}")
            diagnosticEmitter(
                "info",
                "Skipping stopAdvertising — BT adapter not on",
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
        pendingAdvertiseRestart?.let { bleHandler.removeCallbacks(it) }
        val runnable = Runnable {
            pendingAdvertiseRestart = null
            start(reason)
        }
        pendingAdvertiseRestart = runnable
        bleHandler.postDelayed(runnable, delay)
    }

    /** Call when [PeripheralGattServer] signals the service registration has completed. */
    fun onGattServerReady() {
        val reason = pendingAdvertiseReason ?: return
        pendingAdvertiseReason = null
        Log.i(TAG, "Starting deferred advertising after GATT service ready (reason=$reason)")
        // Caller is already on the BLE thread (the facade posts onReady
        // listener callbacks there), so start inline — no extra hop.
        start(reason)
    }

    fun clearPendingReason() {
        pendingAdvertiseReason = null
    }

    fun shutdown() {
        stop()
        pendingAdvertiseRestart?.let { bleHandler.removeCallbacks(it) }
        pendingAdvertiseRestart = null
        pendingAdvertiseReason = null
        lastAdvertiseRestartAt = 0L
        startInFlight = false
        advertiseRetryScheduler.cancel()
        advertiser = null
    }
}
