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
            promise.resolve(null)
        } catch (e: Exception) {
            promise.reject("ERROR_START", "Failed to start protocol: ${e.message}", e)
        }
    }

    @ReactMethod
    fun stop(promise: Promise) {
        stopProcessScheduler()
        
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

