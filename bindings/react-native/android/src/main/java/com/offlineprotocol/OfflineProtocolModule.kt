package com.offlineprotocol

import com.facebook.react.bridge.*
import com.facebook.react.modules.core.DeviceEventManagerModule
import org.json.JSONObject
import java.util.concurrent.Executors
import java.util.concurrent.ScheduledExecutorService
import java.util.concurrent.TimeUnit

class OfflineProtocolModule(reactContext: ReactApplicationContext) :
    ReactContextBaseJavaModule(reactContext) {

    private var protocolHandle: Long = 0
    private var bleManager: BleManager? = null
    private var deviceId: String = ""
    private var bleSendScheduler: ScheduledExecutorService? = null
    private var processScheduler: ScheduledExecutorService? = null
    private val bleRecipientBuffer = ByteArray(BLE_RECIPIENT_BUFFER_SIZE)
    private val bleFragmentBuffer = ByteArray(BLE_FRAGMENT_BUFFER_SIZE)
    private var listenerCount: Int = 0
    private var bleBridgeInitialized: Boolean = false

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
        private const val BLE_RECIPIENT_BUFFER_SIZE = 512
        private const val BLE_FRAGMENT_BUFFER_SIZE = 65536

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
        stopProcessScheduler()
        if (bleBridgeInitialized) {
            nativeCleanupBleBridge()
            bleBridgeInitialized = false
        }
        if (protocolHandle != 0L) {
            nativeDestroy(protocolHandle)
            protocolHandle = 0
        }
        blePeerRefreshTimestamps.clear()
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
            blePeerRefreshTimestamps.clear()
            initializeBleManager()

            // Create new protocol instance
            // Note: This doesn't initialize Bluetooth yet, just creates the protocol object
            val handle = nativeCreate(configJson)
            if (handle == 0L) {
                promise.reject("ERROR_CREATE_FAILED", "Failed to create protocol instance")
                return
            }

            protocolHandle = handle
            
            // Start process scheduler for retries and cleanup
            startProcessScheduler()
            
            // Optionally enable Internet and WiFi Direct transports based on config
            try {
                val transports = config.optJSONObject("transports")
                
                // Enable Internet transport if configured
                val internetConfig = transports?.optJSONObject("internet")
                if (internetConfig?.optBoolean("enabled", false) == true) {
                    val internetConfigJson = internetConfig.toString()
                    val result = nativeAddInternetTransport(handle, internetConfigJson)
                    if (result == SUCCESS) {
                        android.util.Log.d(NAME, "Internet transport enabled")
                    } else {
                        android.util.Log.w(NAME, "Failed to enable Internet transport: $result")
                    }
                }
                
                // Enable WiFi Direct transport if configured
                val wifiDirectConfig = transports?.optJSONObject("wifiDirect")
                if (wifiDirectConfig?.optBoolean("enabled", false) == true) {
                    val wifiDirectConfigJson = wifiDirectConfig.toString()
                    val result = nativeAddWifiDirectTransport(handle, wifiDirectConfigJson)
                    if (result == SUCCESS) {
                        android.util.Log.d(NAME, "WiFi Direct transport enabled")
                    } else {
                        android.util.Log.w(NAME, "Failed to enable WiFi Direct transport: $result")
                    }
                }
            } catch (e: Exception) {
                android.util.Log.w(NAME, "Error enabling additional transports: ${e.message}")
                // Don't fail the entire create, just log the warning
            }
            
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
            stopProcessScheduler()
            bleManager?.stop()
            blePeerRefreshTimestamps.clear()
            bleManager = null
            if (bleBridgeInitialized) {
                nativeCleanupBleBridge()
                bleBridgeInitialized = false
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
            blePeerRefreshTimestamps.clear()
            
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

    private fun startProcessScheduler() {
        if (processScheduler != null) {
            return
        }

        val scheduler = Executors.newSingleThreadScheduledExecutor { runnable ->
            Thread(runnable, "offlineprotocol-processor").apply { isDaemon = true }
        }

        scheduler.scheduleAtFixedRate({
            try {
                val handle = protocolHandle
                if (handle != 0L) {
                    nativeProcess(handle)
                }
            } catch (t: Throwable) {
                android.util.Log.e(NAME, "Error while processing protocol", t)
            }
        }, 500, 500, TimeUnit.MILLISECONDS)

        processScheduler = scheduler
    }

    private fun stopProcessScheduler() {
        processScheduler?.shutdownNow()
        processScheduler = null
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
                recordBleSendFailure()
                val requeueResult = nativeBleReturnFragment(handle, recipient, payload, fragmentLength)
                if (requeueResult != SUCCESS) {
                    android.util.Log.e(NAME, "Failed to requeue BLE fragment: $requeueResult")
                }
                break
            } else {
                recordBleSendSuccess()
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

            android.util.Log.d(NAME, "Calling nativeSendMessage(recipient=$recipient, priority=$priority)")
            val messageId = nativeSendMessage(protocolHandle, recipient, content, priority)
            android.util.Log.d(NAME, "nativeSendMessage returned: ${messageId ?: "null"}")
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

    @ReactMethod
    fun sendFile(
        filePath: String,
        recipient: String,
        fileName: String,
        promise: Promise
    ) {
        try {
            if (protocolHandle == 0L) {
                promise.reject("ERROR_NOT_INITIALIZED", "Protocol not initialized")
                return
            }

            // Read file data
            val file = java.io.File(filePath)
            if (!file.exists()) {
                promise.reject("ERROR_FILE_NOT_FOUND", "File not found: $filePath")
                return
            }

            val fileData = file.readBytes()

            val fileId = nativeSendFile(protocolHandle, fileData, fileName, recipient)
            if (fileId == null) {
                promise.reject("ERROR_SEND_FILE_FAILED", "Failed to send file")
                return
            }

            promise.resolve(fileId)
        } catch (e: Exception) {
            android.util.Log.e(NAME, "Exception during sendFile: ${e.message}", e)
            promise.reject("ERROR_SEND_FILE_FAILED", e.message, e)
        }
    }

    @ReactMethod
    fun getFileProgress(fileId: String, promise: Promise) {
        try {
            if (protocolHandle == 0L) {
                promise.reject("ERROR_NOT_INITIALIZED", "Protocol not initialized")
                return
            }

            val progressJson = nativeGetFileProgress(protocolHandle, fileId)
            if (progressJson == null) {
                promise.resolve(null)
            } else {
                val jsonObject = JSONObject(progressJson)
                val map = Arguments.createMap().apply {
                    putString("file_id", jsonObject.optString("file_id"))
                    putString("file_name", jsonObject.optString("file_name"))
                    putDouble("file_size", jsonObject.optDouble("file_size"))
                    putInt("chunks_completed", jsonObject.optInt("chunks_completed"))
                    putInt("total_chunks", jsonObject.optInt("total_chunks"))
                    putInt("percentage", jsonObject.optInt("percentage"))
                }
                promise.resolve(map)
            }
        } catch (e: Exception) {
            android.util.Log.e(NAME, "Exception during getFileProgress: ${e.message}", e)
            promise.reject("ERROR_GET_PROGRESS_FAILED", e.message, e)
        }
    }

    @ReactMethod
    fun cancelFileTransfer(fileId: String, promise: Promise) {
        try {
            if (protocolHandle == 0L) {
                promise.reject("ERROR_NOT_INITIALIZED", "Protocol not initialized")
                return
            }

            val result = nativeCancelFileTransfer(protocolHandle, fileId)
            promise.resolve(result > 0)
        } catch (e: Exception) {
            android.util.Log.e(NAME, "Exception during cancelFileTransfer: ${e.message}", e)
            promise.reject("ERROR_CANCEL_FAILED", e.message, e)
        }
    }

    @ReactMethod
    fun receiveMessage(promise: Promise) {
        try {
            if (protocolHandle == 0L) {
                promise.reject("ERROR_NOT_INITIALIZED", "Protocol not initialized")
                return
            }

            val messageJson = nativeReceiveMessage(protocolHandle)
            if (messageJson == null) {
                promise.resolve(null)
            } else {
                val jsonObject = JSONObject(messageJson)
                val map = Arguments.createMap().apply {
                    putString("id", jsonObject.optString("id"))
                    putString("sender", jsonObject.optString("sender"))
                    putString("recipient", jsonObject.optString("recipient"))
                    putString("content", jsonObject.optString("content"))
                    putDouble("timestamp", jsonObject.optDouble("timestamp"))
                    putInt("hop_count", jsonObject.optInt("hop_count"))
                }
                promise.resolve(map)
            }
        } catch (e: Exception) {
            android.util.Log.e(NAME, "Exception during receiveMessage: ${e.message}", e)
            promise.reject("ERROR_RECEIVE_FAILED", e.message, e)
        }
    }

    @ReactMethod
    fun pause(promise: Promise) {
        try {
            if (protocolHandle == 0L) {
                promise.reject("ERROR_NOT_INITIALIZED", "Protocol not initialized")
                return
            }

            val result = nativePause(protocolHandle)
            when (result) {
                SUCCESS -> promise.resolve(null)
                ERROR_NOT_STARTED -> promise.reject("ERROR_NOT_STARTED", "Protocol not started")
                else -> promise.reject("ERROR_PAUSE_FAILED", "Failed to pause protocol")
            }
        } catch (e: Exception) {
            android.util.Log.e(NAME, "Exception during pause: ${e.message}", e)
            promise.reject("ERROR_PAUSE_FAILED", e.message, e)
        }
    }

    @ReactMethod
    fun resume(promise: Promise) {
        try {
            if (protocolHandle == 0L) {
                promise.reject("ERROR_NOT_INITIALIZED", "Protocol not initialized")
                return
            }

            val result = nativeResume(protocolHandle)
            when (result) {
                SUCCESS -> promise.resolve(null)
                else -> promise.reject("ERROR_RESUME_FAILED", "Failed to resume protocol")
            }
        } catch (e: Exception) {
            android.util.Log.e(NAME, "Exception during resume: ${e.message}", e)
            promise.reject("ERROR_RESUME_FAILED", e.message, e)
        }
    }

    @ReactMethod
    fun getState(promise: Promise) {
        try {
            if (protocolHandle == 0L) {
                promise.reject("ERROR_NOT_INITIALIZED", "Protocol not initialized")
                return
            }

            val state = nativeGetState(protocolHandle)
            if (state < 0) {
                promise.reject("ERROR_GET_STATE_FAILED", "Failed to get protocol state")
            } else {
                promise.resolve(state)
            }
        } catch (e: Exception) {
            android.util.Log.e(NAME, "Exception during getState: ${e.message}", e)
            promise.reject("ERROR_GET_STATE_FAILED", e.message, e)
        }
    }

    @ReactMethod
    fun enableTransport(type: String, config: ReadableMap?, promise: Promise) {
        try {
            if (protocolHandle == 0L) {
                promise.reject("ERROR_NOT_INITIALIZED", "Protocol not initialized")
                return
            }

            val configJson = config?.let { 
                org.json.JSONObject(it.toHashMap()).toString() 
            }

            val transportType = when (type) {
                "internet" -> 0
                "ble" -> 1
                "wifiDirect" -> 2
                else -> {
                    promise.reject("ERROR_INVALID_TRANSPORT", "Invalid transport type: $type")
                    return
                }
            }

            val result = when (type) {
                "internet" -> nativeAddInternetTransport(protocolHandle, configJson)
                "wifiDirect" -> nativeAddWifiDirectTransport(protocolHandle, configJson)
                else -> SUCCESS // BLE is always enabled
            }

            when (result) {
                SUCCESS -> promise.resolve(null)
                else -> promise.reject("ERROR_ENABLE_TRANSPORT_FAILED", "Failed to enable transport")
            }
        } catch (e: Exception) {
            android.util.Log.e(NAME, "Exception during enableTransport: ${e.message}", e)
            promise.reject("ERROR_ENABLE_TRANSPORT_FAILED", e.message, e)
        }
    }

    @ReactMethod
    fun disableTransport(type: String, promise: Promise) {
        try {
            if (protocolHandle == 0L) {
                promise.reject("ERROR_NOT_INITIALIZED", "Protocol not initialized")
                return
            }

            val transportType = when (type) {
                "internet" -> 0
                "ble" -> 1
                "wifiDirect" -> 2
                else -> {
                    promise.reject("ERROR_INVALID_TRANSPORT", "Invalid transport type: $type")
                    return
                }
            }

            val result = nativeRemoveTransport(protocolHandle, transportType)
            when (result) {
                SUCCESS -> promise.resolve(null)
                else -> promise.reject("ERROR_DISABLE_TRANSPORT_FAILED", "Failed to disable transport")
            }
        } catch (e: Exception) {
            android.util.Log.e(NAME, "Exception during disableTransport: ${e.message}", e)
            promise.reject("ERROR_DISABLE_TRANSPORT_FAILED", e.message, e)
        }
    }

    @ReactMethod
    fun getActiveTransports(promise: Promise) {
        try {
            if (protocolHandle == 0L) {
                promise.reject("ERROR_NOT_INITIALIZED", "Protocol not initialized")
                return
            }

            val transportsJson = nativeGetActiveTransports(protocolHandle)
            if (transportsJson == null) {
                promise.reject("ERROR_GET_TRANSPORTS_FAILED", "Failed to get active transports")
                return
            }

            // Parse JSON array and return as array
            val jsonArray = org.json.JSONArray(transportsJson)
            val transports = Arguments.createArray()
            for (i in 0 until jsonArray.length()) {
                transports.pushString(jsonArray.getString(i))
            }
            promise.resolve(transports)
        } catch (e: Exception) {
            android.util.Log.e(NAME, "Exception during getActiveTransports: ${e.message}", e)
            promise.reject("ERROR_GET_TRANSPORTS_FAILED", e.message, e)
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
    private var bleSendSuccessCount = 0
    private var bleSendFailureCount = 0
    private var bleLastRssi: Short = -1
    private val blePeerRefreshTimestamps = mutableMapOf<String, Long>()
    private val blePeerUpdateThrottleMs = 2000L

    private fun updateBleMetrics(rssi: Short = -1) {
        if (protocolHandle == 0L) return
        
        // Update RSSI if provided
        if (rssi != (-1).toShort()) {
            bleLastRssi = rssi
        }
        
        // Transport type 1 = BLE
        val result = nativeUpdateTransportMetrics(
            protocolHandle,
            transportType = 1, // BLE
            rssi = bleLastRssi,
            latencyMs = 0, // Not tracking yet
            bandwidthBps = 150_000, // Typical BLE bandwidth ~150 KB/s
            congestion = 0.0f, // Could calculate based on queue depth
            queueDepth = 0, // BLE queue is managed in Rust
            successCount = bleSendSuccessCount,
            failureCount = bleSendFailureCount
        )
        
        if (result != SUCCESS) {
            android.util.Log.d(NAME, "Failed to update BLE metrics: $result")
        }
    }

    private fun recordBleSendSuccess() {
        bleSendSuccessCount++
        updateBleMetrics()
    }

    private fun recordBleSendFailure() {
        bleSendFailureCount++
        updateBleMetrics()
    }

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
                    
                    // Update BLE metrics with RSSI value
                    updateBleMetrics(rssi.toShort())
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
            onPeerUpdated = { peerId, address, rssi ->
                val now = System.currentTimeMillis()
                val last = blePeerRefreshTimestamps[peerId]
                if (last == null || now - last >= blePeerUpdateThrottleMs) {
                    blePeerRefreshTimestamps[peerId] = now

                    if (protocolHandle != 0L) {
                        val result = nativeBlePeerDiscovered(protocolHandle, peerId, address, rssi.toShort())
                        if (result != SUCCESS) {
                            android.util.Log.e(NAME, "Failed to refresh BLE peer discovery: $result")
                        }
                        updateBleMetrics(rssi.toShort())
                    }
                }
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
                
                // Note: transport_switched events are now emitted by Rust DORS core
                // No need to synthesize them here
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

        try {
            bleManager?.let {
                nativeInitBleBridge(it)
                bleBridgeInitialized = true
                android.util.Log.d(NAME, "BLE bridge bound to BleManager instance")
            }
        } catch (e: Exception) {
            android.util.Log.e(NAME, "Failed to initialize BLE bridge: ${e.message}", e)
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
    
    // Visualization methods
    private external fun nativeGetTopology(handle: Long): String?
    private external fun nativeGetMessageStats(handle: Long): String?
    private external fun nativeGetDeliverySuccessRate(handle: Long): Float
    private external fun nativeGetMedianLatency(handle: Long): Long
    private external fun nativeGetMedianHops(handle: Long): Int
    
    // File transfer methods
    private external fun nativeSendFile(handle: Long, fileData: ByteArray, fileName: String, recipient: String): String?
    private external fun nativeGetFileProgress(handle: Long, fileId: String): String?
    private external fun nativeCancelFileTransfer(handle: Long, fileId: String): Int
    
    // Process and state methods
    private external fun nativeProcess(handle: Long): Int
    private external fun nativePause(handle: Long): Int
    private external fun nativeResume(handle: Long): Int
    private external fun nativeGetState(handle: Long): Int
    private external fun nativeReceiveMessage(handle: Long): String?
    
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
    
    // Transport management methods
    private external fun nativeUpdateTransportMetrics(
        handle: Long,
        transportType: Int,
        rssi: Short,
        latencyMs: Int,
        bandwidthBps: Long,
        congestion: Float,
        queueDepth: Int,
        successCount: Int,
        failureCount: Int
    ): Int
    private external fun nativeShouldEscalateToWifi(handle: Long): Int
    private external fun nativeAddInternetTransport(handle: Long, configJson: String?): Int
    private external fun nativeAddWifiDirectTransport(handle: Long, configJson: String?): Int
    private external fun nativeRemoveTransport(handle: Long, transportType: Int): Int
    private external fun nativeGetActiveTransports(handle: Long): String?
}
