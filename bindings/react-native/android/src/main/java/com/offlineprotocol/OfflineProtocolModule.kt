package com.offlineprotocol

import com.facebook.react.bridge.*
import com.facebook.react.modules.core.DeviceEventManagerModule
import org.json.JSONObject
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
    private fun parseConfig(configJson: String): ProtocolConfig {
        val json = JSONObject(configJson)
        
        return ProtocolConfig(
            appId = json.optString("appId", json.optString("app_id", "")),
            userId = json.optString("userId", json.optString("user_id", "")),
            bleEnabled = json.optBoolean("bleEnabled", json.optBoolean("ble_enabled", true)),
            wifiDirectEnabled = json.optBoolean("wifiDirectEnabled", json.optBoolean("wifi_direct_enabled", true)),
            internetEnabled = json.optBoolean("internetEnabled", json.optBoolean("internet_enabled", true)),
            preferOnline = json.optBoolean("preferOnline", json.optBoolean("prefer_online", false)),
            initialTtl = json.optInt("initialTtl", json.optInt("initial_ttl", 8)).toUByte()
        )
    }

    @ReactMethod
    fun create(configJson: String, promise: Promise) {
        try {
            val config = parseConfig(configJson)
            val proto = OfflineProtocol(config)
            
            // Set up event callback
            proto.setEventCallback(object : EventCallback {
                override fun onEvent(eventJson: String) {
                    val params = Arguments.createMap().apply {
                        putString("eventJson", eventJson)
                    }
                    sendEvent(EVENT_NAME, params)
                }
            })
            
            protocol = proto
            
            // Initialize BLE manager if BLE is enabled
            if (config.bleEnabled) {
                bleManager = BleManager(reactApplicationContext, proto, config.userId)
                android.util.Log.i(NAME, "BLE Manager initialized for user: ${config.userId}")
            }
            
            // Start process scheduler
            startProcessScheduler()
            
            promise.resolve(null)
        } catch (e: Exception) {
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

    @ReactMethod
    fun start(promise: Promise) {
        try {
            protocol?.start()
            
            // Start BLE manager if available
            bleManager?.let { manager ->
                try {
                    manager.start()
                    android.util.Log.i(NAME, "BLE Manager started")
                } catch (e: Exception) {
                    android.util.Log.w(NAME, "Warning: Failed to start BLE Manager: ${e.message}")
                    // Don't fail the entire start if BLE fails
                }
            }
            
            promise.resolve(null)
        } catch (e: Exception) {
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
        
        try {
            protocol?.stop()
            promise.resolve(null)
        } catch (e: Exception) {
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
                "wifidirect" -> TransportType.WIFI_DIRECT
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
                "wifidirect" -> TransportType.WIFI_DIRECT
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
                switchHysteresis = json.optDouble("switchHysteresis", 15.0).toFloat(),
                switchCooldownSecs = json.optLong("switchCooldownSecs", 20).toULong(),
                bleToWifiRetryThreshold = json.optInt("bleToWifiRetryThreshold", 2).toUInt(),
                rssiSwitchThreshold = json.optInt("rssiSwitchThreshold", -85).toShort(),
                congestionQueueThreshold = json.optLong("congestionQueueThreshold", 50).toULong(),
                stabilityWindowSecs = json.optLong("stabilityWindowSecs", 8).toULong()
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
            protocol?.process()
        } catch (e: Exception) {
            android.util.Log.e(NAME, "Process error: ${e.message}", e)
        }
    }
}

