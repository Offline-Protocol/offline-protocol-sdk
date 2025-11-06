package com.offlineprotocol

import com.facebook.react.bridge.*
import com.facebook.react.modules.core.DeviceEventManagerModule
import java.util.concurrent.Executors
import java.util.concurrent.ScheduledExecutorService
import java.util.concurrent.TimeUnit

class OfflineProtocolModule(reactContext: ReactApplicationContext) :
    ReactContextBaseJavaModule(reactContext) {

    private var protocolHandle: Long = 0
    private var bleManager: BleManager? = null
    private var deviceId: String = ""
    private var bleSendScheduler: ScheduledExecutorService? = null
    private val bleRecipientBuffer = ByteArray(256)
    private val bleFragmentBuffer = ByteArray(512)
    private var listenerCount: Int = 0

    companion object {
        const val NAME = "OfflineProtocolModule"
        const val EVENT_NAME = "OfflineProtocol_Event"
        
        // Error codes
        const val SUCCESS = 0
        const val NO_FRAGMENT_AVAILABLE = 1
        const val ERROR_NULL_POINTER = -1
        const val ERROR_NOT_STARTED = -3
        const val ERROR_ALREADY_STARTED = -4
        const val ERROR_SEND_FAILED = -5

        private var nativeLibraryLoaded = false
        private var nativeLibraryError: String? = null

        init {
            try {
                // Load the Rust FFI library first
                android.util.Log.d(NAME, "Loading native library offline_protocol_ffi...")
                System.loadLibrary("offline_protocol_ffi")
                android.util.Log.d(NAME, "Loaded offline_protocol_ffi successfully")
                
                // Then load the JNI wrapper which depends on it
                android.util.Log.d(NAME, "Loading native library offline_protocol_jni...")
                System.loadLibrary("offline_protocol_jni")
                nativeLibraryLoaded = true
                android.util.Log.d(NAME, "Native library loaded successfully")
            } catch (e: UnsatisfiedLinkError) {
                nativeLibraryError = "Failed to load native library: ${e.message}"
                android.util.Log.e(NAME, nativeLibraryError!!, e)
            } catch (e: Exception) {
                nativeLibraryError = "Unexpected error loading native library: ${e.message}"
                android.util.Log.e(NAME, nativeLibraryError!!, e)
            }
        }
    }

    override fun getName(): String = NAME

    override fun invalidate() {
        super.invalidate()
        stopBleFragmentPump()
        if (protocolHandle != 0L) {
            nativeDestroy(protocolHandle)
            protocolHandle = 0
        }
    }

    @ReactMethod
    fun addListener(eventName: String) {
        listenerCount += 1
    }

    @ReactMethod
    fun removeListeners(count: Double) {
        listenerCount = (listenerCount - count.toInt()).coerceAtLeast(0)
    }

    @ReactMethod
    fun create(configJson: String, promise: Promise) {
        // Check if native library loaded successfully
        if (!nativeLibraryLoaded) {
            promise.reject("ERROR_NATIVE_LIBRARY", nativeLibraryError ?: "Native library not loaded")
            return
        }

        try {
            // Clean up existing handle
            if (protocolHandle != 0L) {
                stopBleFragmentPump()
                nativeDestroy(protocolHandle)
            }

            // Parse config to extract userId
            val config = org.json.JSONObject(configJson)
            deviceId = config.optString("userId", config.optString("user_id", ""))
            
            if (deviceId.isEmpty()) {
                promise.reject("ERROR_INVALID_CONFIG", "userId is required")
                return
            }

            // Initialize BLE manager
            initializeBleManager()

            // Create new protocol instance
            // Note: This doesn't initialize Bluetooth yet, just creates the protocol object
            val handle = nativeCreate(configJson)
            if (handle == 0L) {
                promise.reject("ERROR_CREATE_FAILED", "Failed to create protocol instance")
                return
            }

            protocolHandle = handle
            promise.resolve(null)
        } catch (e: SecurityException) {
            // Permission error - can happen if permissions are revoked after app start
            android.util.Log.e(NAME, "Security exception during create: ${e.message}", e)
            promise.reject("ERROR_PERMISSION_DENIED", "Bluetooth permissions not granted: ${e.message}", e)
        } catch (e: Exception) {
            android.util.Log.e(NAME, "Exception during create: ${e.message}", e)
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

            // Stop and cleanup BLE
            stopBleFragmentPump()
            bleManager?.stop()
            bleManager = null

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

            android.util.Log.d(NAME, "Starting protocol...")
            
            // Start BLE operations
            val bleStarted = bleManager?.start() ?: false
            if (!bleStarted) {
                android.util.Log.e(NAME, "Failed to start BLE")
                promise.reject("ERROR_BLE_START_FAILED", "Failed to start BLE. Check permissions and Bluetooth state.")
                return
            }
            
            // Start protocol
            val result = nativeStart(protocolHandle)
            when (result) {
                SUCCESS -> {
                    android.util.Log.d(NAME, "Protocol started successfully")
                    startBleFragmentPump()
                    promise.resolve(null)
                }
                ERROR_ALREADY_STARTED -> {
                    android.util.Log.w(NAME, "Protocol already started")
                    promise.reject(
                        "ERROR_ALREADY_STARTED",
                        "Protocol already started"
                    )
                }
                else -> {
                    android.util.Log.e(NAME, "Failed to start protocol, error code: $result")
                    promise.reject("ERROR_START_FAILED", "Failed to start protocol (error code: $result)")
                }
            }
        } catch (e: SecurityException) {
            // This is the most common error when permissions are missing
            android.util.Log.e(NAME, "Security exception during start: ${e.message}", e)
            promise.reject(
                "ERROR_PERMISSION_DENIED",
                "Bluetooth permissions not granted. Please grant Bluetooth and Location permissions in Settings.",
                e
            )
        } catch (e: IllegalStateException) {
            // Can occur if Bluetooth adapter is not available or in bad state
            android.util.Log.e(NAME, "Illegal state during start: ${e.message}", e)
            promise.reject(
                "ERROR_BLUETOOTH_UNAVAILABLE",
                "Bluetooth is not available or disabled. Please enable Bluetooth and try again.",
                e
            )
        } catch (e: Exception) {
            android.util.Log.e(NAME, "Exception during start: ${e.message}", e)
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

            // Stop BLE first
            stopBleFragmentPump()
            bleManager?.stop()
            
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

    private fun startBleFragmentPump() {
        if (bleSendScheduler != null) {
            return
        }

        val scheduler = Executors.newSingleThreadScheduledExecutor { runnable ->
            Thread(runnable, "offlineprotocol-ble-sender").apply { isDaemon = true }
        }

        scheduler.scheduleAtFixedRate({
            try {
                flushBleFragments()
            } catch (t: Throwable) {
                android.util.Log.e(NAME, "Error while flushing BLE fragments", t)
            }
        }, 0, 150, TimeUnit.MILLISECONDS)

        bleSendScheduler = scheduler
    }

    private fun stopBleFragmentPump() {
        bleSendScheduler?.shutdownNow()
        bleSendScheduler = null
    }

    private fun flushBleFragments() {
        val handle = protocolHandle
        val manager = bleManager ?: return

        if (handle == 0L) {
            return
        }

        while (true) {
            val fragmentLength = nativeBleGetNextFragment(handle, bleRecipientBuffer, bleFragmentBuffer)

            if (fragmentLength == NO_FRAGMENT_AVAILABLE || fragmentLength == 0) {
                break
            }

            if (fragmentLength < 0) {
                android.util.Log.e(NAME, "Failed to fetch BLE fragment: $fragmentLength")
                break
            }

            val terminatorIndex = bleRecipientBuffer.indexOf(0)
            val recipient = if (terminatorIndex >= 0) {
                String(bleRecipientBuffer, 0, terminatorIndex, Charsets.UTF_8)
            } else {
                String(bleRecipientBuffer, Charsets.UTF_8).trimEnd('\u0000')
            }

            val payload = bleFragmentBuffer.copyOfRange(0, fragmentLength)

            val sendSucceeded = nativeSendBleMessage(recipient, payload)
            if (!sendSucceeded) {
                val requeueResult = nativeBleReturnFragment(handle, recipient, payload, fragmentLength)
                if (requeueResult != SUCCESS) {
                    android.util.Log.e(NAME, "Failed to requeue BLE fragment: $requeueResult")
                }
                break
            }
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
        } catch (e: SecurityException) {
            android.util.Log.e(NAME, "Security exception during sendMessage: ${e.message}", e)
            promise.reject("ERROR_PERMISSION_DENIED", "Bluetooth permissions not granted", e)
        } catch (e: Exception) {
            android.util.Log.e(NAME, "Exception during sendMessage: ${e.message}", e)
            promise.reject("ERROR_SEND_FAILED", e.message, e)
        }
    }

    @ReactMethod
    fun getTopology(promise: Promise) {
        try {
            if (protocolHandle == 0L) {
                promise.reject("ERROR_NOT_INITIALIZED", "Protocol not initialized")
                return
            }

            val topologyJson = nativeGetTopology(protocolHandle)
            if (topologyJson == null) {
                promise.reject("ERROR_GET_TOPOLOGY_FAILED", "Failed to get topology")
                return
            }

            promise.resolve(topologyJson)
        } catch (e: Exception) {
            android.util.Log.e(NAME, "Exception during getTopology: ${e.message}", e)
            promise.reject("ERROR_GET_TOPOLOGY_FAILED", e.message, e)
        }
    }

    @ReactMethod
    fun getMessageStats(promise: Promise) {
        try {
            if (protocolHandle == 0L) {
                promise.reject("ERROR_NOT_INITIALIZED", "Protocol not initialized")
                return
            }

            val statsJson = nativeGetMessageStats(protocolHandle)
            if (statsJson == null) {
                promise.reject("ERROR_GET_STATS_FAILED", "Failed to get message stats")
                return
            }

            promise.resolve(statsJson)
        } catch (e: Exception) {
            android.util.Log.e(NAME, "Exception during getMessageStats: ${e.message}", e)
            promise.reject("ERROR_GET_STATS_FAILED", e.message, e)
        }
    }

    @ReactMethod
    fun getDeliverySuccessRate(promise: Promise) {
        try {
            if (protocolHandle == 0L) {
                promise.reject("ERROR_NOT_INITIALIZED", "Protocol not initialized")
                return
            }

            val rate = nativeGetDeliverySuccessRate(protocolHandle)
            promise.resolve(rate)
        } catch (e: Exception) {
            android.util.Log.e(NAME, "Exception during getDeliverySuccessRate: ${e.message}", e)
            promise.reject("ERROR_GET_RATE_FAILED", e.message, e)
        }
    }

    @ReactMethod
    fun getMedianLatency(promise: Promise) {
        try {
            if (protocolHandle == 0L) {
                promise.reject("ERROR_NOT_INITIALIZED", "Protocol not initialized")
                return
            }

            val latency = nativeGetMedianLatency(protocolHandle)
            if (latency < 0) {
                promise.resolve(null)
            } else {
                promise.resolve(latency)
            }
        } catch (e: Exception) {
            android.util.Log.e(NAME, "Exception during getMedianLatency: ${e.message}", e)
            promise.reject("ERROR_GET_LATENCY_FAILED", e.message, e)
        }
    }

    @ReactMethod
    fun getMedianHops(promise: Promise) {
        try {
            if (protocolHandle == 0L) {
                promise.reject("ERROR_NOT_INITIALIZED", "Protocol not initialized")
                return
            }

            val hops = nativeGetMedianHops(protocolHandle)
            if (hops < 0) {
                promise.resolve(null)
            } else {
                promise.resolve(hops)
            }
        } catch (e: Exception) {
            android.util.Log.e(NAME, "Exception during getMedianHops: ${e.message}", e)
            promise.reject("ERROR_GET_HOPS_FAILED", e.message, e)
        }
    }

    // Called from JNI
    @Suppress("unused")
    fun handleEvent(eventJson: String) {
        try {
            if (listenerCount == 0) {
                return
            }
            reactApplicationContext
                .getJSModule(DeviceEventManagerModule.RCTDeviceEventEmitter::class.java)
                .emit(EVENT_NAME, Arguments.createMap().apply {
                    putString("eventJson", eventJson)
                })
        } catch (e: SecurityException) {
            // Permission error while handling events
            android.util.Log.e(NAME, "Security exception handling event: ${e.message}", e)
        } catch (e: Exception) {
            android.util.Log.e(NAME, "Error handling event: ${e.message}", e)
        }
    }

    // Initialize BLE manager
    private fun initializeBleManager() {
        if (bleManager != null) {
            return // Already initialized
        }

        bleManager = BleManager(
            context = reactApplicationContext,
            deviceId = deviceId,
            onPeerDiscovered = { peerId, address, rssi ->
                android.util.Log.d(NAME, "Peer discovered: $peerId at $address (RSSI: $rssi)")
                
                // Notify the Rust transport layer
                if (protocolHandle != 0L) {
                    val result = nativeBlePeerDiscovered(protocolHandle, peerId, address, rssi.toShort())
                    if (result != SUCCESS) {
                        android.util.Log.e(NAME, "Failed to notify BLE transport of peer discovery: $result")
                    } else {
                        android.util.Log.d(NAME, "Successfully notified Rust transport of peer: $peerId")
                    }
                }
                
                // Emit neighbor_discovered event (matches NetworkScreen expectations)
                val eventJson = """
                    {
                        "type": "neighbor_discovered",
                        "peer_id": "$peerId",
                        "transport": "ble",
                        "rssi": $rssi,
                        "timestamp": ${System.currentTimeMillis()}
                    }
                """.trimIndent()
                
                handleEvent(eventJson)
            },
            onPeerLost = { peerId ->
                android.util.Log.d(NAME, "Peer lost: $peerId")
                
                // Notify the Rust transport layer
                if (protocolHandle != 0L) {
                    val result = nativeBlePeerLost(protocolHandle, peerId)
                    if (result != SUCCESS) {
                        android.util.Log.e(NAME, "Failed to notify BLE transport of peer loss: $result")
                    }
                }
                
                // Emit neighbor_lost event (matches NetworkScreen expectations)
                val eventJson = """
                    {
                        "type": "neighbor_lost",
                        "peer_id": "$peerId",
                        "timestamp": ${System.currentTimeMillis()}
                    }
                """.trimIndent()
                
                handleEvent(eventJson)
            },
            onMessageReceived = { messageData ->
                android.util.Log.d(NAME, "Message received: ${messageData.size} bytes")
                
                if (protocolHandle != 0L) {
                    val result = nativeBleFragmentReceived(protocolHandle, messageData)
                    if (result != SUCCESS) {
                        android.util.Log.e(NAME, "Failed to forward BLE fragment to Rust: $result")
                        try {
                            val messageJson = String(messageData)
                            handleEvent(messageJson)
                        } catch (e: Exception) {
                            android.util.Log.e(NAME, "Failed to parse received message", e)
                        }
                    }
                } else {
                    try {
                        val messageJson = String(messageData)
                        handleEvent(messageJson)
                    } catch (e: Exception) {
                        android.util.Log.e(NAME, "Failed to parse received message", e)
                    }
                }
            },
            onStatusChanged = { status ->
                android.util.Log.d(NAME, "BLE status changed: $status")
                
                // Notify the Rust transport layer
                if (protocolHandle != 0L) {
                    val statusCode = when (status) {
                        BleManager.Status.UNAVAILABLE -> 0
                        BleManager.Status.AVAILABLE, BleManager.Status.SCANNING, 
                        BleManager.Status.ADVERTISING, BleManager.Status.CONNECTED -> 1
                        BleManager.Status.DISCONNECTED -> 2
                    }
                    
                    val result = nativeBleStatusChanged(protocolHandle, statusCode)
                    if (result != SUCCESS) {
                        android.util.Log.e(NAME, "Failed to notify BLE transport of status change: $result")
                    }
                }
                
                // Emit transport_switched event when BLE becomes available
                when (status) {
                    BleManager.Status.AVAILABLE, BleManager.Status.SCANNING, 
                    BleManager.Status.ADVERTISING -> {
                        val eventJson = """
                            {
                                "type": "transport_switched",
                                "from": null,
                                "to": "ble",
                                "reason": "BLE transport became available",
                                "timestamp": ${System.currentTimeMillis()}
                            }
                        """.trimIndent()
                        handleEvent(eventJson)
                    }
                    BleManager.Status.DISCONNECTED -> {
                        val eventJson = """
                            {
                                "type": "transport_switched",
                                "from": "ble",
                                "to": "none",
                                "reason": "BLE transport disconnected",
                                "timestamp": ${System.currentTimeMillis()}
                            }
                        """.trimIndent()
                        handleEvent(eventJson)
                    }
                    else -> {
                        // Do nothing for other statuses
                    }
                }
            },
            onDiagnostic = { message ->
                android.util.Log.d(NAME, message)
                
                // Emit diagnostic event to React Native
                val eventJson = """
                    {
                        "type": "diagnostic",
                        "message": "$message",
                        "timestamp": ${System.currentTimeMillis()}
                    }
                """.trimIndent()
                
                handleEvent(eventJson)
            }
        )

        android.util.Log.d(NAME, "BLE manager initialized for device: $deviceId")
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
    
    // Visualization methods
    private external fun nativeGetTopology(handle: Long): String?
    private external fun nativeGetMessageStats(handle: Long): String?
    private external fun nativeGetDeliverySuccessRate(handle: Long): Float
    private external fun nativeGetMedianLatency(handle: Long): Long
    private external fun nativeGetMedianHops(handle: Long): Int
    
    // BLE bridge native methods
    private external fun nativeInitBleBridge(bleManager: BleManager)
    private external fun nativeStartBle(): Boolean
    private external fun nativeStopBle()
    private external fun nativeSendBleMessage(recipientId: String, messageData: ByteArray): Boolean
    private external fun nativeCleanupBleBridge()
    
    // BLE transport notification methods
    private external fun nativeBlePeerDiscovered(handle: Long, deviceId: String, address: String, rssi: Short): Int
    private external fun nativeBlePeerLost(handle: Long, deviceId: String): Int
    private external fun nativeBleStatusChanged(handle: Long, status: Int): Int
    private external fun nativeBleGetPeerCount(handle: Long): Int
    private external fun nativeBleFragmentReceived(handle: Long, fragmentData: ByteArray): Int
    private external fun nativeBleGetNextFragment(handle: Long, recipientBuffer: ByteArray, fragmentBuffer: ByteArray): Int
    private external fun nativeBleReturnFragment(handle: Long, recipient: String, fragmentData: ByteArray, fragmentLength: Int): Int
}
