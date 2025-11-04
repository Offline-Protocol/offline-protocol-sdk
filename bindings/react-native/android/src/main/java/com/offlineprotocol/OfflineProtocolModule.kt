package com.offlineprotocol

import com.facebook.react.bridge.*
import com.facebook.react.modules.core.DeviceEventManagerModule
import java.util.concurrent.Executors
import java.util.concurrent.ScheduledExecutorService
import java.util.concurrent.TimeUnit

/**
 * React Native module for Offline Protocol SDK.
 * Bridges JavaScript calls to the native C FFI library.
 */
class OfflineProtocolModule(reactContext: ReactApplicationContext) :
    ReactContextBaseJavaModule(reactContext) {

    companion object {
        init {
            System.loadLibrary("offline_protocol_ffi")
        }
    }

    private var protocolHandle: Long = 0
    private var eventPollingExecutor: ScheduledExecutorService? = null
    private val reactContext = reactContext

    override fun getName(): String {
        return "OfflineProtocol"
    }

    /**
     * Starts the protocol with the given configuration.
     */
    @ReactMethod
    fun start(configJson: String, promise: Promise) {
        try {
            if (protocolHandle != 0L) {
                promise.reject("ALREADY_STARTED", "Protocol is already started")
                return
            }

            // Create protocol instance
            protocolHandle = nativeCreate(configJson)
            if (protocolHandle == 0L) {
                promise.reject("CREATE_FAILED", "Failed to create protocol instance")
                return
            }

            // Start the protocol
            val result = nativeStart(protocolHandle)
            if (result != 0) {
                nativeDestroy(protocolHandle)
                protocolHandle = 0
                promise.reject("START_FAILED", "Failed to start protocol: error code $result")
                return
            }

            // Start event polling
            startEventPolling()

            promise.resolve(null)
        } catch (e: Exception) {
            promise.reject("EXCEPTION", e.message, e)
        }
    }

    /**
     * Stops the protocol.
     */
    @ReactMethod
    fun stop(promise: Promise) {
        try {
            if (protocolHandle == 0L) {
                promise.reject("NOT_STARTED", "Protocol is not started")
                return
            }

            // Stop event polling
            stopEventPolling()

            // Stop the protocol
            val result = nativeStop(protocolHandle)
            if (result != 0) {
                promise.reject("STOP_FAILED", "Failed to stop protocol: error code $result")
                return
            }

            // Destroy the protocol instance
            nativeDestroy(protocolHandle)
            protocolHandle = 0

            promise.resolve(null)
        } catch (e: Exception) {
            promise.reject("EXCEPTION", e.message, e)
        }
    }

    /**
     * Pauses the protocol (for background mode).
     */
    @ReactMethod
    fun pause(promise: Promise) {
        try {
            if (protocolHandle == 0L) {
                promise.reject("NOT_STARTED", "Protocol is not started")
                return
            }

            stopEventPolling()
            promise.resolve(null)
        } catch (e: Exception) {
            promise.reject("EXCEPTION", e.message, e)
        }
    }

    /**
     * Resumes the protocol from pause.
     */
    @ReactMethod
    fun resume(promise: Promise) {
        try {
            if (protocolHandle == 0L) {
                promise.reject("NOT_STARTED", "Protocol is not started")
                return
            }

            startEventPolling()
            promise.resolve(null)
        } catch (e: Exception) {
            promise.reject("EXCEPTION", e.message, e)
        }
    }

    /**
     * Sends a message.
     */
    @ReactMethod
    fun sendMessage(recipient: String, content: String, priority: Int, promise: Promise) {
        try {
            if (protocolHandle == 0L) {
                promise.reject("NOT_STARTED", "Protocol is not started")
                return
            }

            val messageIdBuffer = ByteArray(256)
            val result = nativeSendMessage(
                protocolHandle,
                recipient,
                content,
                priority,
                messageIdBuffer,
                messageIdBuffer.size
            )

            if (result != 0) {
                promise.reject("SEND_FAILED", "Failed to send message: error code $result")
                return
            }

            // Extract message ID from buffer (null-terminated string)
            // Find the null terminator
            val nullIndex = messageIdBuffer.indexOf(0)
            val messageId = if (nullIndex >= 0) {
                String(messageIdBuffer, 0, nullIndex, Charsets.UTF_8)
            } else {
                String(messageIdBuffer, Charsets.UTF_8).trim { it <= ' ' }
            }
            promise.resolve(messageId)
        } catch (e: Exception) {
            promise.reject("EXCEPTION", e.message, e)
        }
    }

    /**
     * Sends a file.
     * Note: File transfer functionality is not yet implemented in the FFI layer.
     */
    @ReactMethod
    fun sendFile(recipient: String, filePath: String, priority: Int, promise: Promise) {
        promise.reject("NOT_IMPLEMENTED", "File transfer is not yet implemented")
    }

    /**
     * Starts polling for events from the native layer.
     */
    private fun startEventPolling() {
        if (eventPollingExecutor != null) {
            return // Already polling
        }

        eventPollingExecutor = Executors.newSingleThreadScheduledExecutor()
        eventPollingExecutor?.scheduleWithFixedDelay(
            {
                if (protocolHandle != 0L) {
                    pollAndEmitEvents()
                }
            },
            0,
            100,
            TimeUnit.MILLISECONDS
        )
    }

    /**
     * Stops event polling.
     */
    private fun stopEventPolling() {
        eventPollingExecutor?.shutdown()
        eventPollingExecutor = null
    }

    /**
     * Polls for events and emits them to JavaScript.
     */
    private fun pollAndEmitEvents() {
        if (protocolHandle == 0L) {
            return
        }

        val eventBuffer = ByteArray(4096)
        val result = nativePollEvent(protocolHandle, eventBuffer, eventBuffer.size)

        if (result == 0) {
            // No event available
            return
        }

        if (result < 0) {
            // Error occurred
            return
        }

        // Extract event JSON from buffer (null-terminated string)
        val nullIndex = eventBuffer.indexOf(0)
        val eventJson = if (nullIndex >= 0) {
            String(eventBuffer, 0, nullIndex, Charsets.UTF_8)
        } else {
            String(eventBuffer, Charsets.UTF_8).trim { it <= ' ' }
        }
        if (eventJson.isNotEmpty()) {
            try {
                // Parse JSON event using org.json.JSONObject
                val jsonObject = org.json.JSONObject(eventJson)
                val eventType = jsonObject.optString("type", "")

                // Map snake_case event types to JavaScript event names
                val jsEventType = when (eventType) {
                    "message_sent" -> "message:sent"
                    "message_received" -> "message:received"
                    "message_delivered" -> "message:delivered"
                    "message_failed" -> "message:failed"
                    "transport_switched" -> "transport:switched"
                    "relay_promoted" -> "relay:promoted"
                    "relay_demoted" -> "relay:demoted"
                    "neighbor_discovered" -> "neighbor:discovered"
                    "neighbor_lost" -> "neighbor:lost"
                    "network_metrics" -> "network:metrics"
                    "file_progress" -> "file:progress"
                    "file_received" -> "file:received"
                    else -> eventType
                }

                // Convert JSON object to WritableMap
                val eventMap = Arguments.createMap()
                convertJsonToMap(jsonObject, eventMap)
                eventMap.putString("type", jsEventType)

                // Emit event to JavaScript
                reactContext
                    .getJSModule(DeviceEventManagerModule.RCTDeviceEventEmitter::class.java)
                    .emit("OfflineProtocolEvent", eventMap)
            } catch (e: Exception) {
                // Ignore parsing errors
            }
        }
    }

    /**
     * Converts a JSONObject to a WritableMap recursively.
     */
    private fun convertJsonToMap(jsonObject: org.json.JSONObject, map: WritableMap) {
        val iterator = jsonObject.keys()
        while (iterator.hasNext()) {
            val key = iterator.next()
            val value = jsonObject.get(key)

            when (value) {
                is Boolean -> map.putBoolean(key, value)
                is Int -> map.putInt(key, value)
                is Long -> map.putDouble(key, value.toDouble())
                is Double -> map.putDouble(key, value)
                is String -> map.putString(key, value)
                is org.json.JSONObject -> {
                    val nestedMap = Arguments.createMap()
                    convertJsonToMap(value, nestedMap)
                    map.putMap(key, nestedMap)
                }
                is org.json.JSONArray -> {
                    val array = Arguments.createArray()
                    for (i in 0 until value.length()) {
                        when (val item = value.get(i)) {
                            is Boolean -> array.pushBoolean(item)
                            is Int -> array.pushInt(item)
                            is Long -> array.pushDouble(item.toDouble())
                            is Double -> array.pushDouble(item)
                            is String -> array.pushString(item)
                            else -> array.pushString(item.toString())
                        }
                    }
                    map.putArray(key, array)
                }
                else -> map.putString(key, value.toString())
            }
        }
    }

    override fun onCatalystInstanceDestroy() {
        super.onCatalystInstanceDestroy()
        if (protocolHandle != 0L) {
            stopEventPolling()
            nativeDestroy(protocolHandle)
            protocolHandle = 0
        }
    }

    // Native method declarations
    private external fun nativeCreate(configJson: String): Long
    private external fun nativeDestroy(handle: Long)
    private external fun nativeStart(handle: Long): Int
    private external fun nativeStop(handle: Long): Int
    private external fun nativeSendMessage(
        handle: Long,
        recipient: String,
        content: String,
        priority: Int,
        outMessageId: ByteArray,
        outLen: Int
    ): Int
    private external fun nativePollEvent(
        handle: Long,
        outEventJson: ByteArray,
        outLen: Int
    ): Int
}

