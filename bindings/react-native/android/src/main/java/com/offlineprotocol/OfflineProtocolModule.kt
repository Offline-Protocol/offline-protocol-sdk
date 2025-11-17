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
    private var processScheduler: ScheduledExecutorService? = null
    private var listenerCount: Int = 0
    private var currentConfig: ProtocolConfig? = null
    private val mainHandler = android.os.Handler(android.os.Looper.getMainLooper())

    companion object {
        const val NAME = "OfflineProtocolModule"
        const val EVENT_NAME = "OfflineProtocol_Event"
    }

    override fun getName(): String = NAME

    override fun invalidate() {
        super.invalidate()
        stopProcessScheduler()
        bleManager?.stop()
        bleManager = null
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

        val config = ProtocolConfig(
            appId = json.optString("appId", json.optString("app_id", "")),
            userId = json.optString("userId", json.optString("user_id", "")),
            bleEnabled = json.optBoolean("bleEnabled", json.optBoolean("ble_enabled", true)),
            wifiDirectEnabled = json.optBoolean("wifiDirectEnabled", json.optBoolean("wifi_direct_enabled", true)),
            internetEnabled = json.optBoolean("internetEnabled", json.optBoolean("internet_enabled", true)),
            preferOnline = json.optBoolean("preferOnline", json.optBoolean("prefer_online", false)),
            initialTtl = json.optInt("initialTtl", json.optInt("initial_ttl", 8)).toUByte()
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
                        ?.let { max(1L, min(100L, it)) }
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
            
            // Start BLE manager if available
            bleManager?.let { manager ->
                try {
                    android.util.Log.i(NAME, "About to call BLE manager.start()...")
                    manager.start()
                    android.util.Log.i(NAME, "BLE Manager started successfully")
                    emitDiagnostic("info", "BLE manager started")
                    
                    // CRITICAL FIX: Backup bleStatusChanged(true) call in case timing is off
                    mainHandler.postDelayed({
                        android.util.Log.i(NAME, "Backup bleStatusChanged(true) call")
                        emitDiagnostic("info", "Backup call to protocol.bleStatusChanged(true)")
                        try {
                            protocol?.bleStatusChanged(true)
                            emitDiagnostic("info", "Backup bleStatusChanged(true) completed")
                        } catch (e: Exception) {
                            android.util.Log.w(NAME, "Backup bleStatusChanged failed: ${e.message}", e)
                        }
                    }, 1000) // 1 second delay
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
                    // Don't fail the entire start if BLE fails
                }
            } ?: run {
                android.util.Log.w(NAME, "⚠️ BLE manager is null, cannot start")
                emitDiagnostic("warning", "BLE manager is null")
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
    fun sendMessage(recipient: String, content: String, priority: Int, promise: Promise) {
        try {
            val msgPriority = when (priority) {
                0 -> MessagePriority.LOW
                1 -> MessagePriority.MEDIUM
                2 -> MessagePriority.HIGH
                3 -> MessagePriority.CRITICAL
                else -> MessagePriority.MEDIUM
            }
            
            val proto = protocol ?: throw IllegalStateException("Protocol not initialized")
            val messageId = proto.sendMessage(recipient, content, msgPriority)
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
                    val (host, port) = parseInternetConfig(config)
                    proto.addInternetTransport(host, port.toUShort())
                }
                "wifidirect", "wifi_direct" -> {
                    proto.addWifiDirectTransport()
                }
                "ble" -> {
                    // BLE transport is managed automatically
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

    @ReactMethod
    fun disableTransport(type: String, promise: Promise) {
        try {
            val proto = protocol ?: throw IllegalStateException("Protocol not initialized")
            val transportType = mapTransportType(type)
            proto.removeTransport(transportType)
            promise.resolve(null)
        } catch (e: ProtocolException) {
            promise.reject("ERROR_TRANSPORT_DISABLE", "Failed to disable transport: ${e.message}", e)
        } catch (e: Exception) {
            promise.reject("ERROR_TRANSPORT_DISABLE", "Failed to disable transport: ${e.message}", e)
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
            protocol?.setBatteryLevel(level.coerceIn(0, 100).toUByte())
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
                rssiSwitchThreshold = json.optInt("rssiSwitchThreshold", -85).toShort(),
                congestionQueueThreshold = json.optLong("congestionQueueThreshold", 50).toULong(),
                stabilityWindowSecs = json.optLong("stabilityWindowSecs", 8).toULong(),
                poorSignalDurationSecs = json.optLong("poorSignalDurationSecs", 10).toULong(),
                ttlEscalationThreshold = json.optInt("ttlEscalationThreshold", 2).toUByte(),
                congestionDurationSecs = json.optLong("congestionDurationSecs", 10).coerceAtLeast(0).toULong(),
                ttlEscalationHoldSecs = json.optLong("ttlEscalationHoldSecs", 20).coerceAtLeast(1).toULong(),
                historyWindowSize = json.optLong("historyWindowSize", 10).let { max(1L, min(100L, it)) }.toULong(),
                queueRecoveryRatio = json.optDouble("queueRecoveryRatio", 0.5).toFloat().coerceIn(0f, 1f)
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

    /**
     * Start background process scheduler
     */
    private fun startProcessScheduler() {
        stopProcessScheduler()
        
        processScheduler = Executors.newSingleThreadScheduledExecutor().apply {
            scheduleAtFixedRate({
                processProtocol()
            }, 0, 100, TimeUnit.MILLISECONDS)
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
            
            // Add debug logging to verify process is being called
            if (System.currentTimeMillis() % 5000 < 100) { // Log every ~5 seconds
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
                "wss", "https" -> 443
                "ws", "http" -> 80
                else -> 443
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
                put("last_seen", node.lastSeenMs.toLong() / 1000)
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
            put("timestamp", System.currentTimeMillis() / 1000)
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

