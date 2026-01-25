package com.offlineprotocol

import android.net.Uri
import com.facebook.react.bridge.*
import com.facebook.react.modules.core.DeviceEventManagerModule
import org.json.JSONArray
import org.json.JSONObject
import kotlin.math.max
import kotlin.math.min
import java.util.concurrent.Executors
import java.util.concurrent.ScheduledExecutorService
import java.util.concurrent.TimeUnit

// Import generated UniFFI bindings
import uniffi.offline_protocol.*

/**
 * UniFFI-based React Native module
 */
class OfflineProtocolModule(reactContext: ReactApplicationContext) :
    ReactContextBaseJavaModule(reactContext) {

    private var protocol: OfflineProtocol? = null
    private var bleManager: BleManager? = null
    private var internetManager: InternetManager? = null
    private var wifiDirectManager: WifiDirectManager? = null
    private var processScheduler: ScheduledExecutorService? = null
    private var listenerCount: Int = 0
    private var currentConfig: ProtocolConfig? = null
    private val mainHandler = android.os.Handler(android.os.Looper.getMainLooper())

    companion object {
        const val NAME = "OfflineProtocolModule"
        const val EVENT_NAME = "OfflineProtocol_Event"
    }
    
    private object Constants {
        const val DEFAULT_INITIAL_TTL = 8
        const val MIN_BATTERY_LEVEL = 0
        const val MAX_BATTERY_LEVEL = 100
        const val MIN_HISTORY_WINDOW = 1L
        const val MAX_HISTORY_WINDOW = 100L
        const val BLE_RESTART_DELAY_MS = 1000L
        const val PROCESS_INTERVAL_MS = 100L
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
        stopProcessScheduler()
        bleManager?.stop()
        bleManager = null
        internetManager?.stop()
        internetManager = null
        wifiDirectManager?.stop()
        wifiDirectManager = null
        protocol = null
    }

    @ReactMethod
    fun addListener(eventName: String) {
        listenerCount += 1
    }

    @ReactMethod
    fun removeListeners(count: Double) {
        listenerCount = (listenerCount - count.toInt()).coerceAtLeast(0)
    }

    /**
     * Parse JSON config into ProtocolConfig
     */
    private data class ParsedConfig(
        val coreConfig: ProtocolConfig,
        val rawJson: JSONObject
    )

    private fun parseConfig(configJson: String): ParsedConfig {
        val json = JSONObject(configJson)

        // Parse encryption config with defaults (enabled by default)
        val encryptionJson = json.optJSONObject("encryption")
        val encryptionEnabled = encryptionJson?.optBoolean("enabled", true) ?: true
        val autoKeyExchange = encryptionJson?.let {
            it.optBooleanCompat("autoKeyExchange", "auto_key_exchange") ?: true
        } ?: true
        val storePending = encryptionJson?.let {
            it.optBooleanCompat("storePending", "store_pending") ?: true
        } ?: true

        val config = ProtocolConfig(
            appId = json.optString("appId", json.optString("app_id", "")),
            userId = json.optString("userId", json.optString("user_id", "")),
            bleEnabled = json.optBoolean("bleEnabled", json.optBoolean("ble_enabled", true)),
            wifiDirectEnabled = json.optBoolean("wifiDirectEnabled", json.optBoolean("wifi_direct_enabled", true)),
            internetEnabled = json.optBoolean("internetEnabled", json.optBoolean("internet_enabled", true)),
            preferOnline = json.optBoolean("preferOnline", json.optBoolean("prefer_online", false)),
            initialTtl = json.optInt("initialTtl", json.optInt("initial_ttl", Constants.DEFAULT_INITIAL_TTL)).toUByte(),
            encryptionEnabled = encryptionEnabled,
            autoKeyExchange = autoKeyExchange,
            storePending = storePending
        )

        return ParsedConfig(config, json)
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
            val priorityRaw = relayJson.optString("relayPriority", relayJson.optString("relay_priority", ""))
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

    @ReactMethod
    fun create(configJson: String, promise: Promise) {
        try {
            val parsed = parseConfig(configJson)
            val config = parsed.coreConfig
            val proto = OfflineProtocol(config)
            currentConfig = config
            emitDiagnostic("info", "Protocol core created", mapOf(
                "appId" to config.appId,
                "userId" to config.userId,
                "bleEnabled" to config.bleEnabled,
                "wifiDirectEnabled" to config.wifiDirectEnabled,
                "internetEnabled" to config.internetEnabled
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
            
            // Initialize BLE manager if BLE is enabled
            if (config.bleEnabled) {
                bleManager = BleManager(reactApplicationContext, proto, config.userId) { level, message, context ->
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

    @ReactMethod
    fun start(promise: Promise) {
        try {
            emitDiagnostic("info", "Starting protocol")
            protocol?.start()
            emitDiagnostic("info", "Protocol core started")
            
            //  Start BLE manager if available - BLE should work independently
            // BLE peer discovery and messaging must work even when Internet/WiFi are disabled
            bleManager?.let { manager ->
                try {
                    android.util.Log.i(NAME, "Starting BLE manager (BLE should work independently of other transports)...")
                    emitDiagnostic("info", "Starting BLE manager", mapOf(
                        "internetEnabled" to (currentConfig?.internetEnabled ?: false),
                        "wifiDirectEnabled" to (currentConfig?.wifiDirectEnabled ?: false)
                    ))
                    manager.start()
                    android.util.Log.i(NAME, "✅ BLE Manager started successfully - scanning and advertising should be active")
                    emitDiagnostic("info", "BLE manager started - peer discovery active", mapOf(
                        "scanning" to true,
                        "advertising" to true
                    ))
                    
                    //  Ensure bleStatusChanged(true) is called immediately and as backup
                    // This ensures BLE transport is marked as Available for message sending
                    try {
                        protocol?.bleStatusChanged(true)
                        android.util.Log.i(NAME, "✅ Called protocol.bleStatusChanged(true) immediately")
                        emitDiagnostic("info", "BLE status set to available")
                    } catch (e: Exception) {
                        android.util.Log.w(NAME, "Immediate bleStatusChanged failed: ${e.message}", e)
                    }
                    
                    // Backup call in case timing is off
                    mainHandler.postDelayed({
                        android.util.Log.i(NAME, "Backup bleStatusChanged(true) call")
                        emitDiagnostic("info", "Backup call to protocol.bleStatusChanged(true)")
                        try {
                            protocol?.bleStatusChanged(true)
                            emitDiagnostic("info", "Backup bleStatusChanged(true) completed")
                        } catch (e: Exception) {
                            android.util.Log.w(NAME, "Backup bleStatusChanged failed: ${e.message}", e)
                        }
                    }, Constants.BLE_RESTART_DELAY_MS)
                } catch (e: Exception) {
                    android.util.Log.e(NAME, "❌ FAILED to start BLE Manager!", e)
                    android.util.Log.e(NAME, "Error type: ${e.javaClass.simpleName}")
                    android.util.Log.e(NAME, "Error message: ${e.message}")
                    android.util.Log.e(NAME, "Stack trace: ", e)
                    emitDiagnostic("error", "Failed to start BLE manager", mapOf(
                        "message" to (e.message ?: "unknown"),
                        "exception" to e.javaClass.simpleName,
                        "stackTrace" to e.stackTraceToString()
                    ))
                    // Don't fail the entire start if BLE fails, but log the error clearly
                    android.util.Log.w(NAME, "⚠️ Protocol will continue without BLE, but peer discovery and BLE messaging will not work")
                }
            } ?: run {
                android.util.Log.w(NAME, "⚠️ BLE manager is null - BLE was not initialized. Check if bleEnabled=true in config.")
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
    fun stop(promise: Promise) {
        stopProcessScheduler()
        
        // Stop BLE manager first
        bleManager?.stop()
        android.util.Log.i(NAME, "BLE Manager stopped")
        emitDiagnostic("info", "BLE manager stopped")
        
        // Stop Internet manager
        internetManager?.stop()
        android.util.Log.i(NAME, "Internet Manager stopped")
        emitDiagnostic("info", "Internet manager stopped")
        
        try {
            protocol?.stop()
            emitDiagnostic("info", "Protocol stopped")
            promise.resolve(null)
        } catch (e: Exception) {
            emitDiagnostic("error", "Failed to stop protocol", mapOf(
                "message" to (e.message ?: "unknown"),
                "exception" to e.javaClass.simpleName
            ))
            promise.reject("ERROR_STOP", "Failed to stop protocol: ${e.message}", e)
        }
    }

    @ReactMethod
    fun pause(promise: Promise) {
        try {
            // Pause BLE manager for background mode
            bleManager?.pause()
            
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
            
            // Resume BLE manager
            bleManager?.resume()
            
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
            promise.reject("ERROR_SEND", "Failed to send message: ${e.message}", e)
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
            bleManager?.stop()
            bleManager = null
            internetManager?.stop()
            internetManager = null
            wifiDirectManager?.stop()
            wifiDirectManager = null

            try {
                protocol?.stop()
            } catch (_: Exception) {
                // Ignore stop errors during destroy
            }

            protocol = null
            listenerCount = 0
            currentConfig = null
            promise.resolve(null)
        } catch (e: Exception) {
            promise.reject("ERROR_DESTROY", "Failed to destroy protocol: ${e.message}", e)
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
                        // Create manager if not already created
                        internetManager = InternetManager(reactApplicationContext, proto, currentConfig?.userId ?: "unknown") { level, message, context ->
                            emitDiagnostic(level, message, context)
                        }
                        emitDiagnostic("info", "Internet manager created on demand")
                    }
                    
                    val manager = internetManager
                        ?: throw IllegalStateException("Failed to create Internet manager")
                    
                    // Stop the manager first if it's running (to ensure clean restart)
                    if (manager.state == TransportState.RUNNING) {
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
                    if (bleManager == null) {
                        bleManager = BleManager(reactApplicationContext, proto, currentConfig?.userId ?: "unknown") { level, message, context ->
                            emitDiagnostic(level, message, context)
                        }
                        emitDiagnostic("info", "BLE manager created on demand")
                    }
                    
                    val manager = bleManager
                        ?: throw IllegalStateException("Failed to create BLE manager")
                    
                    if (manager.state != TransportState.RUNNING) {
                        manager.start()
                        emitDiagnostic("info", "BLE transport enabled")
                    }
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
        
        // Internet transport is already registered during protocol initialization
        // Just configure and start the WebSocket manager
        manager.configure(wsUrl, autoReconnect, maxRetries)
        manager.start()
        
        emitDiagnostic("info", "Internet transport enabled", mapOf(
            "serverUrl" to wsUrl,
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
                    bleManager?.stop()
                    try {
                        proto.bleStatusChanged(false)
                    } catch (e: Exception) {
                        android.util.Log.w(NAME, "Failed to notify BLE status change: ${e.message}")
                    }
                    emitDiagnostic("info", "BLE transport disabled (manager stopped)")
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
    fun sendFile(filePath: String, recipient: String, fileName: String, promise: Promise) {
        try {
            val proto = protocol ?: throw IllegalStateException("Protocol not initialized")
            val id = proto.sendFile(recipient, filePath, fileName)
            promise.resolve(id)
        } catch (e: ProtocolException) {
            promise.reject("ERROR_SEND_FILE", "Failed to send file: ${e.message}", e)
        } catch (e: Exception) {
            promise.reject("ERROR_SEND_FILE", "Failed to send file: ${e.message}", e)
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
                    putString("file_name", progress.fileId)
                    putDouble("file_size", 0.0)
                    putInt("chunks_completed", progress.chunksSent.toInt())
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
            promise.reject("ERROR_FILE_CANCEL", "Failed to cancel file transfer: ${e.message}", e)
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
            uniffi.offline_protocol.ProtocolState.STARTING -> "Starting"
            uniffi.offline_protocol.ProtocolState.RUNNING -> "Running"
            uniffi.offline_protocol.ProtocolState.PAUSED -> "Paused"
            uniffi.offline_protocol.ProtocolState.STOPPING -> "Stopping"
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
                val map = Arguments.createMap()
                map.putBoolean("preferOnline", config.preferOnline)
                map.putDouble("switchHysteresis", config.switchHysteresis.toDouble())
                map.putInt("switchCooldownSecs", config.switchCooldownSecs.toInt())
                map.putInt("bleToWifiRetryThreshold", config.bleToWifiRetryThreshold.toInt())
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
                defaultTimeoutMs = json.optLong("defaultTimeoutMs", 5000).toULong(),
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
            val retryConfig = RetryConfig(
                maxRetries = json.optInt("maxRetries", 3).toUInt(),
                initialDelayMs = json.optLong("initialDelayMs", 1000).toULong(),
                maxDelayMs = json.optLong("maxDelayMs", 30000).toULong(),
                backoffMultiplier = json.optDouble("backoffMultiplier", 2.0).toFloat(),
                outboxMaxLifetimeMs = json.optLong("outboxMaxLifetimeMs", 3600000).toULong()
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
    fun learnRoute(destination: String, nextHop: String, hopCount: Int, quality: Double, promise: Promise) {
        try {
            val proto = protocol ?: throw IllegalStateException("Protocol not initialized")
            proto.learnRoute(
                destination,
                nextHop,
                hopCount.coerceIn(0, 255).toUByte(),
                quality.toFloat()
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
    fun processFileChunk(fileId: String, chunkIndex: Int, data: ReadableArray, promise: Promise) {
        try {
            val proto = protocol ?: throw IllegalStateException("Protocol not initialized")
            val bytes = mutableListOf<UByte>()
            for (i in 0 until data.size()) {
                bytes.add(data.getInt(i).toUByte())
            }
            proto.processFileChunk(fileId, chunkIndex.toUInt(), bytes)
            promise.resolve(null)
        } catch (e: ProtocolException) {
            promise.reject("ERROR_FILE", "Failed to process file chunk: ${e.message}", e)
        } catch (e: Exception) {
            promise.reject("ERROR_FILE", "Failed to process file chunk: ${e.message}", e)
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
    fun internetReturnMessage(promise: Promise) {
        try {
            val proto = protocol ?: throw IllegalStateException("Protocol not initialized")
            proto.internetReturnMessage()
            promise.resolve(null)
        } catch (e: Exception) {
            promise.reject("ERROR_INTERNET", "Internet return message failed: ${e.message}", e)
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
            val storage = MlsSecureStorage(reactApplicationContext)
            proto.initializeMls(storage)
            emitDiagnostic("info", "MLS initialized with EncryptedSharedPreferences storage")
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
                groupId = json.optString("groupId", ""),
                welcomeData = welcomeData,
                inviterId = json.optString("inviterId", ""),
                groupName = json.optString("groupName", null),
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
                groupId = json.optString("groupId", ""),
                messageType = json.optString("messageType", "Application"),
                epoch = json.optLong("epoch", 0).toULong(),
                ciphertext = ciphertext,
                senderId = json.optString("senderId", ""),
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
                groupId = json.optString("groupId", ""),
                messageType = json.optString("messageType", "Application"),
                epoch = json.optLong("epoch", 0).toULong(),
                ciphertext = ciphertext,
                senderId = json.optString("senderId", ""),
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
     * Create a new group
     */
    @ReactMethod
    fun mlsCreateGroup(groupName: String, promise: Promise) {
        try {
            val proto = protocol ?: throw IllegalStateException("Protocol not initialized")
            val info = proto.mlsCreateGroup(groupName)
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
            promise.reject("ERROR_MLS", "Failed to create group: ${e.message}", e)
        }
    }

    /**
     * Add a member to a group
     */
    @ReactMethod
    fun mlsAddGroupMember(groupId: String, memberKeyPackage: ReadableArray, promise: Promise) {
        try {
            val proto = protocol ?: throw IllegalStateException("Protocol not initialized")
            val data = mutableListOf<UByte>()
            for (i in 0 until memberKeyPackage.size()) {
                data.add(memberKeyPackage.getInt(i).toUByte())
            }
            val welcome = proto.mlsAddGroupMember(groupId, data)
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
            promise.reject("ERROR_MLS", "Failed to add group member: ${e.message}", e)
        }
    }

    /**
     * Remove a member from a group
     */
    @ReactMethod
    fun mlsRemoveGroupMember(groupId: String, memberId: String, promise: Promise) {
        try {
            val proto = protocol ?: throw IllegalStateException("Protocol not initialized")
            val commit = proto.mlsRemoveGroupMember(groupId, memberId)
            val result = Arguments.createMap().apply {
                putString("groupId", commit.groupId)
                putString("messageType", commit.messageType)
                putDouble("epoch", commit.epoch.toDouble())
                val ciphertextArray = Arguments.createArray()
                commit.ciphertext.forEach { ciphertextArray.pushInt(it.toInt()) }
                putArray("ciphertext", ciphertextArray)
                putString("senderId", commit.senderId)
                putDouble("timestampMs", commit.timestampMs.toDouble())
            }
            promise.resolve(result)
        } catch (e: Exception) {
            promise.reject("ERROR_MLS", "Failed to remove group member: ${e.message}", e)
        }
    }

    /**
     * Leave a group
     */
    @ReactMethod
    fun mlsLeaveGroup(groupId: String, promise: Promise) {
        try {
            val proto = protocol ?: throw IllegalStateException("Protocol not initialized")
            proto.mlsLeaveGroup(groupId)
            promise.resolve(null)
        } catch (e: Exception) {
            promise.reject("ERROR_MLS", "Failed to leave group: ${e.message}", e)
        }
    }

    /**
     * Encrypt a message for a group
     */
    @ReactMethod
    fun mlsEncryptForGroup(groupId: String, plaintext: ReadableArray, promise: Promise) {
        try {
            val proto = protocol ?: throw IllegalStateException("Protocol not initialized")
            val data = mutableListOf<UByte>()
            for (i in 0 until plaintext.size()) {
                data.add(plaintext.getInt(i).toUByte())
            }
            val encrypted = proto.mlsEncryptForGroup(groupId, data)
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
            promise.reject("ERROR_MLS", "Failed to encrypt message for group: ${e.message}", e)
        }
    }

    /**
     * Decrypt a message from a group
     */
    @ReactMethod
    fun mlsDecryptFromGroup(encryptedJson: String, promise: Promise) {
        try {
            val proto = protocol ?: throw IllegalStateException("Protocol not initialized")
            val json = JSONObject(encryptedJson)
            
            val ciphertextArray = json.optJSONArray("ciphertext") ?: JSONArray()
            val ciphertext = mutableListOf<UByte>()
            for (i in 0 until ciphertextArray.length()) {
                ciphertext.add(ciphertextArray.getInt(i).toUByte())
            }
            
            val encrypted = MlsEncryptedMessage(
                groupId = json.optString("groupId", ""),
                messageType = json.optString("messageType", "Application"),
                epoch = json.optLong("epoch", 0).toULong(),
                ciphertext = ciphertext,
                senderId = json.optString("senderId", ""),
                timestampMs = json.optLong("timestampMs", 0).toULong()
            )
            
            val plaintext = proto.mlsDecryptFromGroup(encrypted)
            if (plaintext != null) {
                val result = Arguments.createArray()
                plaintext.forEach { result.pushInt(it.toInt()) }
                promise.resolve(result)
            } else {
                promise.resolve(null)
            }
        } catch (e: Exception) {
            promise.reject("ERROR_MLS", "Failed to decrypt message from group: ${e.message}", e)
        }
    }

    /**
     * Join a group using a Welcome message
     */
    @ReactMethod
    fun mlsJoinGroup(welcomeJson: String, promise: Promise) {
        try {
            val proto = protocol ?: throw IllegalStateException("Protocol not initialized")
            val json = JSONObject(welcomeJson)
            
            val welcomeDataArray = json.optJSONArray("welcomeData") ?: JSONArray()
            val welcomeData = mutableListOf<UByte>()
            for (i in 0 until welcomeDataArray.length()) {
                welcomeData.add(welcomeDataArray.getInt(i).toUByte())
            }
            
            val welcome = MlsWelcomeMessage(
                groupId = json.optString("groupId", ""),
                welcomeData = welcomeData,
                inviterId = json.optString("inviterId", ""),
                groupName = json.optString("groupName", null),
                timestampMs = json.optLong("timestampMs", 0).toULong()
            )
            
            val info = proto.mlsJoinGroup(welcome)
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
            promise.reject("ERROR_MLS", "Failed to join group: ${e.message}", e)
        }
    }

    /**
     * List all groups
     */
    @ReactMethod
    fun mlsListGroups(promise: Promise) {
        val proto = protocol
        if (proto == null) {
            promise.resolve(Arguments.createArray())
            return
        }
        val groups = proto.mlsListGroups()
        promise.resolve(Arguments.fromList(groups))
    }

    /**
     * Get group information
     */
    @ReactMethod
    fun mlsGetGroupInfo(groupId: String, promise: Promise) {
        val proto = protocol
        if (proto == null) {
            promise.reject("ERROR_MLS", "Protocol not initialized", null)
            return
        }
        val info = proto.mlsGetGroupInfo(groupId)
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
                groupId = json.optString("groupId", ""),
                welcomeData = welcomeData,
                inviterId = json.optString("inviterId", ""),
                groupName = json.optString("groupName", null),
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

    /**
     * Start background process scheduler
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
        processScheduler?.shutdown()
        processScheduler = null
    }

    /**
     * Call protocol.process() to handle background tasks
     */
    private fun processProtocol() {
        try {
            val instance = protocol ?: return
            
            if (System.currentTimeMillis() % Constants.LOG_INTERVAL_MS < Constants.LOG_INTERVAL_THRESHOLD_MS) {
                android.util.Log.d(NAME, "🔄 Processing protocol...")
            }
            
            instance.process()
            
            var messageCount = 0
            while (true) {
                val message = instance.receiveMessage() ?: break
                messageCount++
                android.util.Log.i(NAME, "🎉 PROTOCOL RECEIVED MESSAGE #$messageCount: $message")
                emitDiagnostic("info", "Protocol received message", mapOf(
                    "messageNumber" to messageCount,
                    "messageJson" to (message ?: "null")
                ))
                // Message events are dispatched via the event callback; no further action required here.
            }
            
            // Only log when messages are found to avoid spam
            if (messageCount > 0) {
                android.util.Log.i(NAME, "📬 Processed $messageCount received messages")
            }
        } catch (e: Exception) {
            android.util.Log.e(NAME, "Process error: ${e.message}", e)
            emitDiagnostic("error", "Protocol process error", mapOf(
                "error" to (e.message ?: "unknown"),
                "exception" to e.javaClass.simpleName
            ))
        }
    }

    private fun JSONObject.optBooleanCompat(vararg keys: String): Boolean? {
        keys.forEach { key ->
            if (has(key) && !isNull(key)) {
                return runCatching { getBoolean(key) }.getOrNull()
            }
        }
        return null
    }

    private fun JSONObject.optIntCompat(vararg keys: String): Int? {
        keys.forEach { key ->
            if (has(key) && !isNull(key)) {
                return runCatching { getInt(key) }
                    .getOrElse { runCatching { getDouble(key).toInt() }.getOrNull() }
            }
        }
        return null
    }

    private fun JSONObject.optLongCompat(vararg keys: String): Long? {
        keys.forEach { key ->
            if (has(key) && !isNull(key)) {
                return runCatching { getLong(key) }
                    .getOrElse { runCatching { getDouble(key).toLong() }.getOrNull() }
            }
        }
        return null
    }

    private fun JSONObject.optDoubleCompat(vararg keys: String): Double? {
        keys.forEach { key ->
            if (has(key) && !isNull(key)) {
                return runCatching { getDouble(key) }.getOrNull()
            }
        }
        return null
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

