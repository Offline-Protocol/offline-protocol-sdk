package com.offlineprotocol

import com.facebook.react.bridge.*
import com.facebook.react.modules.core.DeviceEventManagerModule

class OfflineProtocolModule(reactContext: ReactApplicationContext) :
    ReactContextBaseJavaModule(reactContext) {

    private var protocolHandle: Long = 0

    companion object {
        const val NAME = "OfflineProtocolModule"
        const val EVENT_NAME = "OfflineProtocol_Event"
        
        // Error codes
        const val SUCCESS = 0
        const val ERROR_NULL_POINTER = -1
        const val ERROR_NOT_STARTED = -3
        const val ERROR_ALREADY_STARTED = -4
        const val ERROR_SEND_FAILED = -5

        init {
            System.loadLibrary("offline_protocol_jni")
        }
    }

    override fun getName(): String = NAME

    override fun invalidate() {
        super.invalidate()
        if (protocolHandle != 0L) {
            nativeDestroy(protocolHandle)
            protocolHandle = 0
        }
    }

    @ReactMethod
    fun create(configJson: String, promise: Promise) {
        try {
            // Clean up existing handle
            if (protocolHandle != 0L) {
                nativeDestroy(protocolHandle)
            }

            // Create new protocol instance
            val handle = nativeCreate(configJson)
            if (handle == 0L) {
                promise.reject("ERROR_CREATE_FAILED", "Failed to create protocol instance")
                return
            }

            protocolHandle = handle
            promise.resolve(null)
        } catch (e: Exception) {
            promise.reject("ERROR_CREATE_FAILED", e.message, e)
        }
    }

    @ReactMethod
    fun destroy(promise: Promise) {
        try {
            if (protocolHandle == 0L) {
                promise.reject("ERROR_NOT_INITIALIZED", "Protocol not initialized")
                return
            }

            nativeDestroy(protocolHandle)
            protocolHandle = 0
            promise.resolve(null)
        } catch (e: Exception) {
            promise.reject("ERROR_DESTROY_FAILED", e.message, e)
        }
    }

    @ReactMethod
    fun start(promise: Promise) {
        try {
            if (protocolHandle == 0L) {
                promise.reject("ERROR_NOT_INITIALIZED", "Protocol not initialized")
                return
            }

            val result = nativeStart(protocolHandle)
            when (result) {
                SUCCESS -> promise.resolve(null)
                ERROR_ALREADY_STARTED -> promise.reject(
                    "ERROR_ALREADY_STARTED",
                    "Protocol already started"
                )
                else -> promise.reject("ERROR_START_FAILED", "Failed to start protocol")
            }
        } catch (e: Exception) {
            promise.reject("ERROR_START_FAILED", e.message, e)
        }
    }

    @ReactMethod
    fun stop(promise: Promise) {
        try {
            if (protocolHandle == 0L) {
                promise.reject("ERROR_NOT_INITIALIZED", "Protocol not initialized")
                return
            }

            val result = nativeStop(protocolHandle)
            when (result) {
                SUCCESS -> promise.resolve(null)
                ERROR_NOT_STARTED -> promise.reject(
                    "ERROR_NOT_STARTED",
                    "Protocol not started"
                )
                else -> promise.reject("ERROR_STOP_FAILED", "Failed to stop protocol")
            }
        } catch (e: Exception) {
            promise.reject("ERROR_STOP_FAILED", e.message, e)
        }
    }

    @ReactMethod
    fun sendMessage(
        recipient: String,
        content: String,
        priority: Int,
        promise: Promise
    ) {
        try {
            if (protocolHandle == 0L) {
                promise.reject("ERROR_NOT_INITIALIZED", "Protocol not initialized")
                return
            }

            val messageId = nativeSendMessage(protocolHandle, recipient, content, priority)
            if (messageId == null) {
                promise.reject("ERROR_SEND_FAILED", "Failed to send message")
                return
            }

            promise.resolve(messageId)
        } catch (e: Exception) {
            promise.reject("ERROR_SEND_FAILED", e.message, e)
        }
    }

    // Called from JNI
    @Suppress("unused")
    fun handleEvent(eventJson: String) {
        try {
            reactApplicationContext
                .getJSModule(DeviceEventManagerModule.RCTDeviceEventEmitter::class.java)
                .emit(EVENT_NAME, Arguments.createMap().apply {
                    putString("eventJson", eventJson)
                })
        } catch (e: Exception) {
            android.util.Log.e(NAME, "Error handling event: ${e.message}", e)
        }
    }

    // Native methods
    private external fun nativeCreate(configJson: String): Long
    private external fun nativeDestroy(handle: Long)
    private external fun nativeStart(handle: Long): Int
    private external fun nativeStop(handle: Long): Int
    private external fun nativeSendMessage(
        handle: Long,
        recipient: String,
        content: String,
        priority: Int
    ): String?
}
