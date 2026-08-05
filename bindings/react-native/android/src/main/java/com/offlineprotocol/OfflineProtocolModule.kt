package com.offlineprotocol

import android.net.Uri
import android.util.Log
import com.facebook.react.bridge.*
import com.facebook.react.modules.core.DeviceEventManagerModule
import org.json.JSONArray
import org.json.JSONObject
import kotlin.math.max
import kotlin.math.min
import java.lang.ref.WeakReference
import java.util.concurrent.Executors
import java.util.concurrent.ScheduledExecutorService
import java.util.concurrent.TimeUnit

import com.offlineprotocol.ble.BleTransportFacade

// Import generated UniFFI bindings
import uniffi.offline_protocol.*

/**
 * UniFFI-based React Native module
 */
class OfflineProtocolModule(reactContext: ReactApplicationContext) :
    ReactContextBaseJavaModule(reactContext), LifecycleEventListener {

    /**
     * Gates the foreground relay reconnect on how long the app actually stayed
     * backgrounded, fed [android.os.SystemClock.elapsedRealtime] (monotonic AND
     * sleep-inclusive). Paired with iOS's ForegroundReconnectPolicy so both
     * platforms heal on foreground automatically and identically. Main-thread
     * confined: React Native delivers the host lifecycle callbacks there.
     */
    private val foregroundReconnectPolicy = ForegroundReconnectPolicy()

    init {
        // Drive the foreground relay-heal from the host activity's lifecycle so
        // Android matches iOS: both platforms reconnect automatically on
        // foreground after a background stay long enough to have killed the
        // socket. See onHostPause/onHostResume and ForegroundReconnectPolicy.
        // Registered after foregroundReconnectPolicy is initialized above so an
        // (in practice never synchronous) callback can't race its init.
        reactContext.addLifecycleEventListener(this)
    }

    private var protocol: OfflineProtocol? = null
    private var meshServices: MeshServices? = null
    private var bleTransport: BleTransportFacade? = null
    private var internetManager: InternetManager? = null
    private var wifiDirectManager: WifiDirectManager? = null
    private var reticulumManager: ReticulumManager? = null
    private var nostrManager: NostrManager? = null
    private var processScheduler: ScheduledExecutorService? = null
    private var listenerCount: Int = 0
    private var currentConfig: ProtocolConfig? = null
    private val mainHandler = android.os.Handler(android.os.Looper.getMainLooper())

    companion object {
        const val NAME = "OfflineProtocolModule"
        const val EVENT_NAME = "OfflineProtocol_Event"
        const val TELEMETRY_EVENT_NAME = "OfflineProtocol_Telemetry"

        /** How long `destroy` waits for an in-flight process tick to finish. */
        private const val PROCESS_SHUTDOWN_TIMEOUT_MS = 2_000L
    }
    
    private object Constants {
        const val MIN_BATTERY_LEVEL = 0
        const val MAX_BATTERY_LEVEL = 100
        const val MIN_HISTORY_WINDOW = 1L
        const val MAX_HISTORY_WINDOW = 100L
        const val BLE_RESTART_DELAY_MS = 1000L
        // Matches iOS 100ms tick. Handles retries, ACK timeouts, welcome
        // processing, and DORS. Latency-sensitive work is also event-driven.
        const val PROCESS_INTERVAL_MS = 100L
        const val MAX_RECEIVE_DRAIN_PER_TICK = 100
        const val LOG_INTERVAL_MS = 5000L
        const val LOG_INTERVAL_THRESHOLD_MS = 100L
        const val DEFAULT_RSSI_THRESHOLD: Short = -85
        const val DEFAULT_CONGESTION_QUEUE = 50L
        const val DEFAULT_STABILITY_WINDOW = 8L
        const val DEFAULT_QUEUE_RECOVERY_RATIO = 0.5f
        const val HTTPS_PORT = 443
        const val HTTP_PORT = 80
        const val MILLISECONDS_PER_SECOND = 1000L
    }

    override fun getName(): String = NAME

    override fun invalidate() {
        super.invalidate()
        reactApplicationContext.removeLifecycleEventListener(this)
        stopProcessScheduler()
        bleTransport?.stop()
        bleTransport = null
        internetManager?.stop()
        internetManager = null
        wifiDirectManager?.stop()
        wifiDirectManager = null
        reticulumManager?.stop()
        reticulumManager = null
        nostrManager?.stop()
        nostrManager = null
        protocol = null
        stopForegroundService()
        // Companion field capturing this module — drop it or the module and
        // its ReactContext outlive teardown.
        MeshForegroundService.onStopRequestedByUser = null
    }

    // MARK: - Host lifecycle (foreground relay heal)

    /**
     * App went to background. Record the moment so [onHostResume] can measure
     * the stay. A background long enough to have killed the relay TCP (Doze, OS
     * freeze, network handoff) leaves the cached ready flags stale-true against
     * a dead socket, which only a full reconnect heals.
     */
    override fun onHostPause() {
        foregroundReconnectPolicy.didEnterBackground(nowMs = android.os.SystemClock.elapsedRealtime())
    }

    /**
     * App returned to foreground. Proactively heal a socket the OS likely killed
     * while backgrounded: `isReady()` cannot tell a healthy socket from a zombie
     * (both report connected+authenticated), so gate on background duration
     * instead (see ForegroundReconnectPolicy). forceReconnect() no-ops unless
     * the transport is running/starting and resets the reconnect backoff, so it
     * is safe to call unconditionally within the gate. Mirrors iOS's
     * applicationWillEnterForeground — apps no longer need to call
     * forceInternetReconnect() on foreground themselves.
     */
    override fun onHostResume() {
        if (foregroundReconnectPolicy.shouldReconnectOnForeground(nowMs = android.os.SystemClock.elapsedRealtime())) {
            internetManager?.forceReconnect()
        }
    }

    override fun onHostDestroy() {
        // No-op: teardown is handled by invalidate(); nothing lifecycle-specific
        // to release here.
    }

    @ReactMethod
    fun addListener(eventName: String) {
        listenerCount += 1
    }

    @ReactMethod
    fun removeListeners(count: Double) {
        listenerCount = (listenerCount - count.toInt()).coerceAtLeast(0)
    }

    private fun normalizeRelayPriority(priority: String?): RelayPriority? {
        if (priority.isNullOrBlank()) {
            return null
        }
        return when (priority.lowercase()) {
            "low" -> RelayPriority.LOW
            "medium" -> RelayPriority.MEDIUM
            "high" -> RelayPriority.HIGH
            "never" -> RelayPriority.LOW
            "always" -> RelayPriority.HIGH
            "auto" -> RelayPriority.MEDIUM
            else -> null
        }
    }

    private fun applyInitialRuntimeConfig(proto: OfflineProtocol, json: JSONObject) {
        json.optJSONObject("dors")?.let { dorsJson ->
            try {
                val baseConfig = proto.getDorsConfig()
                val updatedConfig = baseConfig.copy(
                    preferOnline = dorsJson.optBooleanCompat("preferOnline", "prefer_online")
                        ?: baseConfig.preferOnline,
                    switchHysteresis = dorsJson.optDoubleCompat("switchHysteresis", "switch_hysteresis")
                        ?.toFloat()
                        ?.coerceAtLeast(0f)
                        ?: baseConfig.switchHysteresis,
                    switchCooldownSecs = dorsJson.optLongCompat("switchCooldownSecs", "switch_cooldown_secs")
                        ?.coerceAtLeast(0)
                        ?.toULong()
                        ?: baseConfig.switchCooldownSecs,
                    bleToWifiRetryThreshold = dorsJson.optIntCompat("bleToWifiRetryThreshold", "ble_to_wifi_retry_threshold")
                        ?.coerceAtLeast(0)
                        ?.toUInt()
                        ?: baseConfig.bleToWifiRetryThreshold,
                    minSuccessRateBeforeEscalation = dorsJson.optDoubleCompat("minSuccessRateBeforeEscalation", "min_success_rate_before_escalation")
                        ?.toFloat()
                        ?.coerceIn(0f, 1f)
                        ?: baseConfig.minSuccessRateBeforeEscalation,
                    minBleSamplesBeforeSuccessRateEscalation = dorsJson.optLongCompat("minBleSamplesBeforeSuccessRateEscalation", "min_ble_samples_before_success_rate_escalation")
                        ?.coerceAtLeast(0)
                        ?.toULong()
                        ?: baseConfig.minBleSamplesBeforeSuccessRateEscalation,
                    rssiSwitchThreshold = dorsJson.optIntCompat("rssiSwitchThreshold", "rssi_switch_threshold")
                        ?.coerceIn(Short.MIN_VALUE.toInt(), Short.MAX_VALUE.toInt())
                        ?.toShort()
                        ?: baseConfig.rssiSwitchThreshold,
                    congestionQueueThreshold = dorsJson.optLongCompat("congestionQueueThreshold", "congestion_queue_threshold")
                        ?.coerceAtLeast(0)
                        ?.toULong()
                        ?: baseConfig.congestionQueueThreshold,
                    stabilityWindowSecs = dorsJson.optLongCompat("stabilityWindowSecs", "stability_window_secs")
                        ?.coerceAtLeast(0)
                        ?.toULong()
                        ?: baseConfig.stabilityWindowSecs,
                    poorSignalDurationSecs = dorsJson.optLongCompat("poorSignalDurationSecs", "poor_signal_duration_secs")
                        ?.coerceAtLeast(0)
                        ?.toULong()
                        ?: baseConfig.poorSignalDurationSecs,
                    ttlEscalationThreshold = dorsJson.optIntCompat("ttlEscalationThreshold", "ttl_escalation_threshold")
                        ?.coerceIn(0, UByte.MAX_VALUE.toInt())
                        ?.toUByte()
                        ?: baseConfig.ttlEscalationThreshold,
                    congestionDurationSecs = dorsJson.optLongCompat("congestionDurationSecs", "congestion_duration_secs")
                        ?.coerceAtLeast(0)
                        ?.toULong()
                        ?: baseConfig.congestionDurationSecs,
                    ttlEscalationHoldSecs = dorsJson.optLongCompat("ttlEscalationHoldSecs", "ttl_escalation_hold_secs")
                        ?.coerceAtLeast(1)
                        ?.toULong()
                        ?: baseConfig.ttlEscalationHoldSecs,
                    historyWindowSize = dorsJson.optLongCompat("historyWindowSize", "history_window_size")
                        ?.let { max(Constants.MIN_HISTORY_WINDOW, min(Constants.MAX_HISTORY_WINDOW, it)) }
                        ?.toULong()
                        ?: baseConfig.historyWindowSize,
                    queueRecoveryRatio = dorsJson.optDoubleCompat("queueRecoveryRatio", "queue_recovery_ratio")
                        ?.toFloat()
                        ?.coerceIn(0f, 1f)
                        ?: baseConfig.queueRecoveryRatio
                )
                proto.updateDorsConfig(updatedConfig)
                emitDiagnostic("info", "Applied initial DORS config")
            } catch (e: Exception) {
                emitDiagnostic("warning", "Failed to apply initial DORS config", mapOf(
                    "message" to (e.message ?: "unknown")
                ))
            }
        }

        json.optJSONObject("relay")?.let { relayJson ->
            val priorityRaw = relayJson.safeOptString("relayPriority", relayJson.safeOptString("relay_priority"))
            val priority = normalizeRelayPriority(priorityRaw)
            if (priority != null) {
                try {
                    proto.setRelayPriority(priority)
                    emitDiagnostic("info", "Applied initial relay priority", mapOf("priority" to priority.name.lowercase()))
                } catch (e: Exception) {
                    emitDiagnostic("warning", "Failed to apply initial relay priority", mapOf(
                        "message" to (e.message ?: "unknown")
                    ))
                }
            }
        }
    }

    /**
     * Rejects [promise] with the stable typed code when the error maps
     * (see ProtocolErrorBridge.kt), otherwise with the method's legacy
     * fallback code and "$fallbackMessage: <cause>".
     */
    private fun rejectWithProtocolError(
        promise: Promise,
        error: Throwable,
        fallbackCode: String,
        fallbackMessage: String
    ) {
        val mapped = mapProtocolBridgeError(error)
        if (mapped != null) {
            promise.reject(mapped.code, mapped.message, error)
        } else {
            promise.reject(fallbackCode, "$fallbackMessage: ${error.message}", error)
        }
    }

    @ReactMethod
    fun create(configJson: String, promise: Promise) {
        try {
            val parsed = ProtocolConfigParser.parse(configJson)
            val config = parsed.coreConfig
            val proto = OfflineProtocol(config)
            currentConfig = config
            emitDiagnostic("info", "Protocol core created", mapOf(
                "appId" to config.appId,
                "userId" to config.userId,
                "bleEnabled" to config.bleEnabled,
                "wifiDirectEnabled" to config.wifiDirectEnabled,
                "internetEnabled" to config.internetEnabled,
                "reticulumEnabled" to config.reticulumEnabled
            ))
            
            // Set up event callback
            proto.setEventCallback(object : EventCallback {
                override fun onEvent(eventJson: String) {
                    val params = Arguments.createMap().apply {
                        putString("eventJson", eventJson)
                    }
                    sendEvent(EVENT_NAME, params)
                }
            })

            applyInitialRuntimeConfig(proto, parsed.rawJson)

            protocol = proto
            meshServices = MeshServices(proto)

            // Initialize BLE manager if BLE is enabled
            if (config.bleEnabled) {
                bleTransport = BleTransportFacade(
                    reactApplicationContext,
                    proto,
                    config.userId,
                ) { level, message, context ->
                    emitDiagnostic(level, message, context)
                }.also { manager ->
                    manager.listener = object : TransportManagerListener {
                        override fun onTransportStateChanged(manager: TransportManager, state: TransportState) {
                            emitDiagnostic("info", "BLE transport state changed", mapOf(
                                "state" to state.name.lowercase()
                            ))
                        }

                        override fun onTransportError(manager: TransportManager, error: Throwable) {
                            emitDiagnostic("error", "BLE transport error", mapOf(
                                "message" to (error.message ?: "unknown"),
                                "exception" to error.javaClass.simpleName
                            ))
                        }

                        override fun onTransportMetricsUpdated(manager: TransportManager, metrics: Map<String, Any>) {
                            emitDiagnostic("info", "BLE transport metrics", metrics.mapValues { it.value })
                        }

                        override fun onTransportDiagnostic(
                            manager: TransportManager,
                            level: String,
                            message: String,
                            context: Map<String, Any?>
                        ) {
                            emitDiagnostic(level, message, context)
                        }
                    }
                }
                android.util.Log.i(NAME, "BLE Manager initialized for user: ${config.userId}")
                emitDiagnostic("info", "BLE manager initialized", mapOf(
                    "userId" to config.userId
                ))
            } else {
                emitDiagnostic("warning", "BLE disabled in configuration", mapOf(
                    "userId" to config.userId
                ))
            }
            
            // Initialize Internet manager if internet is enabled
            if (config.internetEnabled) {
                internetManager = InternetManager(reactApplicationContext, proto, config.userId) { level, message, context ->
                    emitDiagnostic(level, message, context)
                }.also { manager ->
                    manager.serverMessageEmitter = { rawJson -> emitServerMessageEvent(rawJson) }
                    manager.connectionStatusEmitter = { connected, authenticated ->
                        emitInternetStatusEvent(connected, authenticated)
                    }
                    manager.supersededEmitter = { reason -> emitInternetSupersededEvent(reason) }
                    manager.listener = object : TransportManagerListener {
                        override fun onTransportStateChanged(manager: TransportManager, state: TransportState) {
                            emitDiagnostic("info", "Internet transport state changed", mapOf(
                                "transport" to manager.transportId,
                                "state" to state.name.lowercase()
                            ))
                        }

                        override fun onTransportError(manager: TransportManager, error: Throwable) {
                            emitDiagnostic("error", "Internet transport error", mapOf(
                                "transport" to manager.transportId,
                                "message" to (error.message ?: "unknown"),
                                "exception" to error.javaClass.simpleName
                            ))
                        }

                        override fun onTransportMetricsUpdated(manager: TransportManager, metrics: Map<String, Any>) {
                            val enrichedMetrics = metrics.toMutableMap()
                            enrichedMetrics["transport"] = manager.transportId
                            emitDiagnostic("info", "Internet transport metrics", enrichedMetrics)
                        }

                        override fun onTransportDiagnostic(
                            manager: TransportManager,
                            level: String,
                            message: String,
                            context: Map<String, Any?>
                        ) {
                            val enrichedContext = context.toMutableMap()
                            enrichedContext["transport"] = manager.transportId
                            emitDiagnostic(level, message, enrichedContext)
                        }
                    }
                }
                android.util.Log.i(NAME, "Internet Manager initialized for user: ${config.userId}")
                emitDiagnostic("info", "Internet manager initialized", mapOf(
                    "userId" to config.userId
                ))
            } else {
                emitDiagnostic("info", "Internet disabled in configuration", mapOf(
                    "userId" to config.userId
                ))
            }

            // Initialize Reticulum manager if reticulum is enabled
            if (config.reticulumEnabled) {
                reticulumManager = createReticulumManager(proto, config.userId)
                android.util.Log.i(NAME, "Reticulum Manager initialized for user: ${config.userId}")
                emitDiagnostic("info", "Reticulum manager initialized", mapOf(
                    "userId" to config.userId
                ))
            } else {
                emitDiagnostic("info", "Reticulum disabled in configuration", mapOf(
                    "userId" to config.userId
                ))
            }

            // Initialize Nostr manager if nostr is enabled
            if (config.nostrEnabled) {
                nostrManager = createNostrManager(proto, config.userId)
                android.util.Log.i(NAME, "Nostr Manager initialized for user: ${config.userId}")
                emitDiagnostic("info", "Nostr manager initialized", mapOf(
                    "userId" to config.userId
                ))
            } else {
                emitDiagnostic("info", "Nostr disabled in configuration", mapOf(
                    "userId" to config.userId
                ))
            }

            // Wire Rust transport callbacks for event-driven sending.
            // These replace the 100ms polling loops; the Rust core calls back into
            // Kotlin when outgoing data is enqueued, and the manager drains the queue
            // immediately. Requires regenerated UniFFI Android bindings that include
            // BleTransportCallback / WifiDirectTransportCallback interfaces.
            wireTransportCallbacks(proto)

            // Start process scheduler
            startProcessScheduler()
            emitDiagnostic("info", "Protocol process scheduler started")
            
            promise.resolve(null)
        } catch (e: Exception) {
            emitDiagnostic("error", "Failed to create protocol", mapOf(
                "message" to (e.message ?: "unknown"),
                "exception" to e.javaClass.simpleName
            ))
            promise.reject("ERROR_CREATE", "Failed to create protocol: ${e.message}", e)
        }
    }
    
    /**
     * Send event to JavaScript
     */
    private fun sendEvent(eventName: String, params: Any?) {
        if (listenerCount > 0) {
            reactApplicationContext
                .getJSModule(DeviceEventManagerModule.RCTDeviceEventEmitter::class.java)
                .emit(eventName, params)
        }
    }

    private fun emitDiagnostic(level: String, message: String, context: Map<String, Any?> = emptyMap()) {
        try {
            val json = JSONObject()
            json.put("type", "diagnostic")
            json.put("level", level)
            json.put("message", message)

            if (context.isNotEmpty()) {
                val contextJson = JSONObject()
                context.forEach { (key, value) ->
                    when (value) {
                        null -> contextJson.put(key, JSONObject.NULL)
                        else -> contextJson.put(key, value)
                    }
                }
                json.put("context", contextJson)
            }

            val params = Arguments.createMap().apply {
                putString("eventJson", json.toString())
            }
            sendEvent(EVENT_NAME, params)
        } catch (e: Exception) {
            android.util.Log.e(NAME, "Failed to emit diagnostic event", e)
        }
    }

    /**
     * Forwards an internet-transport readiness transition as the
     * `internet_status_changed` event. `authenticated: true` is the
     * positive gate for raw server commands (replaces app-side
     * `relayStatus === 'authenticated'` tracking).
     */
    private fun emitInternetStatusEvent(connected: Boolean, authenticated: Boolean) {
        try {
            val json = JSONObject()
            json.put("type", "internet_status_changed")
            json.put("connected", connected)
            json.put("authenticated", authenticated)
            val params = Arguments.createMap().apply {
                putString("eventJson", json.toString())
            }
            sendEvent(EVENT_NAME, params)
        } catch (e: Exception) {
            android.util.Log.e(NAME, "Failed to emit internet status event", e)
        }
    }

    /**
     * Forwards a relay session displacement ("superseded") as the
     * `internet_session_superseded` event. The relay closed this socket with
     * code 4000 because a newer registration for the same identity took over;
     * the SDK will NOT auto-reconnect. The app surfaces "connected elsewhere"
     * and reconnects only on explicit user action (re-enabling the transport).
     */
    private fun emitInternetSupersededEvent(reason: String?) {
        try {
            val json = JSONObject()
            json.put("type", "internet_session_superseded")
            if (reason != null) json.put("reason", reason)
            val params = Arguments.createMap().apply {
                putString("eventJson", json.toString())
            }
            sendEvent(EVENT_NAME, params)
        } catch (e: Exception) {
            android.util.Log.e(NAME, "Failed to emit internet superseded event", e)
        }
    }

    /**
     * Forwards a raw relay frame apps need outside or in addition to
     * SDK-owned processing (group snapshot extensions, invite links, role
     * changes, rate limiting, unknown types) as the
     * `internet_server_message` event.
     */
    private fun emitServerMessageEvent(rawJson: String) {
        try {
            val json = JSONObject()
            json.put("type", "internet_server_message")
            json.put("json", rawJson)
            val params = Arguments.createMap().apply {
                putString("eventJson", json.toString())
            }
            sendEvent(EVENT_NAME, params)
        } catch (e: Exception) {
            android.util.Log.e(NAME, "Failed to emit server message event", e)
        }
    }

    @ReactMethod
    fun start(promise: Promise) {
        try {
            emitDiagnostic("info", "Starting protocol")
            protocol?.start()
            emitDiagnostic("info", "Protocol core started")

            // Start foreground service to protect the process from being killed
            startForegroundService()
            
            //  Start BLE manager if available - BLE should work independently
            // BLE peer discovery and messaging must work even when Internet/WiFi are disabled
            bleTransport?.let { manager ->
                try {
                    android.util.Log.i(NAME, "Starting BLE manager (BLE should work independently of other transports)...")
                    emitDiagnostic("info", "Starting BLE manager", mapOf(
                        "internetEnabled" to (currentConfig?.internetEnabled ?: false),
                        "wifiDirectEnabled" to (currentConfig?.wifiDirectEnabled ?: false)
                    ))
                    manager.start()
                    android.util.Log.i(NAME, "BLE Manager started successfully - scanning and advertising should be active")
                    emitDiagnostic("info", "BLE manager started - peer discovery active", mapOf(
                        "scanning" to true,
                        "advertising" to true
                    ))
                    
                    // bleStatusChanged(true) is called inside BleTransportFacade.start()
                    // after advertising and scanning are both active — no backup timer needed.
                } catch (e: Exception) {
                    android.util.Log.e(NAME, "FAILED to start BLE Manager!", e)
                    android.util.Log.e(NAME, "Error type: ${e.javaClass.simpleName}")
                    android.util.Log.e(NAME, "Error message: ${e.message}")
                    android.util.Log.e(NAME, "Stack trace: ", e)
                    emitDiagnostic("error", "Failed to start BLE manager", mapOf(
                        "message" to (e.message ?: "unknown"),
                        "exception" to e.javaClass.simpleName,
                        "stackTrace" to e.stackTraceToString()
                    ))
                    // Don't fail the entire start if BLE fails, but log the error clearly
                    android.util.Log.w(NAME, "Protocol will continue without BLE, but peer discovery and BLE messaging will not work")
                }
            } ?: run {
                android.util.Log.w(NAME, "BLE manager is null - BLE was not initialized. Check if bleEnabled=true in config.")
                emitDiagnostic("warning", "BLE manager is null - BLE not initialized", mapOf(
                    "bleEnabled" to (currentConfig?.bleEnabled ?: false)
                ))
            }
            
            promise.resolve(null)
        } catch (e: Exception) {
            emitDiagnostic("error", "Failed to start protocol", mapOf(
                "message" to (e.message ?: "unknown"),
                "exception" to e.javaClass.simpleName
            ))
            promise.reject("ERROR_START", "Failed to start protocol: ${e.message}", e)
        }
    }
    
    @ReactMethod
    fun emitTestEvent(promise: Promise) {
        try {
            protocol?.emitTestEvent()
            promise.resolve(null)
        } catch (e: Exception) {
            promise.reject("ERROR_TEST_EVENT", "Failed to emit test event: ${e.message}", e)
        }
    }

    @ReactMethod
    fun installTelemetrySink(configMap: ReadableMap?, promise: Promise) {
        val proto = protocol
        if (proto == null) {
            promise.reject("NOT_STARTED", "Protocol not created", null)
            return
        }
        try {
            val cfg = parseTelemetryConfig(configMap)
            proto.installTelemetrySink(TelemetrySinkImpl(this), cfg)
            promise.resolve(null)
        } catch (e: Exception) {
            promise.reject("TELEMETRY_INSTALL", "Failed to install telemetry sink: ${e.message}", e)
        }
    }

    @ReactMethod
    fun pollTelemetryFrame(promise: Promise) {
        val proto = protocol
        if (proto == null) {
            promise.reject("NOT_STARTED", "Protocol not created", null)
            return
        }
        try {
            promise.resolve(proto.pollTelemetryFrame())
        } catch (e: Exception) {
            promise.reject("TELEMETRY_POLL", "Failed to poll telemetry frame: ${e.message}", e)
        }
    }

    @ReactMethod
    fun uninstallTelemetrySink(promise: Promise) {
        val proto = protocol
        if (proto == null) {
            promise.reject("NOT_STARTED", "Protocol not created", null)
            return
        }
        try {
            proto.uninstallTelemetrySink()
            promise.resolve(null)
        } catch (e: Exception) {
            promise.reject("TELEMETRY_UNINSTALL", "Failed to uninstall telemetry sink: ${e.message}", e)
        }
    }

    @ReactMethod
    fun telemetryInstallId(promise: Promise) {
        val proto = protocol
        if (proto == null) {
            promise.reject("NOT_STARTED", "Protocol not created", null)
            return
        }
        try {
            promise.resolve(proto.telemetryInstallId())
        } catch (e: Exception) {
            promise.reject("TELEMETRY_INSTALL_ID", "Failed to read telemetry install id: ${e.message}", e)
        }
    }

    // The TS `TelemetryConfig` type (bindings/react-native/src/types.ts)
    // only emits camelCase keys — these parsers match that contract.
    //
    // Unrecognised `mlsVerbosity` strings fall back to null (Rust default)
    // with a `Log.w` — silent fallback would have hid integrator typos
    // behind a config that "just works" at Lifecycle.
    private fun parseTelemetryConfig(map: ReadableMap?): TelemetryConfig {
        if (map == null) {
            return TelemetryConfig(
                scrubIds = null,
                mlsVerbosity = null,
                metricsCadenceMs = null,
                routingDiagnostic = null,
                enablePollQueue = null,
                mlsSamplingBypass = null,
            )
        }
        val scrubIds = readOptionalBoolean(map, "scrubIds")
        val verbosity = readOptionalString(map, "mlsVerbosity")?.let { raw ->
            when (raw.lowercase()) {
                "off" -> MlsVerbosity.OFF
                "diagnostic" -> MlsVerbosity.DIAGNOSTIC
                "lifecycle" -> MlsVerbosity.LIFECYCLE
                else -> {
                    Log.w(
                        NAME,
                        "telemetry: unknown mlsVerbosity '$raw' — expected 'off', 'lifecycle', or 'diagnostic'. Falling back to the Rust default (lifecycle)."
                    )
                    null
                }
            }
        }
        val cadence = readOptionalNonNegativeLong(map, "metricsCadenceMs")?.toULong()
        val routingDiag = readOptionalBoolean(map, "routingDiagnostic")
        val enablePollQueue = readOptionalBoolean(map, "enablePollQueue")
        val mlsSamplingBypass = readOptionalBoolean(map, "mlsSamplingBypass")
        return TelemetryConfig(
            scrubIds = scrubIds,
            mlsVerbosity = verbosity,
            metricsCadenceMs = cadence,
            routingDiagnostic = routingDiag,
            enablePollQueue = enablePollQueue,
            mlsSamplingBypass = mlsSamplingBypass,
        )
    }

    private fun readOptionalBoolean(map: ReadableMap, key: String): Boolean? =
        if (map.hasKey(key) && !map.isNull(key)) map.getBoolean(key) else null

    private fun readOptionalString(map: ReadableMap, key: String): String? =
        if (map.hasKey(key) && !map.isNull(key)) map.getString(key) else null

    // Non-negative u64-compatible parse: JS numbers are f64, so values above
    // 2^53 lose precision — fine for config-sized fields (cadences in ms
    // fit comfortably). Negative values would wrap to a huge u64 after
    // `.toULong()`, so we explicitly warn-and-reject (returning null keeps
    // the Rust-side default). Do NOT reuse this helper for counter fields
    // that can exceed 2^53.
    private fun readOptionalNonNegativeLong(map: ReadableMap, key: String): Long? {
        if (!map.hasKey(key) || map.isNull(key)) {
            return null
        }
        val value = map.getDouble(key).toLong()
        if (value < 0L) {
            Log.w(
                NAME,
                "telemetry: ignoring negative value for '$key' ($value) — expected a non-negative number. Falling back to the Rust default."
            )
            return null
        }
        return value
    }

    /**
     * Forwards each typed TelemetryRecord variant into the RN bridge as a
     * discriminated body keyed by `category`. Mirrors the iOS
     * `TelemetrySinkImpl`. Must not block, reenter the SDK, or panic.
     *
     * Deliberately a nested (non-`inner`) class holding a
     * `WeakReference<OfflineProtocolModule>` — if this were an inner class
     * it would pin the enclosing module alive for as long as the Rust
     * adapter kept the sink, defeating React Native's ability to GC a
     * detached module (e.g. after a reload).
     */
    private class TelemetrySinkImpl(module: OfflineProtocolModule) : TelemetrySink {
        private val moduleRef = WeakReference(module)

        // Every callback is invoked synchronously from the Rust emit path.
        // A thrown Kotlin exception here would cross the UniFFI boundary
        // and crash the native caller, so we log-and-swallow anything the
        // encoders or React bridge throw (e.g. detached ReactContext during
        // app tear-down). If the weak reference has been GC'd we drop the
        // record silently — the Rust side will eventually replace or free
        // this sink.
        private inline fun safeDispatch(build: (OfflineProtocolModule) -> WritableMap) {
            val module = moduleRef.get() ?: return
            try {
                val params = build(module)
                module.sendEvent(TELEMETRY_EVENT_NAME, params)
            } catch (t: Throwable) {
                Log.w(NAME, "telemetry dispatch failed; dropping record", t)
            }
        }

        override fun onProtocolEvent(eventJson: String) = safeDispatch { _ ->
            Arguments.createMap().apply {
                putString("category", "protocol")
                putString("eventJson", eventJson)
            }
        }

        override fun onMlsEvent(eventJson: String) = safeDispatch { _ ->
            Arguments.createMap().apply {
                putString("category", "mls")
                putString("eventJson", eventJson)
            }
        }

        override fun onMetricsFrame(frame: MetricsFrame) = safeDispatch { m ->
            Arguments.createMap().apply {
                putString("category", "metricsFrame")
                putMap("frame", m.encodeFrame(frame))
            }
        }

        override fun onTransportState(event: TransportStateEvent) = safeDispatch { m ->
            Arguments.createMap().apply {
                putString("category", "transportState")
                putMap("event", m.encodeTransportState(event))
            }
        }

        override fun onRoutingDecision(decision: RoutingDecision) = safeDispatch { m ->
            Arguments.createMap().apply {
                putString("category", "routingDecision")
                putMap("decision", m.encodeRouting(decision))
            }
        }

        override fun onDeviceCapability(snapshot: DeviceCapabilitySnapshot) = safeDispatch { m ->
            Arguments.createMap().apply {
                putString("category", "deviceCapability")
                putMap("snapshot", m.encodeDevice(snapshot))
            }
        }

        override fun onExtension(name: String, payloadJson: String) = safeDispatch { _ ->
            Arguments.createMap().apply {
                putString("category", "extension")
                putString("name", name)
                putString("payloadJson", payloadJson)
            }
        }
    }

    // ---- Telemetry encoders (UniFFI data classes -> WritableMap) ----
    //
    // IMPORTANT: every map produced below MUST be structurally identical
    // to the JSON envelope the Rust adapter enqueues on the pull channel.
    // The canonical contract is pinned by the `shape_parity_*_envelope`
    // tests in `crates/offline-protocol-uniffi/src/lib.rs`. If those tests
    // change, update the matching encoder here in lockstep — the TS
    // `TelemetryRecord` discriminated union expects ONE shape regardless
    // of whether a record arrived via `onTelemetry` (push) or
    // `pollTelemetry` (pull).

    private fun encodeTransport(t: TransportType): String = when (t) {
        TransportType.INTERNET -> "internet"
        TransportType.BLE -> "ble"
        TransportType.WI_FI_DIRECT -> "wifiDirect"
        TransportType.RETICULUM -> "reticulum"
        TransportType.NOSTR -> "nostr"
    }

    private fun encodeStatus(s: TransportStatus): String = when (s) {
        TransportStatus.AVAILABLE -> "available"
        TransportStatus.UNAVAILABLE -> "unavailable"
        TransportStatus.CONNECTING -> "connecting"
        TransportStatus.DISCONNECTED -> "disconnected"
        TransportStatus.ERROR -> "error"
    }

    private fun encodePhase(p: RoutingPhase): String = when (p) {
        RoutingPhase.SCORE_UPDATED -> "scoreUpdated"
        RoutingPhase.SELECTED -> "selected"
        RoutingPhase.SWITCHED -> "switched"
        RoutingPhase.ESCALATED -> "escalated"
        RoutingPhase.UNKNOWN -> "unknown"
    }

    private fun encodeReason(r: RoutingReasonCode): String = when (r) {
        RoutingReasonCode.INITIAL_SELECTION -> "initialSelection"
        RoutingReasonCode.PRIMARY_SELECTED -> "primarySelected"
        RoutingReasonCode.PRIMARY_SUCCESS -> "primarySuccess"
        RoutingReasonCode.FALLBACK_SUCCESS -> "fallbackSuccess"
        RoutingReasonCode.ESCALATION_APPLIED -> "escalationApplied"
        RoutingReasonCode.CURRENT_UNAVAILABLE -> "currentUnavailable"
        RoutingReasonCode.RETRY_THRESHOLD -> "retryThreshold"
        RoutingReasonCode.POOR_SIGNAL -> "poorSignal"
        RoutingReasonCode.CONGESTION -> "congestion"
        RoutingReasonCode.LOW_TTL -> "lowTtl"
        RoutingReasonCode.LOW_SUCCESS_RATE -> "lowSuccessRate"
        RoutingReasonCode.UNKNOWN -> "unknown"
    }

    private fun encodeMetrics(m: TransportMetrics): WritableMap = Arguments.createMap().apply {
        // Counter fields (UInt/UInt32/UInt64 on the Rust side) go through
        // putDouble to avoid Kotlin's signed-Int wrap at 2^31 — long-running
        // relays accumulate byte/packet counts well past that boundary.
        putDouble("packetsSent", m.packetsSent.toLong().toDouble())
        putDouble("packetsReceived", m.packetsReceived.toLong().toDouble())
        putDouble("bytesSent", m.bytesSent.toLong().toDouble())
        putDouble("bytesReceived", m.bytesReceived.toLong().toDouble())
        putDouble("errorRate", m.errorRate.toDouble())
        putDouble("avgLatencyMs", m.avgLatencyMs.toLong().toDouble())
        m.rssi?.let { putInt("rssi", it.toInt()) }
        m.bandwidthBps?.let { putDouble("bandwidthBps", it.toLong().toDouble()) }
        m.congestion?.let { putDouble("congestion", it.toDouble()) }
        m.queueDepth?.let { putDouble("queueDepth", it.toLong().toDouble()) }
        m.batteryLevel?.let { putInt("batteryLevel", it.toInt()) }
        m.isCharging?.let { putBoolean("isCharging", it) }
        m.relayConnectionCount?.let { putInt("relayConnectionCount", it.toInt()) }
        m.isActiveRelay?.let { putBoolean("isActiveRelay", it) }
        m.deliveryRatio?.let { putDouble("deliveryRatio", it.toDouble()) }
        m.dropRate?.let { putDouble("dropRate", it.toDouble()) }
        m.averageHopCount?.let { putDouble("averageHopCount", it.toDouble()) }
        m.energyCost?.let { putDouble("energyCost", it.toDouble()) }
    }

    private fun encodeFrame(f: MetricsFrame): WritableMap = Arguments.createMap().apply {
        putDouble("timestampMs", f.timestampMs.toDouble())
        val transports = Arguments.createArray()
        for (entry in f.transports) {
            transports.pushMap(
                Arguments.createMap().apply {
                    putString("transport", encodeTransport(entry.transport))
                    putMap("metrics", encodeMetrics(entry.metrics))
                }
            )
        }
        putArray("transports", transports)
        putMap(
            "retryQueue",
            Arguments.createMap().apply {
                putDouble("totalCount", f.retryQueue.totalCount.toDouble())
                putDouble("readyCount", f.retryQueue.readyCount.toDouble())
                putDouble("criticalPriorityCount", f.retryQueue.criticalPriorityCount.toDouble())
                putDouble("highPriorityCount", f.retryQueue.highPriorityCount.toDouble())
                putDouble("mediumPriorityCount", f.retryQueue.mediumPriorityCount.toDouble())
                putDouble("lowPriorityCount", f.retryQueue.lowPriorityCount.toDouble())
            }
        )
        putMap(
            "dedup",
            Arguments.createMap().apply {
                putDouble("totalTracked", f.dedup.totalTracked.toDouble())
                putDouble("recentTracked", f.dedup.recentTracked.toDouble())
                putInt("capacityUsedPercent", f.dedup.capacityUsedPercent.toInt())
                f.dedup.falsePositiveRate?.let { putDouble("falsePositiveRate", it) }
                putString("mode", f.dedup.mode)
            }
        )
        putDouble("ackPending", f.ackPending.toDouble())
        putDouble("neighborCount", f.neighborCount.toDouble())
        putBoolean("isLocalRelay", f.isLocalRelay)
        f.currentTransport?.let { putString("currentTransport", encodeTransport(it)) }
    }

    private fun encodeTransportState(e: TransportStateEvent): WritableMap = Arguments.createMap().apply {
        putDouble("timestampMs", e.timestampMs.toDouble())
        putString("transport", encodeTransport(e.transport))
        putString("previous", encodeStatus(e.previous))
        putString("current", encodeStatus(e.current))
    }

    private fun encodeRouting(d: RoutingDecision): WritableMap = Arguments.createMap().apply {
        putDouble("timestampMs", d.timestampMs.toDouble())
        putString("phase", encodePhase(d.phase))
        d.from?.let { putString("from", encodeTransport(it)) }
        d.to?.let { putString("to", encodeTransport(it)) }
        d.winningScore?.let { putDouble("winningScore", it.toDouble()) }
        d.reasonCode?.let { putString("reasonCode", encodeReason(it)) }
        val scores = Arguments.createArray()
        for (s in d.scores) {
            scores.pushMap(
                Arguments.createMap().apply {
                    putString("transport", encodeTransport(s.transport))
                    putDouble("signal", s.signal.toDouble())
                    putDouble("proximity", s.proximity.toDouble())
                    putDouble("bandwidth", s.bandwidth.toDouble())
                    putDouble("congestion", s.congestion.toDouble())
                    putDouble("energy", s.energy.toDouble())
                    putDouble("reliability", s.reliability.toDouble())
                    putDouble("load", s.load.toDouble())
                    putDouble("total", s.total.toDouble())
                }
            )
        }
        putArray("scores", scores)
    }

    private fun encodeDevice(s: DeviceCapabilitySnapshot): WritableMap = Arguments.createMap().apply {
        putDouble("timestampMs", s.timestampMs.toDouble())
        s.batteryLevel?.let { putInt("batteryLevel", it.toInt()) }
        putBoolean("isCharging", s.isCharging)
        putString("relayRole", if (s.relayRole == RelayRole.RELAY) "relay" else "regular")
        putInt("changedFields", s.changedFields.toInt())
    }

    @ReactMethod
    fun stop(promise: Promise) {
        try {
            stopTransportsAndProtocol()
            promise.resolve(null)
        } catch (e: Exception) {
            emitDiagnostic("error", "Failed to stop protocol", mapOf(
                "message" to (e.message ?: "unknown"),
                "exception" to e.javaClass.simpleName
            ))
            promise.reject("ERROR_STOP", "Failed to stop protocol: ${e.message}", e)
        }
    }

    /**
     * Stop every transport, the process scheduler, the keep-alive service and
     * the protocol core. Shared by the JS-facing [stop] and the mesh
     * notification's Stop action so both tear down identically: the foreground
     * service is only a keep-alive, and dropping it on its own would leave the
     * transports and the scheduler running with no process protection.
     *
     * Synchronized because those two callers run on different threads and can
     * overlap — a notification Stop while the app foregrounds and calls `stop()`.
     * Interleaved passes double-stop the transports mid-teardown, and a throw on
     * both leaves every later step unreached while the user-stop path still
     * reports the mesh down. Serialized, the second entrant re-runs the stops
     * after a completed first pass, which is their idempotent no-op path.
     *
     * The keep-alive and the core come down in `finally`: a transport that
     * throws must not strand the notification advertising an active mesh, nor
     * leave the core ticking its outbox against stopped transports. The
     * exception still propagates, so [stop] rejects as it did before.
     */
    @Synchronized
    private fun stopTransportsAndProtocol() {
        try {
            stopProcessScheduler()

            // Stop BLE manager first
            bleTransport?.stop()
            android.util.Log.i(NAME, "BLE Manager stopped")
            emitDiagnostic("info", "BLE manager stopped")

            // Stop Internet manager
            internetManager?.stop()
            android.util.Log.i(NAME, "Internet Manager stopped")
            emitDiagnostic("info", "Internet manager stopped")

            // Stop WiFi Direct manager
            wifiDirectManager?.stop()
            android.util.Log.i(NAME, "WiFi Direct Manager stopped")
            emitDiagnostic("info", "WiFi Direct manager stopped")

            // Stop Reticulum manager
            reticulumManager?.stop()
            android.util.Log.i(NAME, "Reticulum Manager stopped")
            emitDiagnostic("info", "Reticulum manager stopped")

            // Stop Nostr manager
            nostrManager?.stop()
            android.util.Log.i(NAME, "Nostr Manager stopped")
            emitDiagnostic("info", "Nostr manager stopped")
        } finally {
            // Stop foreground service
            stopForegroundService()

            protocol?.stop()
            emitDiagnostic("info", "Protocol stopped")
        }
    }

    /**
     * The user tapped "Stop" on the mesh notification. Runs the same teardown
     * as the JS-facing [stop], then tells JS — without the event, the app would
     * keep reporting mesh as active against transports that are now down.
     *
     * The service invokes this from its main-thread `onStartCommand`, so the
     * work moves off that thread: [stopProcessScheduler] blocks for up to
     * [PROCESS_SHUTDOWN_TIMEOUT_MS] waiting for an in-flight process tick, and
     * it waits again for any overlapping teardown to release the lock.
     *
     * The event fires in `finally` so a throwing transport cannot leave JS
     * believing the mesh is still up; [stopTransportsAndProtocol] already
     * guarantees the keep-alive came down on that path.
     */
    private fun handleUserRequestedMeshStop() {
        Thread({
            try {
                stopTransportsAndProtocol()
            } catch (e: Exception) {
                android.util.Log.e(NAME, "User-requested mesh stop failed", e)
                emitDiagnostic("error", "User-requested mesh stop failed", mapOf(
                    "message" to (e.message ?: "unknown"),
                    "exception" to e.javaClass.simpleName
                ))
            } finally {
                emitMeshStoppedByUserEvent()
            }
        }, "mesh-user-stop").start()
    }

    /**
     * Reports that the mesh was stopped from the notification's Stop action
     * rather than through a JS `stop()` call, so the app can reconcile its own
     * "mesh active" state with a teardown it did not initiate.
     */
    private fun emitMeshStoppedByUserEvent() {
        try {
            val json = JSONObject()
            json.put("type", "mesh_stopped_by_user")
            val params = Arguments.createMap().apply {
                putString("eventJson", json.toString())
            }
            sendEvent(EVENT_NAME, params)
        } catch (e: Exception) {
            android.util.Log.e(NAME, "Failed to emit mesh stopped event", e)
        }
    }

    @ReactMethod
    fun pause(promise: Promise) {
        try {
            // Stop the process scheduler so process() doesn't tick while paused
            stopProcessScheduler()

            // Pause all transports consistently
            bleTransport?.pause()
            internetManager?.pause()
            wifiDirectManager?.pause()
            reticulumManager?.pause()
            nostrManager?.pause()

            protocol?.pause()
            promise.resolve(null)
        } catch (e: Exception) {
            promise.reject("ERROR_PAUSE", "Failed to pause protocol: ${e.message}", e)
        }
    }

    @ReactMethod
    fun resume(promise: Promise) {
        try {
            protocol?.resume()
            
            // Resume all transports consistently
            bleTransport?.resume()
            internetManager?.resume()
            wifiDirectManager?.resume()
            reticulumManager?.resume()
            nostrManager?.resume()

            // Restart the process scheduler
            if (protocol?.getState() == ProtocolState.RUNNING) {
                startProcessScheduler()
            }
            
            promise.resolve(null)
        } catch (e: Exception) {
            promise.reject("ERROR_RESUME", "Failed to resume protocol: ${e.message}", e)
        }
    }

    @ReactMethod
    fun sendMessage(recipient: String, content: String, priority: Int, replyToMsg: String?, promise: Promise) {
        try {
            val msgPriority = when (priority) {
                0 -> MessagePriority.LOW
                1 -> MessagePriority.MEDIUM
                2 -> MessagePriority.HIGH
                3 -> MessagePriority.CRITICAL
                else -> MessagePriority.MEDIUM
            }
            
            val proto = protocol ?: throw IllegalStateException("Protocol not initialized")
            val messageId = proto.sendMessage(recipient, content, msgPriority, replyToMsg)
            promise.resolve(messageId)
        } catch (e: Exception) {
            rejectWithProtocolError(promise, e, "ERROR_SEND", "Failed to send message")
        }
    }

    /**
     * Rich send surface: reply context, rich media metadata, and forward
     * attribution — sealed inside the MLS ciphertext for capable recipients,
     * silently dropped for everyone else (never cleartext).
     */
    @ReactMethod
    fun sendMessageRich(recipient: String, content: String, priority: Int, replyToMsg: String?, options: ReadableMap?, promise: Promise) {
        try {
            val msgPriority = when (priority) {
                0 -> MessagePriority.LOW
                1 -> MessagePriority.MEDIUM
                2 -> MessagePriority.HIGH
                3 -> MessagePriority.CRITICAL
                else -> MessagePriority.MEDIUM
            }

            val proto = protocol ?: throw IllegalStateException("Protocol not initialized")
            val sendOptions = SendMessageOptions(
                priority = msgPriority,
                replyToMsg = replyToMsg,
                contentType = options?.getString("content_type")?.let { parseContentType(it) },
                replyContext = parseReplyContext(options?.getMap("reply_context")),
                mediaMetadata = parseRichMediaMetadata(options?.getMap("media_metadata")),
                forwardInfo = parseForwardInfo(options?.getMap("forward_info"))
            )
            val messageId = proto.sendMessageRich(recipient, content, sendOptions)
            promise.resolve(messageId)
        } catch (e: Exception) {
            rejectWithProtocolError(promise, e, "ERROR_SEND", "Failed to send rich message")
        }
    }

    private fun parseReplyContext(map: ReadableMap?): ReplyContext? {
        if (map == null) return null
        return ReplyContext(
            sender = map.getString("sender") ?: "",
            text = map.getString("text") ?: "",
            timestamp = if (map.hasKey("timestamp") && !map.isNull("timestamp")) map.getDouble("timestamp").toLong() else null,
            replyMediaLabel = map.getString("reply_media_label"),
            replyContentType = map.getString("reply_content_type")
        )
    }

    private fun parseForwardInfo(map: ReadableMap?): ForwardInfo? {
        if (map == null) return null
        return ForwardInfo(
            originalSender = map.getString("original_sender") ?: "",
            originalMessageId = map.getString("original_message_id") ?: "",
            originalTimestamp = if (map.hasKey("original_timestamp") && !map.isNull("original_timestamp")) map.getDouble("original_timestamp").toLong() else 0L,
            forwardCount = if (map.hasKey("forward_count") && !map.isNull("forward_count")) map.getInt("forward_count").toUInt() else 1u
        )
    }

    /**
     * Full MediaMetadata parser for the rich send surface — unlike the
     * legacy sendMedia mapping, this includes the cloud/sticker fields
     * (they only ever travel MLS-sealed on this path).
     */
    private fun parseRichMediaMetadata(map: ReadableMap?): MediaMetadata? {
        if (map == null) return null
        return MediaMetadata(
            mimeType = map.getString("mime_type") ?: "",
            fileName = map.getString("file_name") ?: "",
            fileSize = if (map.hasKey("file_size") && !map.isNull("file_size")) map.getDouble("file_size").toULong() else 0u,
            durationMs = if (map.hasKey("duration_ms") && !map.isNull("duration_ms")) map.getDouble("duration_ms").toULong() else null,
            width = if (map.hasKey("width") && !map.isNull("width")) map.getInt("width").toUInt() else null,
            height = if (map.hasKey("height") && !map.isNull("height")) map.getInt("height").toUInt() else null,
            thumbnailBase64 = map.getString("thumbnail_base64"),
            mediaId = map.getString("media_id"),
            downloadUrl = map.getString("download_url"),
            thumbnailUrl = map.getString("thumbnail_url"),
            encryptionKey = map.getString("encryption_key"),
            iv = map.getString("iv"),
            ciphertextHash = map.getString("ciphertext_hash"),
            stickerProvider = map.getString("sticker_provider"),
            stickerRemoteId = map.getString("sticker_remote_id"),
            stickerKind = map.getString("sticker_kind")
        )
    }

    @ReactMethod
    fun forwardMessage(originalMessageJson: String, newRecipient: String, priority: Int?, promise: Promise) {
        try {
            val proto = protocol ?: throw IllegalStateException("Protocol not initialized")
            val msgPriority = priority?.let {
                when (it) {
                    0 -> MessagePriority.LOW
                    1 -> MessagePriority.MEDIUM
                    2 -> MessagePriority.HIGH
                    3 -> MessagePriority.CRITICAL
                    else -> null
                }
            }
            val messageId = proto.forwardMessage(originalMessageJson, newRecipient, msgPriority)
            promise.resolve(messageId)
        } catch (e: Exception) {
            rejectWithProtocolError(promise, e, "ERROR_FORWARD", "Failed to forward message")
        }
    }

    @ReactMethod
    fun sendConnectionRequest(recipient: String, senderName: String, keyPackage: ReadableArray?, initialMessage: String?, promise: Promise) {
        try {
            val proto = protocol ?: throw IllegalStateException("Protocol not initialized")
            val keyPackageData = if (keyPackage != null) {
                val data = mutableListOf<UByte>()
                for (i in 0 until keyPackage.size()) {
                    data.add(keyPackage.getInt(i).toUByte())
                }
                data
            } else {
                null
            }
            val messageId = proto.sendConnectionRequest(recipient, senderName, keyPackageData, initialMessage)
            promise.resolve(messageId)
        } catch (e: Exception) {
            rejectWithProtocolError(promise, e, "ERROR_CONNECTION_REQUEST", "Failed to send connection request")
        }
    }

    @ReactMethod
    fun acceptConnectionRequest(recipient: String, accepterName: String, keyPackage: ReadableArray?, promise: Promise) {
        try {
            val proto = protocol ?: throw IllegalStateException("Protocol not initialized")
            val keyPackageData = if (keyPackage != null) {
                val data = mutableListOf<UByte>()
                for (i in 0 until keyPackage.size()) {
                    data.add(keyPackage.getInt(i).toUByte())
                }
                data
            } else {
                null
            }
            val messageId = proto.acceptConnectionRequest(recipient, accepterName, keyPackageData)
            promise.resolve(messageId)
        } catch (e: Exception) {
            rejectWithProtocolError(promise, e, "ERROR_CONNECTION_REQUEST", "Failed to accept connection request")
        }
    }

    @ReactMethod
    fun rejectConnectionRequest(recipient: String, promise: Promise) {
        try {
            val proto = protocol ?: throw IllegalStateException("Protocol not initialized")
            val messageId = proto.rejectConnectionRequest(recipient)
            promise.resolve(messageId)
        } catch (e: Exception) {
            rejectWithProtocolError(promise, e, "ERROR_CONNECTION_REQUEST", "Failed to reject connection request")
        }
    }

    @ReactMethod
    fun cancelConnectionRequest(recipient: String, promise: Promise) {
        try {
            val proto = protocol ?: throw IllegalStateException("Protocol not initialized")
            val messageId = proto.cancelConnectionRequest(recipient)
            promise.resolve(messageId)
        } catch (e: Exception) {
            rejectWithProtocolError(promise, e, "ERROR_CONNECTION_REQUEST", "Failed to cancel connection request")
        }
    }

    // User Blocking

    @ReactMethod
    fun blockUser(userId: String, promise: Promise) {
        try {
            val proto = protocol ?: throw IllegalStateException("Protocol not initialized")
            proto.blockUser(userId)
            promise.resolve(null)
        } catch (e: Exception) {
            promise.reject("ERROR_BLOCK_USER", "Failed to block user: ${e.message}", e)
        }
    }

    @ReactMethod
    fun unblockUser(userId: String, promise: Promise) {
        try {
            val proto = protocol ?: throw IllegalStateException("Protocol not initialized")
            proto.unblockUser(userId)
            promise.resolve(null)
        } catch (e: Exception) {
            promise.reject("ERROR_UNBLOCK_USER", "Failed to unblock user: ${e.message}", e)
        }
    }

    @ReactMethod
    fun getBlockedUsers(promise: Promise) {
        try {
            val proto = protocol ?: throw IllegalStateException("Protocol not initialized")
            val blocked = proto.getBlockedUsers()
            val array = Arguments.createArray()
            blocked.forEach { array.pushString(it) }
            promise.resolve(array)
        } catch (e: Exception) {
            promise.reject("ERROR_GET_BLOCKED", "Failed to get blocked users: ${e.message}", e)
        }
    }

    @ReactMethod
    fun isUserBlocked(userId: String, promise: Promise) {
        try {
            val proto = protocol ?: throw IllegalStateException("Protocol not initialized")
            val isBlocked = proto.isUserBlocked(userId)
            promise.resolve(isBlocked)
        } catch (e: Exception) {
            promise.reject("ERROR_IS_BLOCKED", "Failed to check blocked status: ${e.message}", e)
        }
    }

    @ReactMethod
    fun resetTofuForPeer(peerId: String, promise: Promise) {
        try {
            val proto = protocol ?: throw IllegalStateException("Protocol not initialized")
            val removed = proto.resetTofuForPeer(peerId)
            promise.resolve(removed)
        } catch (e: Exception) {
            promise.reject("ERROR_TOFU", "Failed to reset TOFU for peer: ${e.message}", e)
        }
    }

    // ─── Presence, Typing, Read Receipts ───────────────────────

    @ReactMethod
    fun sendPresenceUpdate(recipient: String, status: Int, promise: Promise) {
        try {
            val proto = protocol ?: throw IllegalStateException("Protocol not initialized")
            val presenceStatus = when (status) {
                0 -> uniffi.offline_protocol.PresenceStatus.ONLINE
                1 -> uniffi.offline_protocol.PresenceStatus.AWAY
                2 -> uniffi.offline_protocol.PresenceStatus.OFFLINE
                else -> uniffi.offline_protocol.PresenceStatus.ONLINE
            }
            val messageId = proto.sendPresenceUpdate(recipient, presenceStatus)
            promise.resolve(messageId)
        } catch (e: Exception) {
            rejectWithProtocolError(promise, e, "ERROR_PRESENCE_UPDATE", "Failed to send presence update")
        }
    }

    @ReactMethod
    fun sendTypingIndicator(recipient: String, conversationId: String, isTyping: Boolean, promise: Promise) {
        try {
            val proto = protocol ?: throw IllegalStateException("Protocol not initialized")
            val messageId = proto.sendTypingIndicator(recipient, conversationId, isTyping)
            promise.resolve(messageId)
        } catch (e: Exception) {
            rejectWithProtocolError(promise, e, "ERROR_TYPING_INDICATOR", "Failed to send typing indicator")
        }
    }

    @ReactMethod
    fun sendReadReceipt(recipient: String, messageIds: ReadableArray, promise: Promise) {
        try {
            val proto = protocol ?: throw IllegalStateException("Protocol not initialized")
            val msgIds = (0 until messageIds.size())
                .mapNotNull { messageIds.getString(it)?.takeIf(String::isNotEmpty) }
            if (msgIds.isEmpty()) {
                promise.reject("ERROR_READ_RECEIPT", "No valid message IDs provided", null)
                return
            }
            val messageId = proto.sendReadReceipt(recipient, msgIds)
            promise.resolve(messageId)
        } catch (e: Exception) {
            rejectWithProtocolError(promise, e, "ERROR_READ_RECEIPT", "Failed to send read receipt")
        }
    }

    // Service Discovery & Request/Response (via MeshServices)

    @ReactMethod
    fun registerService(serviceId: String, version: String, capabilitiesJson: String, promise: Promise) {
        try {
            val svc = meshServices ?: throw IllegalStateException("MeshServices not initialized")
            val capabilities = mutableMapOf<String, String>()
            try {
                val json = JSONObject(capabilitiesJson)
                json.keys().forEach { key -> capabilities[key] = json.getString(key) }
            } catch (parseErr: Exception) {
                android.util.Log.w("OfflineProtocol", "Failed to parse capabilities JSON, registering with empty capabilities: ${parseErr.message}")
            }
            svc.registerService(serviceId, version, capabilities)
            promise.resolve(null)
        } catch (e: Exception) {
            promise.reject("ERROR_REGISTER_SERVICE", "Failed to register service: ${e.message}", e)
        }
    }

    @ReactMethod
    fun unregisterService(serviceId: String, promise: Promise) {
        try {
            val svc = meshServices ?: throw IllegalStateException("MeshServices not initialized")
            val removed = svc.unregisterService(serviceId)
            promise.resolve(removed)
        } catch (e: Exception) {
            promise.reject("ERROR_UNREGISTER_SERVICE", "Failed to unregister service: ${e.message}", e)
        }
    }

    @ReactMethod
    fun discoverServices(serviceId: String?, promise: Promise) {
        try {
            val svc = meshServices ?: throw IllegalStateException("MeshServices not initialized")
            val queryId = svc.discoverServices(serviceId)
            promise.resolve(queryId)
        } catch (e: Exception) {
            promise.reject("ERROR_DISCOVER_SERVICES", "Failed to discover services: ${e.message}", e)
        }
    }

    @ReactMethod
    fun sendServiceRequest(provider: String, serviceId: String, method: String, body: String, promise: Promise) {
        try {
            val svc = meshServices ?: throw IllegalStateException("MeshServices not initialized")
            val requestId = svc.sendServiceRequest(provider, serviceId, method, body)
            promise.resolve(requestId)
        } catch (e: Exception) {
            rejectWithProtocolError(promise, e, "ERROR_SERVICE_REQUEST", "Failed to send service request")
        }
    }

    @ReactMethod
    fun respondToServiceRequest(requestId: String, requester: String, serviceId: String, status: String, body: String, promise: Promise) {
        try {
            val svc = meshServices ?: throw IllegalStateException("MeshServices not initialized")
            val messageId = svc.respondToServiceRequest(requestId, requester, serviceId, status, body)
            promise.resolve(messageId)
        } catch (e: Exception) {
            rejectWithProtocolError(promise, e, "ERROR_SERVICE_RESPONSE", "Failed to respond to service request")
        }
    }

    @ReactMethod
    fun receiveMessage(promise: Promise) {
        val messageJson = protocol?.receiveMessage()
        promise.resolve(messageJson)
    }

    @ReactMethod
    fun destroy(promise: Promise) {
        try {
            stopProcessScheduler()
            bleTransport?.stop()
            bleTransport = null
            internetManager?.stop()
            internetManager = null
            wifiDirectManager?.stop()
            wifiDirectManager = null
            reticulumManager?.stop()
            reticulumManager = null
            nostrManager?.stop()
            nostrManager = null

            try {
                protocol?.stop()
            } catch (_: Exception) {
                // Ignore stop errors during destroy
            }

            protocol = null
            meshServices = null
            listenerCount = 0
            currentConfig = null
            promise.resolve(null)
        } catch (e: Exception) {
            promise.reject("ERROR_DESTROY", "Failed to destroy protocol: ${e.message}", e)
        }
    }

    /**
     * Erases every byte of persisted SDK state for one account.
     *
     * Takes the identity explicitly rather than reading [currentConfig], because
     * the call this exists for happens *after* [destroy], which clears it. That
     * also lets an application wipe the account it just signed out of while a
     * different one is already running.
     *
     * Refuses to wipe the account this instance is currently running: the
     * protocol persists as it works — outbox entries on the send path, pending
     * snapshots, sealed state records — so a wipe underneath a live instance
     * races those writes and leaves a partially repopulated container. Call
     * `destroy()` first.
     */
    @ReactMethod
    fun wipePersistedState(appId: String, userId: String, promise: Promise) {
        try {
            val namespace = StorageNamespace.account(appId, userId)
            val live = currentConfig
            if (live != null && StorageNamespace.account(live.appId, live.userId) == namespace) {
                promise.reject(
                    "ERROR_WIPE_STATE",
                    "Refusing to wipe storage for the account this instance is " +
                        "running. Call destroy() first."
                )
                return
            }

            // Secure storage first: it holds the key every sealed protocol-state
            // record is written under, so an interrupted wipe leaves the
            // remainder as ciphertext nobody can open rather than readable
            // state.
            var firstError: Exception? = null
            try {
                MlsSecureStorage.wipeAccount(reactApplicationContext, namespace)
            } catch (error: Exception) {
                firstError = error
            }
            try {
                AppContainerProtocolStateStorage.wipeAccount(
                    reactApplicationContext,
                    namespace
                )
            } catch (error: Exception) {
                if (firstError == null) {
                    firstError = error
                }
            }
            firstError?.let { throw it }
            promise.resolve(null)
        } catch (e: Exception) {
            promise.reject(
                "ERROR_WIPE_STATE",
                "Failed to wipe persisted state: ${e.message}",
                e
            )
        }
    }

    // ========================================================================
    // TRANSPORT MANAGEMENT
    // ========================================================================

    @ReactMethod
    fun enableTransport(type: String, config: ReadableMap?, promise: Promise) {
        try {
            val proto = protocol ?: throw IllegalStateException("Protocol not initialized")
            when (type.lowercase()) {
                "internet" -> {
                    // Configure and start Internet transport via InternetManager
                    if (internetManager == null) {
                        // The manager's identity must be the real user id: the
                        // control-op translator filters self out of member
                        // deltas and gates LeaveGroup on it, so a placeholder
                        // would silently corrupt relay group state.
                        val userId = currentConfig?.userId
                            ?: throw IllegalStateException("Cannot enable Internet transport before initialize(config)")
                        internetManager = InternetManager(reactApplicationContext, proto, userId) { level, message, context ->
                            emitDiagnostic(level, message, context)
                        }.also { manager ->
                            manager.serverMessageEmitter = { rawJson -> emitServerMessageEvent(rawJson) }
                            manager.connectionStatusEmitter = { connected, authenticated ->
                                emitInternetStatusEvent(connected, authenticated)
                            }
                            manager.supersededEmitter = { reason -> emitInternetSupersededEvent(reason) }
                        }
                        emitDiagnostic("info", "Internet manager created on demand")
                    }
                    
                    val manager = internetManager
                        ?: throw IllegalStateException("Failed to create Internet manager")
                    
                    // Stop the manager first if it's active (to ensure clean
                    // restart). STARTING counts: a manager mid-handshake holds
                    // a live OkHttp client and an isConnecting latch — starting
                    // over it would leak the client and silently drop the new
                    // configuration (connect() early-returns on the latch, so
                    // the old handshake to the old URL would just continue).
                    if (manager.state == TransportState.RUNNING ||
                        manager.state == TransportState.STARTING) {
                        manager.stop()
                    }

                    configureAndStartInternet(manager, config)
                    emitDiagnostic("info", "Internet transport enabled")
                }
                "wifidirect", "wifi_direct" -> {
                    // Configure and start WiFi Direct transport via WifiDirectManager
                    if (wifiDirectManager == null) {
                        // Create manager if not already created
                        wifiDirectManager = WifiDirectManager(reactApplicationContext, proto, currentConfig?.userId ?: "unknown") { level, message, context ->
                            emitDiagnostic(level, message, context)
                        }
                        emitDiagnostic("info", "WiFi Direct manager created on demand")
                    }
                    
                    val manager = wifiDirectManager
                        ?: throw IllegalStateException("Failed to create WiFi Direct manager")
                    
                    // Stop the manager first if it's running (to ensure clean restart)
                    if (manager.state == TransportState.RUNNING) {
                        manager.stop()
                    }
                    
                    manager.start()
                    emitDiagnostic("info", "WiFi Direct transport enabled")
                }
                "ble" -> {
                    // Start BLE manager if stopped
                    if (bleTransport == null) {
                        bleTransport = BleTransportFacade(
                            reactApplicationContext,
                            proto,
                            currentConfig?.userId ?: "unknown",
                        ) { level, message, context ->
                            emitDiagnostic(level, message, context)
                        }
                        emitDiagnostic("info", "BLE manager created on demand")
                    }

                    val manager = bleTransport
                        ?: throw IllegalStateException("Failed to create BLE manager")

                    if (manager.state != TransportState.RUNNING) {
                        manager.start()
                        emitDiagnostic("info", "BLE transport enabled")
                    }
                }
                "reticulum" -> {
                    if (reticulumManager == null) {
                        reticulumManager = createReticulumManager(proto, currentConfig?.userId ?: "unknown")
                        emitDiagnostic("info", "Reticulum manager created on demand")
                    }

                    val manager = reticulumManager
                        ?: throw IllegalStateException("Failed to create Reticulum manager")

                    if (manager.state == TransportState.RUNNING) {
                        manager.stop()
                    }

                    configureAndStartReticulum(manager, config)
                    emitDiagnostic("info", "Reticulum transport enabled")
                }
                "nostr" -> {
                    if (nostrManager == null) {
                        nostrManager = createNostrManager(proto, currentConfig?.userId ?: "unknown")
                        emitDiagnostic("info", "Nostr manager created on demand")
                    }

                    val manager = nostrManager
                        ?: throw IllegalStateException("Failed to create Nostr manager")

                    if (manager.state == TransportState.RUNNING) {
                        manager.stop()
                    }

                    configureAndStartNostr(manager, config)
                    emitDiagnostic("info", "Nostr transport enabled")
                }
                else -> throw IllegalArgumentException("Unsupported transport type: $type")
            }
            promise.resolve(null)
        } catch (e: ProtocolException) {
            promise.reject("ERROR_TRANSPORT_ENABLE", "Failed to enable transport: ${e.message}", e)
        } catch (e: Exception) {
            promise.reject("ERROR_TRANSPORT_ENABLE", "Failed to enable transport: ${e.message}", e)
        }
    }
    
    private fun configureAndStartInternet(manager: InternetManager, config: ReadableMap?) {
        val serverAddress = config?.getString("serverAddress")
            ?: config?.getString("server_url")
            ?: throw IllegalArgumentException("Internet transport requires a serverAddress")
        
        // Build WebSocket URL
        var wsUrl = serverAddress.trim()
        if (!wsUrl.startsWith("ws://") && !wsUrl.startsWith("wss://")) {
            // Default to wss:// for secure connection
            wsUrl = "wss://$wsUrl"
        }
        
        // Append port if specified (safely check for key existence first)
        val port = when {
            config?.hasKey("port") == true -> config.getDouble("port").toInt()
            config?.hasKey("serverPort") == true -> config.getDouble("serverPort").toInt()
            else -> null
        }
        if (port != null && port > 0) {
            // Check if URL already has a port
            val uri = java.net.URI(wsUrl)
            if (uri.port == -1) {
                wsUrl = "$wsUrl:$port"
            }
        }
        
        val autoReconnect = if (config?.hasKey("autoReconnect") == true) {
            config.getBoolean("autoReconnect")
        } else {
            true
        }
        val maxRetries = if (config?.hasKey("maxReconnectAttempts") == true) {
            config.getDouble("maxReconnectAttempts").toInt()
        } else {
            0
        }
        
        // Set auth token if provided in config
        val authToken = config?.getString("authToken")
        if (authToken != null) {
            manager.setAuthToken(authToken)
        }
        
        // Internet transport is already registered during protocol initialization
        // Just configure and start the WebSocket manager
        manager.configure(wsUrl, autoReconnect, maxRetries)
        manager.start()
        
        emitDiagnostic("info", "Internet transport enabled", mapOf(
            "serverUrl" to wsUrl,
            "autoReconnect" to autoReconnect,
            "hasAuthToken" to (authToken != null)
        ))
    }

    private fun createReticulumManager(proto: OfflineProtocol, userId: String): ReticulumManager {
        return ReticulumManager(reactApplicationContext, proto, userId) { level, message, context ->
            emitDiagnostic(level, message, context)
        }.also { manager ->
            manager.listener = object : TransportManagerListener {
                override fun onTransportStateChanged(manager: TransportManager, state: TransportState) {
                    emitDiagnostic("info", "Reticulum transport state changed", mapOf(
                        "transport" to manager.transportId,
                        "state" to state.name.lowercase()
                    ))
                }

                override fun onTransportError(manager: TransportManager, error: Throwable) {
                    emitDiagnostic("error", "Reticulum transport error", mapOf(
                        "transport" to manager.transportId,
                        "message" to (error.message ?: "unknown"),
                        "exception" to error.javaClass.simpleName
                    ))
                }

                override fun onTransportMetricsUpdated(manager: TransportManager, metrics: Map<String, Any>) {
                    val enrichedMetrics = metrics.toMutableMap()
                    enrichedMetrics["transport"] = manager.transportId
                    emitDiagnostic("info", "Reticulum transport metrics", enrichedMetrics)
                }

                override fun onTransportDiagnostic(
                    manager: TransportManager,
                    level: String,
                    message: String,
                    context: Map<String, Any?>
                ) {
                    val enrichedContext = context.toMutableMap()
                    enrichedContext["transport"] = manager.transportId
                    emitDiagnostic(level, message, enrichedContext)
                }
            }
        }
    }

    private fun configureAndStartReticulum(manager: ReticulumManager, config: ReadableMap?) {
        val daemonAddress = config?.getString("daemonAddress")
            ?: config?.getString("daemon_address")
            ?: "localhost:4242"

        val autoReconnect = if (config?.hasKey("autoReconnect") == true) {
            config.getBoolean("autoReconnect")
        } else {
            true
        }
        val maxRetries = if (config?.hasKey("maxReconnectAttempts") == true) {
            config.getDouble("maxReconnectAttempts").toInt()
        } else {
            0
        }

        manager.configure(daemonAddress, autoReconnect, maxRetries)
        manager.start()

        emitDiagnostic("info", "Reticulum transport enabled", mapOf(
            "daemonAddress" to daemonAddress,
            "autoReconnect" to autoReconnect
        ))
    }

    private fun createNostrManager(proto: OfflineProtocol, userId: String): NostrManager {
        return NostrManager(reactApplicationContext, proto, userId) { level, message, context ->
            emitDiagnostic(level, message, context)
        }.also { manager ->
            manager.listener = object : TransportManagerListener {
                override fun onTransportStateChanged(manager: TransportManager, state: TransportState) {
                    emitDiagnostic("info", "Nostr transport state changed", mapOf(
                        "transport" to manager.transportId,
                        "state" to state.name.lowercase()
                    ))
                }

                override fun onTransportError(manager: TransportManager, error: Throwable) {
                    emitDiagnostic("error", "Nostr transport error", mapOf(
                        "transport" to manager.transportId,
                        "message" to (error.message ?: "unknown"),
                        "exception" to error.javaClass.simpleName
                    ))
                }

                override fun onTransportMetricsUpdated(manager: TransportManager, metrics: Map<String, Any>) {
                    val enrichedMetrics = metrics.toMutableMap()
                    enrichedMetrics["transport"] = manager.transportId
                    emitDiagnostic("info", "Nostr transport metrics", enrichedMetrics)
                }

                override fun onTransportDiagnostic(
                    manager: TransportManager,
                    level: String,
                    message: String,
                    context: Map<String, Any?>
                ) {
                    val enrichedContext = context.toMutableMap()
                    enrichedContext["transport"] = manager.transportId
                    emitDiagnostic(level, message, enrichedContext)
                }
            }
        }
    }

    private fun configureAndStartNostr(manager: NostrManager, config: ReadableMap?) {
        val relayUrls = mutableListOf<String>()
        if (config?.hasKey("relayUrls") == true) {
            val arr = config.getArray("relayUrls")
            if (arr != null) {
                for (i in 0 until arr.size()) {
                    arr.getString(i)?.let { relayUrls.add(it) }
                }
            }
        }
        if (config?.hasKey("relay_urls") == true && relayUrls.isEmpty()) {
            val arr = config.getArray("relay_urls")
            if (arr != null) {
                for (i in 0 until arr.size()) {
                    arr.getString(i)?.let { relayUrls.add(it) }
                }
            }
        }

        if (relayUrls.isEmpty()) {
            throw IllegalArgumentException("Nostr transport requires at least one relay URL")
        }

        val autoReconnect = if (config?.hasKey("autoReconnect") == true) {
            config.getBoolean("autoReconnect")
        } else {
            true
        }
        val maxRetries = if (config?.hasKey("maxReconnectAttempts") == true) {
            config.getDouble("maxReconnectAttempts").toInt()
        } else {
            0
        }

        manager.configure(relayUrls, autoReconnect, maxRetries)
        manager.start()

        emitDiagnostic("info", "Nostr transport enabled", mapOf(
            "relayCount" to relayUrls.size,
            "autoReconnect" to autoReconnect
        ))
    }

    @ReactMethod
    fun disableTransport(type: String, promise: Promise) {
        try {
            val proto = protocol ?: throw IllegalStateException("Protocol not initialized")
            
            // Stop corresponding transport manager and mark transport as unavailable
            // Note: We don't remove the transport from the Rust protocol anymore
            // because that prevents re-enabling it. Instead, we just stop the manager
            // and the transport status will be updated to unavailable/disconnected.
            when (type.lowercase()) {
                "internet" -> {
                    internetManager?.stop()
                    // Notify the protocol that internet is disconnected
                    try {
                        proto.internetStatusChanged(false)
                    } catch (e: Exception) {
                        android.util.Log.w(NAME, "Failed to notify internet status change: ${e.message}")
                    }
                    emitDiagnostic("info", "Internet transport disabled (manager stopped)")
                }
                "wifidirect", "wifi_direct" -> {
                    wifiDirectManager?.stop()
                    emitDiagnostic("info", "WiFi Direct transport disabled (manager stopped)")
                }
                "ble" -> {
                    bleTransport?.stop()
                    try {
                        proto.bleStatusChanged(false)
                    } catch (e: Exception) {
                        android.util.Log.w(NAME, "Failed to notify BLE status change: ${e.message}")
                    }
                    emitDiagnostic("info", "BLE transport disabled (manager stopped)")
                }
                "reticulum" -> {
                    reticulumManager?.stop()
                    try {
                        proto.reticulumStatusChanged(false)
                    } catch (e: Exception) {
                        android.util.Log.w(NAME, "Failed to notify reticulum status change: ${e.message}")
                    }
                    emitDiagnostic("info", "Reticulum transport disabled (manager stopped)")
                }
                "nostr" -> {
                    nostrManager?.stop()
                    try {
                        proto.nostrStatusChanged(false)
                    } catch (e: Exception) {
                        android.util.Log.w(NAME, "Failed to notify nostr status change: ${e.message}")
                    }
                    emitDiagnostic("info", "Nostr transport disabled (manager stopped)")
                }
                else -> throw IllegalArgumentException("Unsupported transport type: $type")
            }

            promise.resolve(null)
        } catch (e: Exception) {
            promise.reject("ERROR_TRANSPORT_DISABLE", "Failed to disable transport: ${e.message}", e)
        }
    }
    
    
    @ReactMethod
    fun isBluetoothEnabled(promise: Promise) {
        try {
            val bluetoothManager = reactApplicationContext.getSystemService(android.content.Context.BLUETOOTH_SERVICE) as? android.bluetooth.BluetoothManager
            val adapter = bluetoothManager?.adapter
            val enabled = adapter?.isEnabled == true
            promise.resolve(enabled)
        } catch (e: Exception) {
            // If we can't check, assume enabled and let the protocol handle it
            promise.resolve(true)
        }
    }

    @ReactMethod
    fun requestEnableBluetooth(promise: Promise) {
        try {
            val bluetoothManager = reactApplicationContext.getSystemService(android.content.Context.BLUETOOTH_SERVICE) as? android.bluetooth.BluetoothManager
            val adapter = bluetoothManager?.adapter
            
            if (adapter == null) {
                promise.reject("ERROR_BLUETOOTH", "Bluetooth not available on this device", null)
                return
            }
            
            if (adapter.isEnabled) {
                promise.resolve(true)
                return
            }
            
            // Request to enable Bluetooth via intent
            val enableBtIntent = android.content.Intent(android.bluetooth.BluetoothAdapter.ACTION_REQUEST_ENABLE)
            val activity = reactApplicationContext.currentActivity
            if (activity != null) {
                @Suppress("DEPRECATION")
                activity.startActivityForResult(enableBtIntent, 1001)
                // Note: We can't wait for the result in a synchronous way
                // The user will need to re-try after enabling
                promise.resolve(false)
            } else {
                promise.reject("ERROR_BLUETOOTH", "No activity available to show Bluetooth enable dialog", null)
            }
        } catch (e: Exception) {
            promise.reject("ERROR_BLUETOOTH", "Failed to request Bluetooth enable: ${e.message}", e)
        }
    }

    @ReactMethod
    fun getTopology(promise: Promise) {
        try {
            val proto = protocol ?: throw IllegalStateException("Protocol not initialized")
            val topology = proto.getTopology()
            val topologyJson = buildTopologyJson(topology)
            promise.resolve(topologyJson)
        } catch (e: Exception) {
            promise.reject("ERROR_TOPOLOGY", "Failed to get topology: ${e.message}", e)
        }
    }

    @ReactMethod
    fun getMessageStats(promise: Promise) {
        try {
            val proto = protocol ?: throw IllegalStateException("Protocol not initialized")
            val stats = proto.getMessageStats()
            val statsJson = buildMessageStatsJson(stats)
            promise.resolve(statsJson)
        } catch (e: Exception) {
            promise.reject("ERROR_MESSAGE_STATS", "Failed to get message stats: ${e.message}", e)
        }
    }

    @ReactMethod
    fun getDeliverySuccessRate(promise: Promise) {
        try {
            val proto = protocol ?: throw IllegalStateException("Protocol not initialized")
            val rate = proto.getDeliverySuccessRate().toDouble()
            promise.resolve(rate)
        } catch (e: Exception) {
            promise.reject("ERROR_DELIVERY_RATE", "Failed to get delivery success rate: ${e.message}", e)
        }
    }

    @ReactMethod
    fun getMedianLatency(promise: Promise) {
        try {
            val proto = protocol ?: throw IllegalStateException("Protocol not initialized")
            val latency = proto.getMedianLatency().toLong()
            if (latency == 0L) {
                promise.resolve(null)
            } else {
                promise.resolve(latency)
            }
        } catch (e: Exception) {
            promise.reject("ERROR_MEDIAN_LATENCY", "Failed to get median latency: ${e.message}", e)
        }
    }

    @ReactMethod
    fun getMedianHops(promise: Promise) {
        try {
            val proto = protocol ?: throw IllegalStateException("Protocol not initialized")
            val hops = proto.getMedianHops().toInt()
            if (hops == 0) {
                promise.resolve(null)
            } else {
                promise.resolve(hops)
            }
        } catch (e: Exception) {
            promise.reject("ERROR_MEDIAN_HOPS", "Failed to get median hops: ${e.message}", e)
        }
    }

    @ReactMethod
    fun sendFile(recipient: String, fileData: String, fileName: String, promise: Promise) {
        try {
            val proto = protocol ?: throw IllegalStateException("Protocol not initialized")
            val bytes = android.util.Base64.decode(fileData, android.util.Base64.DEFAULT)
            val id = proto.sendFile(recipient, bytes.map { it.toUByte() }, fileName)
            promise.resolve(id)
        } catch (e: Exception) {
            rejectWithProtocolError(promise, e, "ERROR_SEND_FILE", "Failed to send file")
        }
    }

    @ReactMethod
    fun sendMedia(recipient: String, fileData: String, fileName: String, contentType: String, mediaMetadata: ReadableMap?, promise: Promise) {
        try {
            val proto = protocol ?: throw IllegalStateException("Protocol not initialized")
            val bytes = android.util.Base64.decode(fileData, android.util.Base64.DEFAULT)
            val ct = parseContentType(contentType)
            val meta = mediaMetadata?.let { map ->
                MediaMetadata(
                    mimeType = map.getString("mime_type") ?: "",
                    fileName = map.getString("file_name") ?: fileName,
                    fileSize = map.getDouble("file_size").toULong(),
                    durationMs = if (map.hasKey("duration_ms")) map.getDouble("duration_ms").toULong() else null,
                    width = if (map.hasKey("width")) map.getInt("width").toUInt() else null,
                    height = if (map.hasKey("height")) map.getInt("height").toUInt() else null,
                    thumbnailBase64 = map.getString("thumbnail_base64")
                )
            }
            val id = proto.sendMedia(recipient, bytes.map { it.toUByte() }, fileName, ct, meta)
            promise.resolve(id)
        } catch (e: Exception) {
            rejectWithProtocolError(promise, e, "ERROR_SEND_MEDIA", "Failed to send media")
        }
    }

    @ReactMethod
    fun sendMediaRich(recipient: String, fileData: String, fileName: String, contentType: String, options: ReadableMap?, promise: Promise) {
        try {
            val proto = protocol ?: throw IllegalStateException("Protocol not initialized")
            val bytes = android.util.Base64.decode(fileData, android.util.Base64.DEFAULT)
            val ct = parseContentType(contentType)
            val sendOptions = MediaSendOptions(
                mediaMetadata = parseRichMediaMetadata(options?.getMap("media_metadata")),
                caption = options?.getString("caption"),
                replyToMsg = options?.getString("reply_to_msg"),
                replyContext = parseReplyContext(options?.getMap("reply_context")),
                forwardInfo = parseForwardInfo(options?.getMap("forward_info")),
                fileId = options?.getString("file_id")
            )
            val id = proto.sendMediaRich(recipient, bytes.map { it.toUByte() }, fileName, ct, sendOptions)
            promise.resolve(id)
        } catch (e: Exception) {
            rejectWithProtocolError(promise, e, "ERROR_SEND_MEDIA", "Failed to send media")
        }
    }

    private fun parseContentType(value: String): ContentType {
        return when (value.lowercase()) {
            "text" -> ContentType.TEXT
            "image" -> ContentType.IMAGE
            "video" -> ContentType.VIDEO
            "audio" -> ContentType.AUDIO
            "voice_note" -> ContentType.VOICE_NOTE
            "video_note" -> ContentType.VIDEO_NOTE
            "file" -> ContentType.FILE
            "file_chunk" -> ContentType.FILE_CHUNK
            "poll" -> ContentType.POLL
            else -> ContentType.FILE
        }
    }

    @ReactMethod
    fun getFileProgress(fileId: String, promise: Promise) {
        try {
            val proto = protocol ?: throw IllegalStateException("Protocol not initialized")
            val progress = proto.getFileProgress(fileId)
            if (progress != null) {
                val map = Arguments.createMap().apply {
                    putString("file_id", progress.fileId)
                    putInt("chunks_sent", progress.chunksSent.toInt())
                    putInt("total_chunks", progress.totalChunks.toInt())
                    putInt("percentage", progress.percentage.toInt())
                }
                promise.resolve(map)
            } else {
                promise.resolve(null)
            }
        } catch (e: Exception) {
            promise.reject("ERROR_FILE_PROGRESS", "Failed to get file progress: ${e.message}", e)
        }
    }

    @ReactMethod
    fun cancelFileTransfer(fileId: String, promise: Promise) {
        try {
            val proto = protocol ?: throw IllegalStateException("Protocol not initialized")
            proto.cancelFileTransfer(fileId)
            promise.resolve(true)
        } catch (e: ProtocolException) {
            if (e.message?.contains("not found", ignoreCase = true) == true) {
                promise.resolve(false)
            } else {
                promise.reject("ERROR_FILE_CANCEL", "Failed to cancel file transfer: ${e.message}", e)
            }
        } catch (e: Exception) {
            promise.reject("ERROR_FILE_CANCEL", "Failed to cancel file transfer: ${e.message}", e)
        }
    }
    
    // ========================================================================
    // BLE TRANSPORT METHODS
    // ========================================================================
    
    @ReactMethod
    fun blePeerDiscovered(peerId: String, rssi: Int, promise: Promise) {
        try {
            protocol?.blePeerDiscovered(peerId, rssi.toShort())
            promise.resolve(null)
        } catch (e: Exception) {
            promise.reject("ERROR_BLE", "BLE peer discovered failed: ${e.message}", e)
        }
    }
    
    @ReactMethod
    fun blePeerLost(peerId: String, promise: Promise) {
        try {
            protocol?.blePeerLost(peerId)
            promise.resolve(null)
        } catch (e: Exception) {
            promise.reject("ERROR_BLE", "BLE peer lost failed: ${e.message}", e)
        }
    }
    
    @ReactMethod
    fun bleStatusChanged(isAvailable: Boolean, promise: Promise) {
        try {
            protocol?.bleStatusChanged(isAvailable)
            promise.resolve(null)
        } catch (e: Exception) {
            promise.reject("ERROR_BLE", "BLE status changed failed: ${e.message}", e)
        }
    }
    
    @ReactMethod
    fun bleFragmentReceived(senderId: String, fragmentData: ReadableArray, promise: Promise) {
        try {
            val fragment = mutableListOf<UByte>()
            for (i in 0 until fragmentData.size()) {
                fragment.add(fragmentData.getInt(i).toUByte())
            }
            protocol?.bleFragmentReceived(senderId, fragment)
            promise.resolve(null)
        } catch (e: Exception) {
            promise.reject("ERROR_BLE", "BLE fragment received failed: ${e.message}", e)
        }
    }
    
    @ReactMethod
    fun bleGetNextFragment(promise: Promise) {
        try {
            val fragment = protocol?.bleGetNextFragment()
            if (fragment != null) {
                val map = Arguments.createMap()
                map.putString("recipientId", fragment.recipientId)
                val array = Arguments.createArray()
                fragment.data.forEach { array.pushInt(it.toInt()) }
                map.putArray("data", array)
                promise.resolve(map)
            } else {
                promise.resolve(null)
            }
        } catch (e: Exception) {
            promise.reject("ERROR_BLE", "BLE get next fragment failed: ${e.message}", e)
        }
    }
    
    @ReactMethod
    fun bleReturnFragment(promise: Promise) {
        protocol?.bleReturnFragment()
        promise.resolve(null)
    }
    
    @ReactMethod
    fun bleGetPeerCount(promise: Promise) {
        val count = protocol?.bleGetPeerCount()?.toInt() ?: 0
        promise.resolve(count)
    }
    
    @ReactMethod
    fun getActiveTransports(promise: Promise) {
        val transports = protocol?.getActiveTransports() ?: emptyList<String>()
        promise.resolve(Arguments.fromList(transports))
    }
    
    @ReactMethod
    fun getState(promise: Promise) {
        val state = protocol?.getState()
        val stateString = when (state) {
            uniffi.offline_protocol.ProtocolState.STOPPED -> "Stopped"
            uniffi.offline_protocol.ProtocolState.RUNNING -> "Running"
            uniffi.offline_protocol.ProtocolState.PAUSED -> "Paused"
            else -> "Stopped"
        }
        promise.resolve(stateString)
    }
    
    // MARK: - Battery Management
    
    @ReactMethod
    fun setBatteryLevel(level: Int, promise: Promise) {
        try {
            protocol?.setBatteryLevel(level.coerceIn(Constants.MIN_BATTERY_LEVEL, Constants.MAX_BATTERY_LEVEL).toUByte())
            promise.resolve(null)
        } catch (e: Exception) {
            promise.reject("ERROR_BATTERY", "Failed to set battery level: ${e.message}", e)
        }
    }
    
    @ReactMethod
    fun getBatteryLevel(promise: Promise) {
        val level = protocol?.getBatteryLevel()
        if (level != null) {
            promise.resolve(level.toInt())
        } else {
            promise.resolve(null)
        }
    }
    
    // MARK: - Relay Management
    
    @ReactMethod
    fun setRelayPriority(priorityString: String, promise: Promise) {
        try {
            val priority = when (priorityString.lowercase()) {
                "low" -> RelayPriority.LOW
                "high" -> RelayPriority.HIGH
                else -> RelayPriority.MEDIUM
            }
            protocol?.setRelayPriority(priority)
            promise.resolve(null)
        } catch (e: Exception) {
            promise.reject("ERROR_RELAY", "Failed to set relay priority: ${e.message}", e)
        }
    }
    
    @ReactMethod
    fun getRelayPriority(promise: Promise) {
        try {
            val priority = protocol?.getRelayPriority()
            val priorityString = when (priority) {
                RelayPriority.LOW -> "low"
                RelayPriority.HIGH -> "high"
                else -> "medium"
            }
            promise.resolve(priorityString)
        } catch (e: Exception) {
            promise.resolve("medium")
        }
    }
    
    @ReactMethod
    fun isRelay(promise: Promise) {
        val isRelay = protocol?.isRelay() ?: false
        promise.resolve(isRelay)
    }
    
    // MARK: - Transport Metrics
    
    @ReactMethod
    fun getTransportMetrics(transportType: String, promise: Promise) {
        try {
            val type = when (transportType.lowercase()) {
                "ble" -> TransportType.BLE
                "wifidirect" -> TransportType.WI_FI_DIRECT
                "internet" -> TransportType.INTERNET
                else -> TransportType.BLE
            }
            
            val metrics = protocol?.getTransportMetrics(type)
            if (metrics != null) {
                val map = Arguments.createMap()
                map.putInt("packetsSent", metrics.packetsSent.toInt())
                map.putInt("packetsReceived", metrics.packetsReceived.toInt())
                map.putInt("bytesSent", metrics.bytesSent.toInt())
                map.putInt("bytesReceived", metrics.bytesReceived.toInt())
                map.putDouble("errorRate", metrics.errorRate.toDouble())
                map.putInt("avgLatencyMs", metrics.avgLatencyMs.toInt())
                promise.resolve(map)
            } else {
                promise.resolve(null)
            }
        } catch (e: Exception) {
            promise.reject("ERROR_METRICS", "Failed to get transport metrics: ${e.message}", e)
        }
    }
    
    // MARK: - Manual Transport Control
    
    @ReactMethod
    fun forceTransport(transportType: String, promise: Promise) {
        try {
            val type = when (transportType.lowercase()) {
                "ble" -> TransportType.BLE
                "wifidirect" -> TransportType.WI_FI_DIRECT
                "internet" -> TransportType.INTERNET
                else -> TransportType.BLE
            }
            
            protocol?.forceTransport(type)
            promise.resolve(null)
        } catch (e: Exception) {
            promise.reject("ERROR_TRANSPORT", "Failed to force transport: ${e.message}", e)
        }
    }
    
    @ReactMethod
    fun releaseTransportLock(promise: Promise) {
        protocol?.releaseTransportLock()
        promise.resolve(null)
    }
    
    // MARK: - DORS Configuration
    
    @ReactMethod
    fun updateDorsConfig(configJson: String, promise: Promise) {
        try {
            val json = JSONObject(configJson)
            val dorsConfig = DorsConfig(
                preferOnline = json.optBoolean("preferOnline", false),
                switchHysteresis = json.optDouble("switchHysteresis", 15.0).toFloat().coerceAtLeast(0f),
                switchCooldownSecs = json.optLong("switchCooldownSecs", 20).coerceAtLeast(0).toULong(),
                bleToWifiRetryThreshold = json.optInt("bleToWifiRetryThreshold", 2).toUInt(),
                minSuccessRateBeforeEscalation = json.optDouble("minSuccessRateBeforeEscalation", 0.3).toFloat().coerceIn(0f, 1f),
                minBleSamplesBeforeSuccessRateEscalation = json.optLong("minBleSamplesBeforeSuccessRateEscalation", 5).coerceAtLeast(0).toULong(),
                rssiSwitchThreshold = json.optInt("rssiSwitchThreshold", Constants.DEFAULT_RSSI_THRESHOLD.toInt()).toShort(),
                congestionQueueThreshold = json.optLong("congestionQueueThreshold", Constants.DEFAULT_CONGESTION_QUEUE).toULong(),
                stabilityWindowSecs = json.optLong("stabilityWindowSecs", Constants.DEFAULT_STABILITY_WINDOW).toULong(),
                poorSignalDurationSecs = json.optLong("poorSignalDurationSecs", 10).toULong(),
                ttlEscalationThreshold = json.optInt("ttlEscalationThreshold", 2).toUByte(),
                congestionDurationSecs = json.optLong("congestionDurationSecs", 10).coerceAtLeast(0).toULong(),
                ttlEscalationHoldSecs = json.optLong("ttlEscalationHoldSecs", 20).coerceAtLeast(1).toULong(),
                historyWindowSize = json.optLong("historyWindowSize", 10).let { max(Constants.MIN_HISTORY_WINDOW, min(Constants.MAX_HISTORY_WINDOW, it)) }.toULong(),
                queueRecoveryRatio = json.optDouble("queueRecoveryRatio", Constants.DEFAULT_QUEUE_RECOVERY_RATIO.toDouble()).toFloat().coerceIn(0f, 1f)
            )
            
            protocol?.updateDorsConfig(dorsConfig)
            promise.resolve(null)
        } catch (e: Exception) {
            promise.reject("ERROR_CONFIG", "Failed to update DORS config: ${e.message}", e)
        }
    }
    
    @ReactMethod
    fun getDorsConfig(promise: Promise) {
        try {
            val config = protocol?.getDorsConfig()
            if (config != null) {
                // DORS config fields are u64 in the core but serialized as Int for the RN bridge; values must be in Int range (e.g. minBleSamplesBeforeSuccessRateEscalation, sample counts).
                val map = Arguments.createMap()
                map.putBoolean("preferOnline", config.preferOnline)
                map.putDouble("switchHysteresis", config.switchHysteresis.toDouble())
                map.putInt("switchCooldownSecs", config.switchCooldownSecs.toInt())
                map.putInt("bleToWifiRetryThreshold", config.bleToWifiRetryThreshold.toInt())
                map.putDouble("minSuccessRateBeforeEscalation", config.minSuccessRateBeforeEscalation.toDouble())
                map.putInt("minBleSamplesBeforeSuccessRateEscalation", config.minBleSamplesBeforeSuccessRateEscalation.toInt())
                map.putInt("rssiSwitchThreshold", config.rssiSwitchThreshold.toInt())
                map.putInt("congestionQueueThreshold", config.congestionQueueThreshold.toInt())
                map.putInt("stabilityWindowSecs", config.stabilityWindowSecs.toInt())
                map.putInt("poorSignalDurationSecs", config.poorSignalDurationSecs.toInt())
                map.putInt("ttlEscalationThreshold", config.ttlEscalationThreshold.toInt())
                map.putInt("congestionDurationSecs", config.congestionDurationSecs.toInt())
                map.putInt("ttlEscalationHoldSecs", config.ttlEscalationHoldSecs.toInt())
                map.putInt("historyWindowSize", config.historyWindowSize.toInt())
                map.putDouble("queueRecoveryRatio", config.queueRecoveryRatio.toDouble())
                promise.resolve(map)
            } else {
                promise.resolve(null)
            }
        } catch (e: Exception) {
            promise.reject("ERROR_CONFIG", "Failed to get DORS config: ${e.message}", e)
        }
    }
    
    // MARK: - Reliability Configuration
    
    @ReactMethod
    fun updateAckConfig(configJson: String, promise: Promise) {
        try {
            val json = JSONObject(configJson)
            val ackConfig = AckConfig(
                defaultTimeoutMs = json.optLong("defaultTimeoutMs", 10000).toULong(),
                maxPendingAcks = json.optLong("maxPendingAcks", 1000).toULong()
            )
            
            protocol?.updateAckConfig(ackConfig)
            promise.resolve(null)
        } catch (e: Exception) {
            promise.reject("ERROR_CONFIG", "Failed to update ACK config: ${e.message}", e)
        }
    }
    
    @ReactMethod
    fun updateRetryConfig(configJson: String, promise: Promise) {
        try {
            val json = JSONObject(configJson)
            // Fallbacks must mirror the Rust defaults in
            // offline-protocol-reliability/src/constants.rs: a partial config
            // rebuilds the whole RetryConfig, so a stale fallback silently
            // overrides the SDK default for every field the app didn't set.
            val retryConfig = RetryConfig(
                maxRetries = json.optInt("maxRetries", 10).toUInt(),
                initialDelayMs = json.optLong("initialDelayMs", 1000).toULong(),
                maxDelayMs = json.optLong("maxDelayMs", 300000).toULong(),
                backoffMultiplier = json.optDouble("backoffMultiplier", 2.0).toFloat(),
                outboxMaxLifetimeMs = json.optLong("outboxMaxLifetimeMs", 604800000).toULong(),
                pendingMessageMaxLifetimeMs =
                    json.optLong("pendingMessageMaxLifetimeMs", 604800000).toULong()
            )
            
            protocol?.updateRetryConfig(retryConfig)
            promise.resolve(null)
        } catch (e: Exception) {
            promise.reject("ERROR_CONFIG", "Failed to update retry config: ${e.message}", e)
        }
    }
    
    @ReactMethod
    fun updateDedupConfig(configJson: String, promise: Promise) {
        try {
            val json = JSONObject(configJson)
            val dedupConfig = DedupConfig(
                maxTrackedMessages = json.optLong("maxTrackedMessages", 10000).toULong(),
                retentionTimeSecs = json.optLong("retentionTimeSecs", 3600).toULong()
            )
            
            protocol?.updateDedupConfig(dedupConfig)
            promise.resolve(null)
        } catch (e: Exception) {
            promise.reject("ERROR_CONFIG", "Failed to update dedup config: ${e.message}", e)
        }
    }
    
    @ReactMethod
    fun getDedupStats(promise: Promise) {
        try {
            val stats = protocol?.getDedupStats()
            if (stats != null) {
                val map = Arguments.createMap()
                map.putDouble("totalTracked", stats.totalTracked.toDouble())
                map.putDouble("recentTracked", stats.recentTracked.toDouble())
                map.putInt("capacityUsedPercent", stats.capacityUsedPercent.toInt())
                map.putString("mode", stats.mode)
                promise.resolve(map)
            } else {
                promise.resolve(null)
            }
        } catch (e: Exception) {
            promise.reject("ERROR_STATS", "Failed to get dedup stats: ${e.message}", e)
        }
    }
    
    @ReactMethod
    fun getPendingAckCount(promise: Promise) {
        try {
            val count = protocol?.getPendingAckCount() ?: 0UL
            promise.resolve(count.toDouble())
        } catch (e: Exception) {
            promise.reject("ERROR_STATS", "Failed to get pending ACK count: ${e.message}", e)
        }
    }
    
    @ReactMethod
    fun getRetryQueueSize(promise: Promise) {
        try {
            val size = protocol?.getRetryQueueSize() ?: 0UL
            promise.resolve(size.toDouble())
        } catch (e: Exception) {
            promise.reject("ERROR_STATS", "Failed to get retry queue size: ${e.message}", e)
        }
    }

    // ========================================================================
    // GRADIENT ROUTING
    // ========================================================================

    @ReactMethod
    fun learnRoute(destination: String, nextHop: String, hopCount: Int, quality: Double, sequenceNumber: Int, promise: Promise) {
        try {
            val proto = protocol ?: throw IllegalStateException("Protocol not initialized")
            proto.learnRoute(
                destination,
                nextHop,
                hopCount.coerceIn(0, 255).toUByte(),
                quality.toFloat(),
                sequenceNumber.coerceAtLeast(0).toUInt()
            )
            promise.resolve(null)
        } catch (e: Exception) {
            promise.reject("ERROR_ROUTING", "Failed to learn route: ${e.message}", e)
        }
    }

    @ReactMethod
    fun getBestRoute(destination: String, promise: Promise) {
        try {
            val proto = protocol ?: throw IllegalStateException("Protocol not initialized")
            val route = proto.getBestRoute(destination)
            if (route != null) {
                val map = Arguments.createMap().apply {
                    putString("nextHop", route.nextHop)
                    putInt("hopCount", route.hopCount.toInt())
                    putDouble("quality", route.quality.toDouble())
                    putDouble("lastSeenMs", route.lastSeenMs.toDouble())
                }
                promise.resolve(map)
            } else {
                promise.resolve(null)
            }
        } catch (e: Exception) {
            promise.reject("ERROR_ROUTING", "Failed to get best route: ${e.message}", e)
        }
    }

    @ReactMethod
    fun getAllRoutes(destination: String, promise: Promise) {
        try {
            val proto = protocol ?: throw IllegalStateException("Protocol not initialized")
            val routes = proto.getAllRoutes(destination)
            val array = Arguments.createArray()
            routes.forEach { route ->
                val map = Arguments.createMap().apply {
                    putString("nextHop", route.nextHop)
                    putInt("hopCount", route.hopCount.toInt())
                    putDouble("quality", route.quality.toDouble())
                    putDouble("lastSeenMs", route.lastSeenMs.toDouble())
                }
                array.pushMap(map)
            }
            promise.resolve(array)
        } catch (e: Exception) {
            promise.reject("ERROR_ROUTING", "Failed to get all routes: ${e.message}", e)
        }
    }

    @ReactMethod
    fun hasRoute(destination: String, promise: Promise) {
        try {
            val proto = protocol ?: throw IllegalStateException("Protocol not initialized")
            val exists = proto.hasRoute(destination)
            promise.resolve(exists)
        } catch (e: Exception) {
            promise.reject("ERROR_ROUTING", "Failed to check route: ${e.message}", e)
        }
    }

    @ReactMethod
    fun removeNeighborRoutes(neighborId: String, promise: Promise) {
        try {
            val proto = protocol ?: throw IllegalStateException("Protocol not initialized")
            proto.removeNeighborRoutes(neighborId)
            promise.resolve(null)
        } catch (e: Exception) {
            promise.reject("ERROR_ROUTING", "Failed to remove neighbor routes: ${e.message}", e)
        }
    }

    @ReactMethod
    fun cleanupExpiredRoutes(promise: Promise) {
        try {
            val proto = protocol ?: throw IllegalStateException("Protocol not initialized")
            proto.cleanupExpiredRoutes()
            promise.resolve(null)
        } catch (e: Exception) {
            promise.reject("ERROR_ROUTING", "Failed to cleanup expired routes: ${e.message}", e)
        }
    }

    @ReactMethod
    fun getRoutingStats(promise: Promise) {
        try {
            val proto = protocol ?: throw IllegalStateException("Protocol not initialized")
            val stats = proto.getRoutingStats()
            val map = Arguments.createMap().apply {
                putInt("destinationCount", stats.destinationCount.toInt())
                putInt("routeCount", stats.routeCount.toInt())
            }
            promise.resolve(map)
        } catch (e: Exception) {
            promise.reject("ERROR_ROUTING", "Failed to get routing stats: ${e.message}", e)
        }
    }

    @ReactMethod
    fun updateRoutingConfig(configJson: String, promise: Promise) {
        try {
            val proto = protocol ?: throw IllegalStateException("Protocol not initialized")
            val json = JSONObject(configJson)
            val routingConfig = GradientRoutingConfig(
                maxRoutesPerDestination = json.optInt("maxRoutesPerDestination", 3).toUInt(),
                routeTtlSecs = json.optLong("routeTtlSecs", 300).toULong(),
                maxRoutingTableSize = json.optInt("maxRoutingTableSize", 1000).toUInt()
            )
            proto.updateRoutingConfig(routingConfig)
            promise.resolve(null)
        } catch (e: Exception) {
            promise.reject("ERROR_ROUTING", "Failed to update routing config: ${e.message}", e)
        }
    }

    // ========================================================================
    // DORS DECISION SUPPORT
    // ========================================================================

    @ReactMethod
    fun shouldEscalateToWifi(promise: Promise) {
        try {
            val proto = protocol ?: throw IllegalStateException("Protocol not initialized")
            val shouldEscalate = proto.shouldEscalateToWifi()
            promise.resolve(shouldEscalate)
        } catch (e: Exception) {
            promise.reject("ERROR_DORS", "Failed to check escalation: ${e.message}", e)
        }
    }

    // ========================================================================
    // FILE TRANSFER OPERATIONS
    // ========================================================================

    @ReactMethod
    fun processFileChunk(fileId: String, chunkIndex: Int, totalChunks: Int, fileSize: Double, fileName: String, fileChecksum: String, data: ReadableArray, promise: Promise) {
        try {
            val proto = protocol ?: throw IllegalStateException("Protocol not initialized")
            val bytes = mutableListOf<UByte>()
            for (i in 0 until data.size()) {
                bytes.add(data.getInt(i).toUByte())
            }
            proto.processFileChunk(fileId, chunkIndex.toUInt(), totalChunks.toUInt(), fileSize.toULong(), fileName, fileChecksum, bytes)
            promise.resolve(null)
        } catch (e: Exception) {
            rejectWithProtocolError(promise, e, "ERROR_FILE", "Failed to process file chunk")
        }
    }

    @ReactMethod
    fun finalizeFile(fileId: String, promise: Promise) {
        try {
            val proto = protocol ?: throw IllegalStateException("Protocol not initialized")
            proto.finalizeFile(fileId)
            promise.resolve(null)
        } catch (e: ProtocolException) {
            promise.reject("ERROR_FILE", "Failed to finalize file: ${e.message}", e)
        } catch (e: Exception) {
            promise.reject("ERROR_FILE", "Failed to finalize file: ${e.message}", e)
        }
    }

    // ========================================================================
    // WIFI DIRECT TRANSPORT METHODS
    // ========================================================================

    @ReactMethod
    fun wifiDirectStatusChanged(isConnected: Boolean, promise: Promise) {
        try {
            val proto = protocol ?: throw IllegalStateException("Protocol not initialized")
            proto.wifiDirectStatusChanged(isConnected)
            promise.resolve(null)
        } catch (e: ProtocolException) {
            promise.reject("ERROR_WIFI_DIRECT", "WiFi Direct status changed failed: ${e.message}", e)
        } catch (e: Exception) {
            promise.reject("ERROR_WIFI_DIRECT", "WiFi Direct status changed failed: ${e.message}", e)
        }
    }

    @ReactMethod
    fun wifiDirectMessageReceived(senderId: String, data: ReadableArray, promise: Promise) {
        try {
            val proto = protocol ?: throw IllegalStateException("Protocol not initialized")
            val bytes = mutableListOf<UByte>()
            for (i in 0 until data.size()) {
                bytes.add(data.getInt(i).toUByte())
            }
            proto.wifiDirectMessageReceived(senderId, bytes)
            promise.resolve(null)
        } catch (e: ProtocolException) {
            promise.reject("ERROR_WIFI_DIRECT", "WiFi Direct message received failed: ${e.message}", e)
        } catch (e: Exception) {
            promise.reject("ERROR_WIFI_DIRECT", "WiFi Direct message received failed: ${e.message}", e)
        }
    }

    @ReactMethod
    fun wifiDirectGetNextMessage(promise: Promise) {
        try {
            val proto = protocol ?: throw IllegalStateException("Protocol not initialized")
            val message = proto.wifiDirectGetNextMessage()
            if (message != null) {
                val map = Arguments.createMap()
                map.putString("recipientId", message.recipientId)
                val array = Arguments.createArray()
                message.data.forEach { array.pushInt(it.toInt()) }
                map.putArray("data", array)
                promise.resolve(map)
            } else {
                promise.resolve(null)
            }
        } catch (e: Exception) {
            promise.reject("ERROR_WIFI_DIRECT", "WiFi Direct get next message failed: ${e.message}", e)
        }
    }

    @ReactMethod
    fun wifiDirectPeerConnected(peerId: String, promise: Promise) {
        try {
            val proto = protocol ?: throw IllegalStateException("Protocol not initialized")
            proto.wifiDirectPeerConnected(peerId)
            promise.resolve(null)
        } catch (e: ProtocolException) {
            promise.reject("ERROR_WIFI_DIRECT", "WiFi Direct peer connected failed: ${e.message}", e)
        } catch (e: Exception) {
            promise.reject("ERROR_WIFI_DIRECT", "WiFi Direct peer connected failed: ${e.message}", e)
        }
    }

    @ReactMethod
    fun wifiDirectPeerDisconnected(peerId: String, promise: Promise) {
        try {
            val proto = protocol ?: throw IllegalStateException("Protocol not initialized")
            proto.wifiDirectPeerDisconnected(peerId)
            promise.resolve(null)
        } catch (e: ProtocolException) {
            promise.reject("ERROR_WIFI_DIRECT", "WiFi Direct peer disconnected failed: ${e.message}", e)
        } catch (e: Exception) {
            promise.reject("ERROR_WIFI_DIRECT", "WiFi Direct peer disconnected failed: ${e.message}", e)
        }
    }

    // ========================================================================
    // INTERNET TRANSPORT METHODS
    // ========================================================================

    @ReactMethod
    fun internetStatusChanged(isConnected: Boolean, promise: Promise) {
        try {
            val proto = protocol ?: throw IllegalStateException("Protocol not initialized")
            proto.internetStatusChanged(isConnected)
            promise.resolve(null)
        } catch (e: ProtocolException) {
            promise.reject("ERROR_INTERNET", "Internet status changed failed: ${e.message}", e)
        } catch (e: Exception) {
            promise.reject("ERROR_INTERNET", "Internet status changed failed: ${e.message}", e)
        }
    }

    @ReactMethod
    fun internetMessageReceived(senderId: String, data: ReadableArray, promise: Promise) {
        try {
            val proto = protocol ?: throw IllegalStateException("Protocol not initialized")
            val bytes = mutableListOf<UByte>()
            for (i in 0 until data.size()) {
                bytes.add(data.getInt(i).toUByte())
            }
            proto.internetMessageReceived(senderId, bytes)
            promise.resolve(null)
        } catch (e: ProtocolException) {
            promise.reject("ERROR_INTERNET", "Internet message received failed: ${e.message}", e)
        } catch (e: Exception) {
            promise.reject("ERROR_INTERNET", "Internet message received failed: ${e.message}", e)
        }
    }

    @ReactMethod
    fun internetGetNextMessage(promise: Promise) {
        try {
            val proto = protocol ?: throw IllegalStateException("Protocol not initialized")
            val message = proto.internetGetNextMessage()
            if (message != null) {
                val map = Arguments.createMap()
                map.putString("messageId", message.messageId)
                map.putString("recipientId", message.recipientId)
                val array = Arguments.createArray()
                message.data.forEach { array.pushInt(it.toInt()) }
                map.putArray("data", array)
                promise.resolve(map)
            } else {
                promise.resolve(null)
            }
        } catch (e: Exception) {
            promise.reject("ERROR_INTERNET", "Internet get next message failed: ${e.message}", e)
        }
    }

    @ReactMethod
    fun internetConfirmSent(messageId: String, promise: Promise) {
        try {
            val proto = protocol ?: throw IllegalStateException("Protocol not initialized")
            proto.internetConfirmSent(messageId)
            promise.resolve(null)
        } catch (e: Exception) {
            promise.reject("ERROR_INTERNET", "Internet confirm sent failed: ${e.message}", e)
        }
    }

    @ReactMethod
    fun internetSendFailed(messageId: String, promise: Promise) {
        try {
            val proto = protocol ?: throw IllegalStateException("Protocol not initialized")
            proto.internetSendFailed(messageId)
            promise.resolve(null)
        } catch (e: Exception) {
            promise.reject("ERROR_INTERNET", "Internet send failed report failed: ${e.message}", e)
        }
    }

    // ========================================================================
    // MLS (END-TO-END ENCRYPTION)
    // ========================================================================

    /**
     * Initialize MLS with built-in secure storage (EncryptedSharedPreferences)
     */
    @ReactMethod
    fun initializeMlsWithSecureStorage(promise: Promise) {
        try {
            val proto = protocol ?: throw IllegalStateException("Protocol not initialized")
            val config = currentConfig
                ?: throw IllegalStateException("Protocol config not initialized")
            val accountNamespace = StorageNamespace.account(config.appId, config.userId)
            val secureStorage =
                MlsSecureStorage(reactApplicationContext, accountNamespace)
            val protocolStateStorage =
                AppContainerProtocolStateStorage(
                    reactApplicationContext,
                    accountNamespace
                )
            proto.initializeMls(secureStorage, protocolStateStorage)
            // Either of these means this account is starting from a fresh MLS
            // identity — never let that pass silently.
            when (secureStorage.legacyAdoption) {
                is LegacyStoreAdoption.Decision.Conflict -> emitDiagnostic(
                    "error",
                    "Legacy secure store belongs to another account; this " +
                        "account starts from a fresh MLS identity and cannot " +
                        "decrypt its previous sessions. Its pre-split delivery " +
                        "state is unreachable too, so it also comes up with an " +
                        "empty outbox and an empty block list — every " +
                        "previously blocked peer is unblocked"
                )
                is LegacyStoreAdoption.Decision.ClaimUnverified -> emitDiagnostic(
                    "error",
                    "Could not record this account's claim on the legacy secure " +
                        "store, so it was not adopted: another account could " +
                        "otherwise inherit the same MLS identity. This account " +
                        "starts from a fresh identity and comes up with an empty " +
                        "outbox and an empty block list — every previously " +
                        "blocked peer is unblocked. The credential store is " +
                        "failing writes; retrying on a healthy store adopts " +
                        "normally"
                )
                else -> Unit
            }
            emitDiagnostic(
                "info",
                "MLS initialized with split secure and app-container storage"
            )
            promise.resolve(null)
        } catch (e: Exception) {
            emitDiagnostic("error", "Failed to initialize MLS", mapOf(
                "message" to (e.message ?: "unknown")
            ))
            promise.reject("ERROR_MLS", "Failed to initialize MLS: ${e.message}", e)
        }
    }

    /**
     * Check if MLS is initialized
     */
    @ReactMethod
    fun isMlsInitialized(promise: Promise) {
        val proto = protocol
        if (proto == null) {
            promise.resolve(false)
            return
        }
        promise.resolve(proto.isMlsInitialized())
    }

    // ========================================================================
    // IDENTITY AND SIGNING OPERATIONS
    // ========================================================================

    /**
     * Get the identity public key (Ed25519, 32 bytes)
     */
    @ReactMethod
    fun getIdentityPublicKey(promise: Promise) {
        try {
            val proto = protocol ?: throw IllegalStateException("Protocol not initialized")
            val publicKey = proto.getIdentityPublicKey()
            promise.resolve(Arguments.fromList(publicKey.map { it.toInt() }))
        } catch (e: Exception) {
            promise.reject("ERROR_CRYPTO", "Failed to get identity public key: ${e.message}", e)
        }
    }

    /**
     * Derive a user ID from a public key
     */
    @ReactMethod
    fun deriveUserIdFromPublicKey(publicKey: ReadableArray, promise: Promise) {
        try {
            val proto = protocol ?: throw IllegalStateException("Protocol not initialized")
            val publicKeyBytes = (0 until publicKey.size()).map { publicKey.getInt(it).toUByte() }
            val userId = proto.deriveUserIdFromPublicKey(publicKeyBytes)
            promise.resolve(userId)
        } catch (e: Exception) {
            promise.reject("ERROR_CRYPTO", "Failed to derive user ID: ${e.message}", e)
        }
    }

    /**
     * Sign data with the identity private key
     */
    @ReactMethod
    fun signData(data: ReadableArray, promise: Promise) {
        try {
            val proto = protocol ?: throw IllegalStateException("Protocol not initialized")
            val dataBytes = (0 until data.size()).map { data.getInt(it).toUByte() }
            val signature = proto.signData(dataBytes)
            promise.resolve(Arguments.fromList(signature.map { it.toInt() }))
        } catch (e: Exception) {
            promise.reject("ERROR_CRYPTO", "Failed to sign data: ${e.message}", e)
        }
    }

    /**
     * Verify a signature against a public key
     */
    @ReactMethod
    fun verifySignature(publicKey: ReadableArray, data: ReadableArray, signature: ReadableArray, promise: Promise) {
        try {
            val proto = protocol ?: throw IllegalStateException("Protocol not initialized")
            val publicKeyBytes = (0 until publicKey.size()).map { publicKey.getInt(it).toUByte() }
            val dataBytes = (0 until data.size()).map { data.getInt(it).toUByte() }
            val signatureBytes = (0 until signature.size()).map { signature.getInt(it).toUByte() }
            val isValid = proto.verifySignature(publicKeyBytes, dataBytes, signatureBytes)
            promise.resolve(isValid)
        } catch (e: Exception) {
            promise.reject("ERROR_CRYPTO", "Failed to verify signature: ${e.message}", e)
        }
    }

    /**
     * Generate a new MLS key package
     */
    @ReactMethod
    fun mlsGenerateKeyPackage(promise: Promise) {
        try {
            val proto = protocol ?: throw IllegalStateException("Protocol not initialized")
            val bundle = proto.mlsGenerateKeyPackage()
            val result = Arguments.createMap().apply {
                putString("packageId", bundle.packageId)
                putString("userId", bundle.userId)
                val dataArray = Arguments.createArray()
                bundle.keyPackageData.forEach { dataArray.pushInt(it.toInt()) }
                putArray("keyPackageData", dataArray)
                putDouble("createdAtMs", bundle.createdAtMs.toDouble())
                putDouble("expiresAtMs", bundle.expiresAtMs.toDouble())
                putBoolean("synced", bundle.synced)
            }
            promise.resolve(result)
        } catch (e: Exception) {
            promise.reject("ERROR_MLS", "Failed to generate key package: ${e.message}", e)
        }
    }

    /**
     * Get existing or generate new key package
     */
    @ReactMethod
    fun mlsGetOrCreateKeyPackage(promise: Promise) {
        try {
            val proto = protocol ?: throw IllegalStateException("Protocol not initialized")
            val bundle = proto.mlsGetOrCreateKeyPackage()
            val result = Arguments.createMap().apply {
                putString("packageId", bundle.packageId)
                putString("userId", bundle.userId)
                val dataArray = Arguments.createArray()
                bundle.keyPackageData.forEach { dataArray.pushInt(it.toInt()) }
                putArray("keyPackageData", dataArray)
                putDouble("createdAtMs", bundle.createdAtMs.toDouble())
                putDouble("expiresAtMs", bundle.expiresAtMs.toDouble())
                putBoolean("synced", bundle.synced)
            }
            promise.resolve(result)
        } catch (e: Exception) {
            promise.reject("ERROR_MLS", "Failed to get key package: ${e.message}", e)
        }
    }

    /**
     * Get pending key packages to upload
     */
    @ReactMethod
    fun mlsGetPendingKeyPackages(promise: Promise) {
        try {
            val proto = protocol ?: throw IllegalStateException("Protocol not initialized")
            val bundles = proto.mlsGetPendingKeyPackages()
            val resultArray = Arguments.createArray()
            
            bundles.forEach { bundle ->
                val map = Arguments.createMap().apply {
                    putString("packageId", bundle.packageId)
                    putString("userId", bundle.userId)
                    val dataArray = Arguments.createArray()
                    bundle.keyPackageData.forEach { dataArray.pushInt(it.toInt()) }
                    putArray("keyPackageData", dataArray)
                    putDouble("createdAtMs", bundle.createdAtMs.toDouble())
                    putDouble("expiresAtMs", bundle.expiresAtMs.toDouble())
                    putBoolean("synced", bundle.synced)
                }
                resultArray.pushMap(map)
            }
            
            promise.resolve(resultArray)
        } catch (e: Exception) {
            promise.reject("ERROR_MLS", "Failed to get pending key packages: ${e.message}", e)
        }
    }

    /**
     * Mark key package as synced (uploaded)
     */
    @ReactMethod
    fun mlsMarkKeyPackageSynced(packageId: String, promise: Promise) {
        try {
            val proto = protocol ?: throw IllegalStateException("Protocol not initialized")
            proto.mlsMarkKeyPackageSynced(packageId)
            promise.resolve(null)
        } catch (e: Exception) {
            promise.reject("ERROR_MLS", "Failed to mark key package synced: ${e.message}", e)
        }
    }

    /**
     * Create a 1:1 session (returns Welcome message)
     */
    @ReactMethod
    fun mlsCreateSession(otherUserId: String, promise: Promise) {
        try {
            val proto = protocol ?: throw IllegalStateException("Protocol not initialized")
            val welcome = proto.mlsCreateSession(otherUserId)
            val result = Arguments.createMap().apply {
                putString("groupId", welcome.groupId)
                val welcomeDataArray = Arguments.createArray()
                welcome.welcomeData.forEach { welcomeDataArray.pushInt(it.toInt()) }
                putArray("welcomeData", welcomeDataArray)
                putString("inviterId", welcome.inviterId)
                putString("groupName", welcome.groupName)
                putDouble("timestampMs", welcome.timestampMs.toDouble())
            }
            promise.resolve(result)
        } catch (e: Exception) {
            promise.reject("ERROR_MLS", "Failed to create session: ${e.message}", e)
        }
    }

    /**
     * Join a session from Welcome message
     */
    @ReactMethod
    fun mlsJoinSession(welcomeJson: String, promise: Promise) {
        try {
            val proto = protocol ?: throw IllegalStateException("Protocol not initialized")
            val json = JSONObject(welcomeJson)
            
            val welcomeDataArray = json.optJSONArray("welcomeData") ?: JSONArray()
            val welcomeData = mutableListOf<UByte>()
            for (i in 0 until welcomeDataArray.length()) {
                welcomeData.add(welcomeDataArray.getInt(i).toUByte())
            }
            
            val welcome = MlsWelcomeMessage(
                groupId = json.safeOptString("groupId"),
                welcomeData = welcomeData,
                inviterId = json.safeOptString("inviterId"),
                groupName = json.optNullableString("groupName"),
                timestampMs = json.optLong("timestampMs", 0).toULong()
            )

            val info = proto.mlsJoinSession(welcome)
            val result = Arguments.createMap().apply {
                putString("groupId", info.groupId)
                putString("name", info.name)
                val members = Arguments.createArray()
                info.members.forEach { members.pushString(it) }
                putArray("members", members)
                putDouble("epoch", info.epoch.toDouble())
                putBoolean("isSession", info.isSession)
                putDouble("createdAtMs", info.createdAtMs.toDouble())
                putDouble("lastActivityMs", info.lastActivityMs.toDouble())
            }
            promise.resolve(result)
        } catch (e: Exception) {
            promise.reject("ERROR_MLS", "Failed to join session: ${e.message}", e)
        }
    }

    /**
     * Decrypt a message from a user
     */
    @ReactMethod
    fun mlsDecryptFromUser(encryptedJson: String, promise: Promise) {
        try {
            val proto = protocol ?: throw IllegalStateException("Protocol not initialized")
            val json = JSONObject(encryptedJson)
            
            val ciphertextArray = json.optJSONArray("ciphertext") ?: JSONArray()
            val ciphertext = mutableListOf<UByte>()
            for (i in 0 until ciphertextArray.length()) {
                ciphertext.add(ciphertextArray.getInt(i).toUByte())
            }
            
            val encrypted = MlsEncryptedMessage(
                groupId = json.safeOptString("groupId"),
                messageType = json.safeOptString("messageType", "Application"),
                epoch = json.optLong("epoch", 0).toULong(),
                ciphertext = ciphertext,
                senderId = json.safeOptString("senderId"),
                timestampMs = json.optLong("timestampMs", 0).toULong()
            )

            val plaintext = proto.mlsDecryptFromUser(encrypted)
            if (plaintext != null) {
                val result = Arguments.createArray()
                plaintext.forEach { result.pushInt(it.toInt()) }
                promise.resolve(result)
            } else {
                promise.resolve(null)
            }
        } catch (e: Exception) {
            promise.reject("ERROR_MLS", "Failed to decrypt message from user: ${e.message}", e)
        }
    }

    /**
     * Delete a session
     */
    @ReactMethod
    fun mlsDeleteSession(otherUserId: String, promise: Promise) {
        try {
            val proto = protocol ?: throw IllegalStateException("Protocol not initialized")
            proto.mlsDeleteSession(otherUserId)
            promise.resolve(null)
        } catch (e: Exception) {
            promise.reject("ERROR_MLS", "Failed to delete session: ${e.message}", e)
        }
    }

    /**
     * Import a contact's key package
     */
    @ReactMethod
    fun mlsImportKeyPackage(userId: String, keyPackageData: ReadableArray, promise: Promise) {
        try {
            val proto = protocol ?: throw IllegalStateException("Protocol not initialized")
            val data = mutableListOf<UByte>()
            for (i in 0 until keyPackageData.size()) {
                data.add(keyPackageData.getInt(i).toUByte())
            }
            proto.mlsImportKeyPackage(userId, data)
            promise.resolve(null)
        } catch (e: Exception) {
            promise.reject("ERROR_MLS", "Failed to import key package: ${e.message}", e)
        }
    }

    /**
     * Check if a session exists with a user
     */
    @ReactMethod
    fun mlsHasSession(otherUserId: String, promise: Promise) {
        val proto = protocol
        if (proto == null) {
            promise.resolve(false)
            return
        }
        promise.resolve(proto.mlsHasSession(otherUserId))
    }

    /**
     * Check if a pending key package is available for a peer
     */
    @ReactMethod
    fun hasPendingKeyPackage(peerId: String, promise: Promise) {
        val proto = protocol
        if (proto == null) {
            promise.resolve(false)
            return
        }
        promise.resolve(proto.hasPendingKeyPackage(peerId))
    }

    /**
     * Returns the current session establishment state for a peer.
     */
    @ReactMethod
    fun getEstablishmentState(peerId: String, promise: Promise) {
        val proto = protocol
        if (proto == null) {
            promise.resolve("NoKeyPackage")
            return
        }
        val state = proto.getEstablishmentState(peerId)
        val stateString = when (state) {
            EstablishmentState.NO_KEY_PACKAGE -> "NoKeyPackage"
            EstablishmentState.HAVE_KEY_PACKAGE -> "HaveKeyPackage"
            EstablishmentState.SESSION_PENDING -> "SessionPending"
            EstablishmentState.SESSION_CONFIRMED -> "SessionConfirmed"
        }
        promise.resolve(stateString)
    }

    /**
     * Establish a secure session with a peer (high-level API)
     */
    @ReactMethod
    fun establishSecureSession(peerId: String, promise: Promise) {
        try {
            val proto = protocol ?: throw IllegalStateException("Protocol not initialized")
            val welcome = proto.establishSecureSession(peerId)
            if (welcome != null) {
                val result = Arguments.createMap().apply {
                    putString("groupId", welcome.groupId)
                    val welcomeDataArray = Arguments.createArray()
                    welcome.welcomeData.forEach { welcomeDataArray.pushInt(it.toInt()) }
                    putArray("welcomeData", welcomeDataArray)
                    putString("inviterId", welcome.inviterId)
                    putString("groupName", welcome.groupName)
                    putDouble("timestampMs", welcome.timestampMs.toDouble())
                }
                promise.resolve(result)
            } else {
                // Session already exists
                promise.resolve(null)
            }
        } catch (e: Exception) {
            promise.reject("ERROR_MLS", "Failed to establish secure session: ${e.message}", e)
        }
    }

    /**
     * Encrypt a message for a user
     */
    @ReactMethod
    fun mlsEncryptForUser(otherUserId: String, plaintext: ReadableArray, promise: Promise) {
        try {
            val proto = protocol ?: throw IllegalStateException("Protocol not initialized")
            val data = mutableListOf<UByte>()
            for (i in 0 until plaintext.size()) {
                data.add(plaintext.getInt(i).toUByte())
            }
            val encrypted = proto.mlsEncryptForUser(otherUserId, data)
            val result = Arguments.createMap().apply {
                putString("groupId", encrypted.groupId)
                putString("messageType", encrypted.messageType)
                putDouble("epoch", encrypted.epoch.toDouble())
                val ciphertextArray = Arguments.createArray()
                encrypted.ciphertext.forEach { ciphertextArray.pushInt(it.toInt()) }
                putArray("ciphertext", ciphertextArray)
                putString("senderId", encrypted.senderId)
                putDouble("timestampMs", encrypted.timestampMs.toDouble())
            }
            promise.resolve(result)
        } catch (e: Exception) {
            promise.reject("ERROR_MLS", "Failed to encrypt message: ${e.message}", e)
        }
    }

    /**
     * Decrypt a message
     */
    @ReactMethod
    fun mlsDecrypt(encryptedJson: String, promise: Promise) {
        try {
            val proto = protocol ?: throw IllegalStateException("Protocol not initialized")
            val json = JSONObject(encryptedJson)
            
            val ciphertextArray = json.optJSONArray("ciphertext") ?: JSONArray()
            val ciphertext = mutableListOf<UByte>()
            for (i in 0 until ciphertextArray.length()) {
                ciphertext.add(ciphertextArray.getInt(i).toUByte())
            }
            
            val encrypted = MlsEncryptedMessage(
                groupId = json.safeOptString("groupId"),
                messageType = json.safeOptString("messageType", "Application"),
                epoch = json.optLong("epoch", 0).toULong(),
                ciphertext = ciphertext,
                senderId = json.safeOptString("senderId"),
                timestampMs = json.optLong("timestampMs", 0).toULong()
            )

            val plaintext = proto.mlsDecrypt(encrypted)
            if (plaintext != null) {
                val result = Arguments.createArray()
                plaintext.forEach { result.pushInt(it.toInt()) }
                promise.resolve(result)
            } else {
                promise.resolve(null)
            }
        } catch (e: Exception) {
            promise.reject("ERROR_MLS", "Failed to decrypt message: ${e.message}", e)
        }
    }

    /**
     * List all active sessions
     */
    @ReactMethod
    fun mlsListSessions(promise: Promise) {
        val proto = protocol
        if (proto == null) {
            promise.resolve(Arguments.createArray())
            return
        }
        val sessions = proto.mlsListSessions()
        promise.resolve(Arguments.fromList(sessions))
    }

    /**
     * Process a Welcome message
     */
    @ReactMethod
    fun mlsProcessWelcome(welcomeJson: String, promise: Promise) {
        try {
            val proto = protocol ?: throw IllegalStateException("Protocol not initialized")
            val json = JSONObject(welcomeJson)
            
            val welcomeDataArray = json.optJSONArray("welcomeData") ?: JSONArray()
            val welcomeData = mutableListOf<UByte>()
            for (i in 0 until welcomeDataArray.length()) {
                welcomeData.add(welcomeDataArray.getInt(i).toUByte())
            }
            
            val welcome = MlsWelcomeMessage(
                groupId = json.safeOptString("groupId"),
                welcomeData = welcomeData,
                inviterId = json.safeOptString("inviterId"),
                groupName = json.optNullableString("groupName"),
                timestampMs = json.optLong("timestampMs", 0).toULong()
            )

            val info = proto.mlsProcessWelcome(welcome)
            val result = Arguments.createMap().apply {
                putString("groupId", info.groupId)
                putString("name", info.name)
                val members = Arguments.createArray()
                info.members.forEach { members.pushString(it) }
                putArray("members", members)
                putDouble("epoch", info.epoch.toDouble())
                putBoolean("isSession", info.isSession)
                putDouble("createdAtMs", info.createdAtMs.toDouble())
                putDouble("lastActivityMs", info.lastActivityMs.toDouble())
            }
            promise.resolve(result)
        } catch (e: Exception) {
            promise.reject("ERROR_MLS", "Failed to process welcome message: ${e.message}", e)
        }
    }

    // ========================================================================
    // GROUP MANAGEMENT (MESH / MLS PROTOCOL-LEVEL)
    // ========================================================================

    /**
     * Create a new MLS group via the mesh transport.
     */
    @ReactMethod
    fun meshCreateGroup(name: String, promise: Promise) {
        try {
            val proto = protocol ?: throw IllegalStateException("Protocol not initialized")
            val info = proto.createGroup(name)
            val result = Arguments.createMap().apply {
                putString("groupId", info.groupId)
                putString("name", info.name)
                val members = Arguments.createArray()
                info.members.forEach { members.pushString(it) }
                putArray("members", members)
                putDouble("epoch", info.epoch.toDouble())
                putBoolean("isSession", info.isSession)
                putDouble("createdAtMs", info.createdAtMs.toDouble())
                putDouble("lastActivityMs", info.lastActivityMs.toDouble())
            }
            promise.resolve(result)
        } catch (e: Exception) {
            rejectWithProtocolError(promise, e, "ERROR_MESH_GROUP", "Failed to create mesh group")
        }
    }

    /**
     * Invite a user to a mesh group (sends Welcome+Commit to peer).
     */
    @ReactMethod
    fun meshInviteToGroup(groupId: String, inviteeUserId: String, promise: Promise) {
        try {
            val proto = protocol ?: throw IllegalStateException("Protocol not initialized")
            proto.inviteToGroup(groupId, inviteeUserId)
            promise.resolve(null)
        } catch (e: Exception) {
            rejectWithProtocolError(promise, e, "ERROR_MESH_GROUP", "Failed to invite to mesh group")
        }
    }

    /**
     * Send an MLS-encrypted message to all members of a mesh group.
     */
    @ReactMethod
    fun meshSendGroupMessage(groupId: String, content: String, priority: String?, replyToMsg: String?, promise: Promise) {
        try {
            val proto = protocol ?: throw IllegalStateException("Protocol not initialized")
            val msgPriority = priority?.let {
                when (it.lowercase()) {
                    "low" -> MessagePriority.LOW
                    "medium" -> MessagePriority.MEDIUM
                    "high" -> MessagePriority.HIGH
                    "critical" -> MessagePriority.CRITICAL
                    else -> null
                }
            }
            val messageIds = proto.sendGroupMessage(groupId, content, msgPriority, replyToMsg)
            val result = Arguments.createArray()
            messageIds.forEach { result.pushString(it) }
            promise.resolve(result)
        } catch (e: Exception) {
            rejectWithProtocolError(promise, e, "ERROR_MESH_GROUP", "Failed to send mesh group message")
        }
    }

    /**
     * Forward a message to all members of a mesh group with forwarding attribution.
     */
    @ReactMethod
    fun meshForwardMessageToGroup(originalMessageJson: String, groupId: String, priority: String?, promise: Promise) {
        try {
            val proto = protocol ?: throw IllegalStateException("Protocol not initialized")
            val msgPriority = priority?.let {
                when (it.lowercase()) {
                    "low" -> MessagePriority.LOW
                    "medium" -> MessagePriority.MEDIUM
                    "high" -> MessagePriority.HIGH
                    "critical" -> MessagePriority.CRITICAL
                    else -> null
                }
            }
            val messageIds = proto.forwardMessageToGroup(originalMessageJson, groupId, msgPriority)
            val result = Arguments.createArray()
            messageIds.forEach { result.pushString(it) }
            promise.resolve(result)
        } catch (e: Exception) {
            rejectWithProtocolError(promise, e, "ERROR_MESH_GROUP", "Failed to forward message to group")
        }
    }

    /**
     * Remove a member from a mesh group with notification.
     */
    @ReactMethod
    fun meshRemoveFromGroup(groupId: String, memberId: String, promise: Promise) {
        try {
            val proto = protocol ?: throw IllegalStateException("Protocol not initialized")
            proto.removeFromGroup(groupId, memberId)
            promise.resolve(null)
        } catch (e: Exception) {
            rejectWithProtocolError(promise, e, "ERROR_MESH_GROUP", "Failed to remove from mesh group")
        }
    }

    /**
     * Leave a mesh group with notification.
     */
    @ReactMethod
    fun meshLeaveGroup(groupId: String, promise: Promise) {
        try {
            val proto = protocol ?: throw IllegalStateException("Protocol not initialized")
            proto.leaveGroup(groupId)
            promise.resolve(null)
        } catch (e: Exception) {
            rejectWithProtocolError(promise, e, "ERROR_MESH_GROUP", "Failed to leave mesh group")
        }
    }

    /**
     * List all mesh groups.
     */
    @ReactMethod
    fun meshListGroups(promise: Promise) {
        try {
            val proto = protocol ?: throw IllegalStateException("Protocol not initialized")
            val groups = proto.listGroups()
            val result = Arguments.createArray()
            groups.forEach { result.pushString(it) }
            promise.resolve(result)
        } catch (e: Exception) {
            rejectWithProtocolError(promise, e, "ERROR_MESH_GROUP", "Failed to list mesh groups")
        }
    }

    /**
     * Get group information via high-level API.
     */
    @ReactMethod
    fun meshGetGroupInfo(groupId: String, promise: Promise) {
        try {
            val proto = protocol ?: throw IllegalStateException("Protocol not initialized")
            val info = proto.getGroupInfo(groupId)
            if (info != null) {
                val result = Arguments.createMap().apply {
                    putString("groupId", info.groupId)
                    putString("name", info.name)
                    val members = Arguments.createArray()
                    info.members.forEach { members.pushString(it) }
                    putArray("members", members)
                    putDouble("epoch", info.epoch.toDouble())
                    putBoolean("isSession", info.isSession)
                    putDouble("createdAtMs", info.createdAtMs.toDouble())
                    putDouble("lastActivityMs", info.lastActivityMs.toDouble())
                }
                promise.resolve(result)
            } else {
                promise.resolve(null)
            }
        } catch (e: Exception) {
            rejectWithProtocolError(promise, e, "ERROR_MESH_GROUP", "Failed to get group info")
        }
    }

    /**
     * Whether a rich group send right now would seal its extras, and which
     * members hold the gate closed (point-in-time, advisory).
     */
    @ReactMethod
    fun meshGroupRichReadiness(groupId: String, promise: Promise) {
        try {
            val proto = protocol ?: throw IllegalStateException("Protocol not initialized")
            val readiness = proto.groupRichReadiness(groupId)
            val result = Arguments.createMap().apply {
                putBoolean("ready", readiness.ready)
                val unknownMembers = Arguments.createArray()
                readiness.unknownMembers.forEach { unknownMembers.pushString(it) }
                putArray("unknownMembers", unknownMembers)
            }
            promise.resolve(result)
        } catch (e: Exception) {
            rejectWithProtocolError(promise, e, "ERROR_MESH_GROUP", "Failed to get group rich readiness")
        }
    }

    /**
     * Relay-side registration state of a group ('synced' | 'pending' |
     * 'unsynced'). Point-in-time; transitions surface as
     * `group_relay_sync_changed` events.
     */
    @ReactMethod
    fun meshGroupRelaySyncState(groupId: String, promise: Promise) {
        try {
            val proto = protocol ?: throw IllegalStateException("Protocol not initialized")
            promise.resolve(proto.groupRelaySyncState(groupId).name.lowercase())
        } catch (e: Exception) {
            rejectWithProtocolError(promise, e, "ERROR_MESH_GROUP", "Failed to get relay sync state")
        }
    }

    /**
     * Registers (or re-registers) a group with the relay server on demand —
     * the supported path before relay-dependent raw server commands
     * (invite links). Outcome arrives as `group_relay_sync_changed`;
     * resolves true when the frame was queued or the group is already
     * synced, false when relay grouping is disabled or Internet is down.
     */
    @ReactMethod
    fun meshRequestGroupRelayRegistration(groupId: String, promise: Promise) {
        try {
            val proto = protocol ?: throw IllegalStateException("Protocol not initialized")
            promise.resolve(proto.requestGroupRelayRegistration(groupId))
        } catch (e: Exception) {
            rejectWithProtocolError(promise, e, "ERROR_MESH_GROUP", "Failed to request relay registration")
        }
    }

    @ReactMethod
    fun meshSetMemberRole(groupId: String, userId: String, role: String, promise: Promise) {
        try {
            val proto = protocol ?: throw IllegalStateException("Protocol not initialized")
            proto.setMemberRole(groupId, userId, role)
            promise.resolve(null)
        } catch (e: Exception) {
            rejectWithProtocolError(promise, e, "ERROR_MESH_GROUP", "Failed to set member role")
        }
    }

    @ReactMethod
    fun meshGetMemberRole(groupId: String, userId: String, promise: Promise) {
        try {
            val proto = protocol ?: throw IllegalStateException("Protocol not initialized")
            val role = proto.getMemberRole(groupId, userId)
            promise.resolve(role)
        } catch (e: Exception) {
            rejectWithProtocolError(promise, e, "ERROR_MESH_GROUP", "Failed to get member role")
        }
    }

    @ReactMethod
    fun meshGetGroupRoles(groupId: String, promise: Promise) {
        try {
            val proto = protocol ?: throw IllegalStateException("Protocol not initialized")
            val roles = proto.getGroupRoles(groupId)
            val result = Arguments.createMap()
            roles.forEach { (userId, role) -> result.putString(userId, role) }
            promise.resolve(result)
        } catch (e: Exception) {
            rejectWithProtocolError(promise, e, "ERROR_MESH_GROUP", "Failed to get group roles")
        }
    }

    @ReactMethod
    fun meshRenameGroup(groupId: String, newName: String, promise: Promise) {
        try {
            val proto = protocol ?: throw IllegalStateException("Protocol not initialized")
            proto.renameGroup(groupId, newName)
            promise.resolve(null)
        } catch (e: Exception) {
            rejectWithProtocolError(promise, e, "ERROR_MESH_GROUP", "Failed to rename group")
        }
    }

    /**
     * One-shot relay presence query for a peer (last-seen display, pre-send
     * checks). Fire-and-event: resolves true if the query was sent; the
     * answer arrives as the SDK's `presence_updated` event.
     * `options.force` parks the query through the chat-open reconnect
     * window instead of failing fast (see InternetManager.checkPresence).
     */
    @ReactMethod
    fun checkInternetPresence(userId: String, options: ReadableMap, promise: Promise) {
        try {
            val manager = internetManager ?: throw IllegalStateException("Internet transport not initialized")
            val force = options.hasKey("force") && options.getBoolean("force")
            manager.checkPresence(userId, force) { written -> promise.resolve(written) }
        } catch (e: Exception) {
            rejectWithProtocolError(promise, e, "ERROR_INTERNET", "Failed to query presence")
        }
    }

    /**
     * Sends a raw, caller-built relay command verbatim over the SDK's
     * socket (invite-link lifecycle and other server-plane ops). Resolves
     * true when written; false when invalid JSON or not connected and
     * authenticated. Responses arrive as `internet_server_message` events.
     */
    @ReactMethod
    fun internetSendRawCommand(json: String, promise: Promise) {
        try {
            val manager = internetManager ?: throw IllegalStateException("Internet transport not initialized")
            promise.resolve(manager.sendRawCommand(json))
        } catch (e: Exception) {
            rejectWithProtocolError(promise, e, "ERROR_INTERNET", "Failed to send raw server command")
        }
    }

    /**
     * Whether the internet socket is connected AND relay-authenticated —
     * the same gate `internetSendRawCommand` checks. Point-in-time;
     * transitions arrive as `internet_status_changed` events. Resolves
     * false (never rejects) when the internet transport isn't initialized.
     */
    @ReactMethod
    fun internetIsReady(promise: Promise) {
        promise.resolve(internetManager?.isReady() ?: false)
    }

    /**
     * Forces an immediate teardown + reconnect + re-authenticate of the
     * internet socket, bypassing the exponential backoff — the deterministic
     * recovery for a foreground-after-background where the cached ready flags
     * may be stale (see InternetManager.forceReconnect). Resolves true when
     * the request reached a live internet transport ("accepted", not
     * "reconnected" — also true when the transport is initialized but not
     * running, where forceReconnect is a deliberate no-op); false (never
     * rejects) only when the internet transport isn't initialized.
     */
    @ReactMethod
    fun internetForceReconnect(promise: Promise) {
        val manager = internetManager
        if (manager == null) {
            promise.resolve(false)
            return
        }
        manager.forceReconnect()
        promise.resolve(true)
    }

    /**
     * Wire event-driven transport callbacks using direct typed UniFFI calls.
     * Each callback fires when Rust enqueues outgoing data, replacing timer-based polling.
     * Falls back to polling if bindings are stale (logged as warning).
     */
    private fun wireTransportCallbacks(proto: OfflineProtocol) {
        // BLE callback
        bleTransport?.let { manager ->
            try {
                proto.setBleTransportCallback(object : uniffi.offline_protocol.BleTransportCallback {
                    override fun onFragmentsAvailable() {
                        manager.onFragmentsAvailable()
                    }
                })
                android.util.Log.i(NAME, "BLE transport callback wired (event-driven sending active)")
                emitDiagnostic("info", "BLE transport callback wired")
            } catch (e: Throwable) {
                android.util.Log.w(NAME, "BLE transport callback not available; using fallback polling", e)
                emitDiagnostic("warning", "BLE callback wiring skipped (regenerate UniFFI bindings)")
            }
        }

        // WiFi Direct callback
        wifiDirectManager?.let { manager ->
            try {
                proto.setWifiDirectTransportCallback(object : uniffi.offline_protocol.WifiDirectTransportCallback {
                    override fun onMessagesAvailable() {
                        manager.onMessagesAvailable()
                    }
                })
                android.util.Log.i(NAME, "WiFi Direct transport callback wired (event-driven sending active)")
                emitDiagnostic("info", "WiFi Direct transport callback wired")
            } catch (e: Throwable) {
                android.util.Log.w(NAME, "WiFi Direct transport callback not available; using fallback polling", e)
                emitDiagnostic("warning", "WiFi Direct callback wiring skipped (regenerate UniFFI bindings)")
            }
        }

        // Reticulum callback
        reticulumManager?.let { manager ->
            try {
                proto.setReticulumTransportCallback(object : uniffi.offline_protocol.ReticulumTransportCallback {
                    override fun onMessagesAvailable() {
                        manager.onMessagesAvailable()
                    }
                })
                android.util.Log.i(NAME, "Reticulum transport callback wired (event-driven sending active)")
                emitDiagnostic("info", "Reticulum transport callback wired")
            } catch (e: Throwable) {
                android.util.Log.w(NAME, "Reticulum transport callback not available; using fallback polling", e)
                emitDiagnostic("warning", "Reticulum callback wiring skipped (regenerate UniFFI bindings)")
            }
        }

        // Nostr callback
        nostrManager?.let { manager ->
            try {
                proto.setNostrTransportCallback(object : uniffi.offline_protocol.NostrTransportCallback {
                    override fun onMessagesAvailable() {
                        manager.onMessagesAvailable()
                    }
                })
                android.util.Log.i(NAME, "Nostr transport callback wired (event-driven sending active)")
                emitDiagnostic("info", "Nostr transport callback wired")
            } catch (e: Throwable) {
                android.util.Log.w(NAME, "Nostr transport callback not available; using fallback polling", e)
                emitDiagnostic("warning", "Nostr callback wiring skipped (regenerate UniFFI bindings)")
            }
        }
    }

    /**
     * Start the foreground service to prevent process death while mesh is active.
     */
    private fun startForegroundService() {
        try {
            // Register before the service exists, so the notification's Stop
            // action always has a host to defer to. The callback lives in a
            // companion field and captures this module, so invalidate() must
            // clear it or the module — and the ReactContext it holds — is
            // pinned for the process lifetime.
            MeshForegroundService.onStopRequestedByUser = { handleUserRequestedMeshStop() }
            MeshForegroundService.start(reactApplicationContext)
            emitDiagnostic("info", "Mesh foreground service started")
        } catch (e: Exception) {
            android.util.Log.w(NAME, "Failed to start foreground service: ${e.message}", e)
            emitDiagnostic("warning", "Foreground service start failed", mapOf(
                "error" to (e.message ?: "unknown")
            ))
        }
    }

    /**
     * Stop the foreground service.
     */
    private fun stopForegroundService() {
        try {
            MeshForegroundService.stop(reactApplicationContext)
            emitDiagnostic("info", "Mesh foreground service stopped")
        } catch (e: Exception) {
            android.util.Log.w(NAME, "Failed to stop foreground service: ${e.message}", e)
        }
    }

    /**
     * Start background process scheduler.
     * Uses a 500ms interval as a fallback tick; latency-sensitive work is driven
     * by transport callbacks (event-driven), not by this timer.
     */
    private fun startProcessScheduler() {
        stopProcessScheduler()
        
        processScheduler = Executors.newSingleThreadScheduledExecutor().apply {
            scheduleAtFixedRate({
                processProtocol()
            }, 0, Constants.PROCESS_INTERVAL_MS, TimeUnit.MILLISECONDS)
        }
    }

    /**
     * Stop background process scheduler
     */
    private fun stopProcessScheduler() {
        val scheduler = processScheduler
        processScheduler = null
        scheduler?.shutdown()
        // `shutdown()` only refuses *new* work; a tick already running keeps
        // going, and `processProtocol` persists (outbox entries, retry
        // lifecycles). `destroy` must not return while one is still writing, or
        // a wipe that follows it races a straggler and leaves a repopulated
        // container. Bounded so a wedged tick cannot hang the bridge — the tick
        // itself is short, and the timeout is far longer than one takes.
        try {
            scheduler?.awaitTermination(
                PROCESS_SHUTDOWN_TIMEOUT_MS,
                TimeUnit.MILLISECONDS
            )
        } catch (_: InterruptedException) {
            Thread.currentThread().interrupt()
        }
    }

    /**
     * Call protocol.process() to handle background tasks
     */
    private fun processProtocol() {
        try {
            val instance = protocol ?: return
            
            if (System.currentTimeMillis() % Constants.LOG_INTERVAL_MS < Constants.LOG_INTERVAL_THRESHOLD_MS) {
                android.util.Log.d(NAME, "Processing protocol...")
            }
            
            instance.process()
            var drained = 0
            while (drained < Constants.MAX_RECEIVE_DRAIN_PER_TICK) {
                val message = instance.receiveMessage() ?: break
                drained++
                if (System.currentTimeMillis() % Constants.LOG_INTERVAL_MS < Constants.LOG_INTERVAL_THRESHOLD_MS) {
                    android.util.Log.d(NAME, "Drained protocol message #$drained: $message")
                }
            }
            if (drained >= Constants.MAX_RECEIVE_DRAIN_PER_TICK) {
                emitDiagnostic("warning", "Capped receiveMessage drain for this process tick", mapOf(
                    "maxBatch" to Constants.MAX_RECEIVE_DRAIN_PER_TICK
                ))
            }
        } catch (e: Exception) {
            android.util.Log.e(NAME, "Process error: ${e.message}", e)
            emitDiagnostic("error", "Protocol process error", mapOf(
                "error" to (e.message ?: "unknown"),
                "exception" to e.javaClass.simpleName
            ))
        }
    }

    private fun parseInternetConfig(config: ReadableMap?): Pair<String, Int> {
        var serverAddress: String? = null
        if (config != null) {
            if (config.hasKey("serverAddress") && !config.isNull("serverAddress")) {
                serverAddress = config.getString("serverAddress")
            } else if (config.hasKey("server_url") && !config.isNull("server_url")) {
                serverAddress = config.getString("server_url")
            }
        }

        if (serverAddress.isNullOrBlank()) {
            throw IllegalArgumentException("Internet transport requires serverAddress")
        }

        var port: Int? = null
        if (config != null) {
            if (config.hasKey("port") && !config.isNull("port")) {
                port = config.getInt("port")
            } else if (config.hasKey("serverPort") && !config.isNull("serverPort")) {
                port = config.getInt("serverPort")
            }
        }

        val uri = try {
            Uri.parse(serverAddress)
        } catch (_: Exception) {
            null
        }

        var host = serverAddress
        if (uri != null && !uri.scheme.isNullOrBlank()) {
            host = uri.host ?: serverAddress
            if (uri.port != -1) {
                port = uri.port
            }
        }

        if (port == null) {
            port = when (uri?.scheme?.lowercase()) {
                "wss", "https" -> Constants.HTTPS_PORT
                "ws", "http" -> Constants.HTTP_PORT
                else -> Constants.HTTPS_PORT
            }
        }

        val safePort = port!!.coerceIn(0, 65535)
        return host!! to safePort
    }

    private fun mapTransportType(type: String): TransportType {
        return when (type.lowercase()) {
            "internet" -> TransportType.INTERNET
            "ble" -> TransportType.BLE
            "wifidirect", "wifi_direct" -> TransportType.WI_FI_DIRECT
            "reticulum" -> TransportType.RETICULUM
            else -> throw IllegalArgumentException("Unsupported transport type: $type")
        }
    }

    private fun buildTopologyJson(topology: NetworkTopology): String {
        val linksArray = JSONArray()
        val nodesArray = JSONArray()
        val connectionCounts = mutableMapOf<String, Int>()
        val transportsByNode = mutableMapOf<String, MutableSet<String>>()

        topology.links.forEach { link ->
            val transportName = normalizeTransportName(link.transport)
            val linkObj = JSONObject().apply {
                put("from", link.sourceId)
                put("to", link.targetId)
                put("quality", link.quality.toDouble())
                put("transport", transportName)
                put("rssi", JSONObject.NULL)
            }
            linksArray.put(linkObj)

            transportsByNode.getOrPut(link.sourceId) { mutableSetOf() }.add(transportName)
            transportsByNode.getOrPut(link.targetId) { mutableSetOf() }.add(transportName)

            connectionCounts[link.sourceId] = (connectionCounts[link.sourceId] ?: 0) + 1
            connectionCounts[link.targetId] = (connectionCounts[link.targetId] ?: 0) + 1
        }

        topology.nodes.forEach { node ->
            val transportsArray = JSONArray()
            transportsByNode[node.nodeId]?.forEach { transportsArray.put(it) }

            val nodeObj = JSONObject().apply {
                put("user_id", node.nodeId)
                put("role", node.role.lowercase())
                put("connection_count", connectionCounts[node.nodeId] ?: 0)
                put("battery_level", JSONObject.NULL)
                put("last_seen", node.lastSeenMs.toLong() / Constants.MILLISECONDS_PER_SECOND)
                put("transports", transportsArray)
            }
            nodesArray.put(nodeObj)
        }

        val avgQuality = if (topology.links.isNotEmpty()) {
            topology.links.map { it.quality }.average()
        } else {
            0.0
        }

        val statsObj = JSONObject().apply {
            put("total_nodes", topology.nodes.size)
            put("relay_nodes", topology.nodes.count { it.role.equals("relay", true) })
            put("total_connections", topology.links.size)
            put("avg_link_quality", avgQuality)
            put("network_diameter", JSONObject.NULL)
        }

        val root = JSONObject().apply {
            put("timestamp", System.currentTimeMillis() / Constants.MILLISECONDS_PER_SECOND)
            put("local_user_id", currentConfig?.userId ?: "")
            put("nodes", nodesArray)
            put("links", linksArray)
            put("stats", statsObj)
        }

        return root.toString()
    }

    private fun buildMessageStatsJson(stats: List<MessageStats>): String {
        val array = JSONArray()
        stats.forEach { stat ->
            val obj = JSONObject().apply {
                put("message_id", stat.messageId)
                put("sender", JSONObject.NULL)
                put("recipient", JSONObject.NULL)
                put("sent_at", stat.sentAtMs.toLong())
                if (stat.deliveredAtMs != null) {
                    put("delivered_at", stat.deliveredAtMs!!.toLong())
                    put("latency_ms", (stat.deliveredAtMs!!.toLong() - stat.sentAtMs.toLong()).coerceAtLeast(0))
                } else {
                    put("delivered_at", JSONObject.NULL)
                    put("latency_ms", JSONObject.NULL)
                }
                put("hop_count", stat.hopCount.toInt())
                put("transport", JSONObject.NULL)
                put("retry_count", 0)
                put("status", stat.status)
            }
            array.put(obj)
        }
        return array.toString()
    }

    private fun normalizeTransportName(name: String): String {
        return when (name.lowercase()) {
            "ble" -> "ble"
            "internet" -> "internet"
            "wifi_direct", "wifidirect", "wi_fi_direct" -> "wifiDirect"
            else -> name.lowercase()
        }
    }
}
