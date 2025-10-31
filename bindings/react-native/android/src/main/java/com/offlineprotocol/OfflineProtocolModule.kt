package com.offlineprotocol

import com.facebook.react.bridge.*
import com.facebook.react.modules.core.DeviceEventManagerModule

/**
 * React Native module for Offline Protocol SDK (Android)
 * 
 * This module wraps the Rust FFI bindings and provides a JavaScript-accessible API.
 */
class OfflineProtocolModule(reactContext: ReactApplicationContext) :
    ReactContextBaseJavaModule(reactContext) {

    private var protocolHandle: Long = 0
    private val eventEmitter: DeviceEventManagerModule.RCTDeviceEventEmitter by lazy {
        reactContext.getJSModule(DeviceEventManagerModule.RCTDeviceEventEmitter::class.java)
    }

    override fun getName(): String {
        return "OfflineProtocol"
    }

    /**
     * Starts the protocol with the given configuration.
     * 
     * @param configJson JSON configuration string
     * @param promise Promise to resolve on success or reject on error
     */
    @ReactMethod
    fun start(configJson: String, promise: Promise) {
        try {
            // Call Rust FFI to create and start protocol
            protocolHandle = nativeCreate(configJson)
            
            if (protocolHandle == 0L) {
                promise.reject("INIT_ERROR", "Failed to create protocol instance")
                return
            }

            val result = nativeStart(protocolHandle)
            if (result == 0) {
                promise.resolve(null)
            } else {
                promise.reject("START_ERROR", "Failed to start protocol: error code $result")
            }
        } catch (e: Exception) {
            promise.reject("ERROR", e.message)
        }
    }

    /**
     * Stops the protocol.
     */
    @ReactMethod
    fun stop(promise: Promise) {
        try {
            if (protocolHandle == 0L) {
                promise.reject("NOT_STARTED", "Protocol not started")
                return
            }

            val result = nativeStop(protocolHandle)
            if (result == 0) {
                nativeDestroy(protocolHandle)
                protocolHandle = 0
                promise.resolve(null)
            } else {
                promise.reject("STOP_ERROR", "Failed to stop protocol: error code $result")
            }
        } catch (e: Exception) {
            promise.reject("ERROR", e.message)
        }
    }

    /**
     * Pauses the protocol (for background mode).
     */
    @ReactMethod
    fun pause(promise: Promise) {
        // TODO: Implement pause via FFI
        promise.resolve(null)
    }

    /**
     * Resumes the protocol from pause.
     */
    @ReactMethod
    fun resume(promise: Promise) {
        // TODO: Implement resume via FFI
        promise.resolve(null)
    }

    /**
     * Sends a message.
     * 
     * @param recipient Recipient user ID
     * @param content Message content
     * @param priority Message priority (0=Low, 1=Medium, 2=High, 3=Critical)
     * @param promise Promise resolving to message ID
     */
    @ReactMethod
    fun sendMessage(recipient: String, content: String, priority: Int, promise: Promise) {
        try {
            if (protocolHandle == 0L) {
                promise.reject("NOT_STARTED", "Protocol not started")
                return
            }

            val messageId = nativeSendMessage(protocolHandle, recipient, content, priority)
            if (messageId != null && messageId.isNotEmpty()) {
                promise.resolve(messageId)
            } else {
                promise.reject("SEND_ERROR", "Failed to send message")
            }
        } catch (e: Exception) {
            promise.reject("ERROR", e.message)
        }
    }

    /**
     * Sends a file.
     * 
     * @param recipient Recipient user ID
     * @param filePath Path to file
     * @param priority Message priority
     * @param promise Promise resolving to file ID
     */
    @ReactMethod
    fun sendFile(recipient: String, filePath: String, priority: Int, promise: Promise) {
        // TODO: Implement file transfer
        promise.reject("NOT_IMPLEMENTED", "File transfer not yet implemented")
    }

    /**
     * Native methods (JNI) - these call the Rust FFI layer
     */
    private external fun nativeCreate(configJson: String): Long
    private external fun nativeDestroy(handle: Long)
    private external fun nativeStart(handle: Long): Int
    private external fun nativeStop(handle: Long): Int
    private external fun nativeSendMessage(handle: Long, recipient: String, content: String, priority: Int): String?

    companion object {
        init {
            try {
                System.loadLibrary("offline_protocol")
            } catch (e: UnsatisfiedLinkError) {
                // Library not found - will fail at runtime but not at load time
                e.printStackTrace()
            }
        }
    }
}

