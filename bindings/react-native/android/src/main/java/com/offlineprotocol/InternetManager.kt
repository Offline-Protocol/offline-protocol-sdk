package com.offlineprotocol

import android.os.Handler
import android.os.Looper
import android.util.Log
import okhttp3.*
import uniffi.offline_protocol.OfflineProtocol
import java.util.concurrent.CountDownLatch
import java.util.concurrent.TimeUnit
import java.util.concurrent.atomic.AtomicBoolean
import java.util.concurrent.atomic.AtomicInteger

/**
 * Internet Manager implementing TransportManager for WebSocket communication
 * Connects to a relay server for internet-based message routing
 */
class InternetManager(
    private val context: android.content.Context,
    private val protocol: OfflineProtocol,
    private val deviceId: String,
    private val diagnosticEmitter: ((String, String, Map<String, Any?>) -> Unit)? = null
) : TransportManager {

    // MARK: - TransportManager Implementation
    
    override val transportId = "internet"
    override val transportName = "Internet (WebSocket)"
    override var state: TransportState = TransportState.UNAVAILABLE
        private set
    override var listener: TransportManagerListener? = null
    
    companion object {
        private const val TAG = "InternetManager"
        
        private const val MESSAGE_POLL_INTERVAL_MS = 100L
        private const val RECONNECT_INITIAL_DELAY_MS = 1000L
        private const val RECONNECT_MAX_DELAY_MS = 30000L
        private const val RECONNECT_BACKOFF_MULTIPLIER = 2.0
        private const val PING_INTERVAL_MS = 30000L
        private const val CONNECTION_TIMEOUT_MS = 10000L
    }
    
    // MARK: - Properties
    
    private var serverUrl: String? = null
    private var autoReconnect = true
    private var maxReconnectAttempts = 0 // 0 = infinite
    
    // OkHttp components
    private var okHttpClient: OkHttpClient? = null
    private var webSocket: WebSocket? = null
    
    // Handler for main thread operations
    private val mainHandler = Handler(Looper.getMainLooper())
    
    // Message polling runnable
    private val messagePollingRunnable = object : Runnable {
        override fun run() {
            pollAndSendMessages()
            if (state == TransportState.RUNNING && isConnected.get()) {
                mainHandler.postDelayed(this, MESSAGE_POLL_INTERVAL_MS)
            }
        }
    }
    
    // Ping runnable
    private val pingRunnable = object : Runnable {
        override fun run() {
            sendPing()
            if (state == TransportState.RUNNING && isConnected.get()) {
                mainHandler.postDelayed(this, PING_INTERVAL_MS)
            }
        }
    }
    
    // Connection state
    private val isConnected = AtomicBoolean(false)
    private val isConnecting = AtomicBoolean(false)
    private var reconnectAttempts = AtomicInteger(0)
    private var currentReconnectDelay = RECONNECT_INITIAL_DELAY_MS
    private var reconnectRunnable: Runnable? = null
    private var transportStartAt: Long = 0L
    
    // Metrics
    private var bytesSent: Long = 0
    private var bytesReceived: Long = 0
    private var messagesSent: Long = 0
    private var messagesReceived: Long = 0
    
    // MARK: - Helper
    
    private fun <T> runOnMainSync(action: () -> T): T {
        if (Looper.myLooper() == Looper.getMainLooper()) {
            return action()
        }
        
        val latch = CountDownLatch(1)
        var outcome: Result<T>? = null
        mainHandler.post {
            outcome = try {
                Result.success(action())
            } catch (t: Throwable) {
                Result.failure(t)
            }
            latch.countDown()
        }
        
        try {
            latch.await()
        } catch (ie: InterruptedException) {
            Thread.currentThread().interrupt()
            throw RuntimeException("Interrupted while executing on main thread", ie)
        }
        
        return outcome!!.getOrThrow()
    }
    
    // MARK: - Configuration
    
    /**
     * Configure the relay server URL
     */
    fun configure(
        serverUrl: String,
        autoReconnect: Boolean = true,
        maxReconnectAttempts: Int = 0
    ) {
        this.serverUrl = serverUrl
        this.autoReconnect = autoReconnect
        this.maxReconnectAttempts = maxReconnectAttempts
        
        emitDiagnostic("info", "Internet transport configured", mapOf(
            "serverUrl" to serverUrl,
            "autoReconnect" to autoReconnect,
            "maxReconnectAttempts" to maxReconnectAttempts
        ))
    }
    
    // MARK: - TransportManager Implementation
    
    override fun isAvailable(): Boolean {
        return !serverUrl.isNullOrBlank()
    }
    
    override fun start() {
        runOnMainSync {
            startUnsafe()
        }
    }
    
    private fun startUnsafe() {
        if (state == TransportState.RUNNING) {
            throw TransportException.AlreadyRunning()
        }
        
        val url = serverUrl
        if (url.isNullOrBlank()) {
            throw TransportException.NotAvailable("Server URL not configured. Call configure(serverUrl) first.")
        }
        
        Log.i(TAG, "Starting Internet transport for device: $deviceId")
        emitDiagnostic("info", "Starting Internet transport", mapOf(
            "deviceId" to deviceId,
            "serverUrl" to url
        ))
        
        updateState(TransportState.STARTING)
        transportStartAt = System.currentTimeMillis()
        
        // Create OkHttp client
        okHttpClient = OkHttpClient.Builder()
            .connectTimeout(CONNECTION_TIMEOUT_MS, TimeUnit.MILLISECONDS)
            .readTimeout(0, TimeUnit.MILLISECONDS) // No timeout for WebSocket
            .writeTimeout(CONNECTION_TIMEOUT_MS, TimeUnit.MILLISECONDS)
            .pingInterval(PING_INTERVAL_MS, TimeUnit.MILLISECONDS)
            .build()
        
        // Connect to WebSocket
        connect()
    }
    
    override fun stop() {
        runOnMainSync {
            stopUnsafe()
        }
    }
    
    private fun stopUnsafe() {
        if (state != TransportState.RUNNING && state != TransportState.STARTING) {
            return
        }
        
        updateState(TransportState.STOPPING)
        
        // Cancel reconnect attempts
        reconnectRunnable?.let { mainHandler.removeCallbacks(it) }
        reconnectRunnable = null
        
        // Stop timers
        stopMessagePolling()
        stopPingTimer()
        
        // Close WebSocket
        disconnect()
        
        // Notify protocol
        try {
            protocol.internetStatusChanged(false)
        } catch (e: Exception) {
            Log.e(TAG, "Error notifying protocol of disconnect", e)
        }
        
        // Shutdown OkHttp
        okHttpClient?.dispatcher?.executorService?.shutdown()
        okHttpClient = null
        
        updateState(TransportState.STOPPED)
        emitDiagnostic("info", "Internet transport stopped")
    }
    
    override fun pause() {
        runOnMainSync {
            stopMessagePolling()
            stopPingTimer()
        }
    }
    
    override fun resume() {
        runOnMainSync {
            if (state == TransportState.RUNNING && isConnected.get()) {
                startMessagePolling()
                startPingTimer()
            }
        }
    }
    
    override fun getMetrics(): Map<String, Any> {
        return mapOf(
            "bytes_sent" to bytesSent,
            "bytes_received" to bytesReceived,
            "messages_sent" to messagesSent,
            "messages_received" to messagesReceived,
            "is_connected" to isConnected.get(),
            "reconnect_attempts" to reconnectAttempts.get()
        )
    }
    
    // MARK: - Connection Management
    
    private fun connect() {
        val url = serverUrl ?: return
        if (isConnecting.get() || isConnected.get()) return
        
        isConnecting.set(true)
        
        val request = Request.Builder()
            .url(url)
            .addHeader("X-Device-ID", deviceId)
            .build()
        
        webSocket = okHttpClient?.newWebSocket(request, webSocketListener)
        
        emitDiagnostic("info", "Connecting to WebSocket", mapOf(
            "url" to url
        ))
    }
    
    private fun disconnect() {
        webSocket?.close(1000, "Client closing")
        webSocket = null
        isConnected.set(false)
        isConnecting.set(false)
    }
    
    private fun handleConnectionOpened() {
        isConnected.set(true)
        isConnecting.set(false)
        reconnectAttempts.set(0)
        currentReconnectDelay = RECONNECT_INITIAL_DELAY_MS
        
        mainHandler.post {
            updateState(TransportState.RUNNING)
        }
        
        // Notify protocol
        try {
            protocol.internetStatusChanged(true)
        } catch (e: Exception) {
            Log.e(TAG, "Error notifying protocol of connect", e)
        }
        
        // Start polling and pinging
        mainHandler.post {
            startMessagePolling()
            startPingTimer()
        }
        
        emitDiagnostic("info", "WebSocket connected", mapOf(
            "serverUrl" to (serverUrl ?: "unknown")
        ))
    }
    
    private fun handleConnectionClosed(code: Int, reason: String?) {
        val wasConnected = isConnected.getAndSet(false)
        isConnecting.set(false)
        
        mainHandler.post {
            stopMessagePolling()
            stopPingTimer()
        }
        
        if (wasConnected) {
            // Notify protocol of disconnection
            try {
                protocol.internetStatusChanged(false)
            } catch (e: Exception) {
                Log.e(TAG, "Error notifying protocol of disconnect", e)
            }
        }
        
        emitDiagnostic("warning", "WebSocket disconnected", mapOf(
            "code" to code,
            "reason" to (reason ?: "none"),
            "wasConnected" to wasConnected
        ))
        
        // Attempt reconnection if enabled
        if (autoReconnect && state != TransportState.STOPPING && state != TransportState.STOPPED) {
            mainHandler.post { scheduleReconnect() }
        } else {
            mainHandler.post { updateState(TransportState.STOPPED) }
        }
    }
    
    private fun handleConnectionFailure(t: Throwable) {
        isConnecting.set(false)
        
        emitDiagnostic("error", "WebSocket connection failed", mapOf(
            "error" to (t.message ?: "unknown"),
            "exception" to t.javaClass.simpleName
        ))
        
        handleConnectionClosed(-1, t.message)
    }
    
    private fun scheduleReconnect() {
        if (!autoReconnect) return
        
        val attempts = reconnectAttempts.incrementAndGet()
        if (maxReconnectAttempts > 0 && attempts > maxReconnectAttempts) {
            emitDiagnostic("error", "Max reconnect attempts reached", mapOf(
                "attempts" to attempts,
                "maxAttempts" to maxReconnectAttempts
            ))
            updateState(TransportState.STOPPED)
            return
        }
        
        val delay = currentReconnectDelay
        currentReconnectDelay = minOf(
            (currentReconnectDelay * RECONNECT_BACKOFF_MULTIPLIER).toLong(),
            RECONNECT_MAX_DELAY_MS
        )
        
        emitDiagnostic("info", "Scheduling reconnect", mapOf(
            "attempt" to attempts,
            "delayMs" to delay
        ))
        
        reconnectRunnable?.let { mainHandler.removeCallbacks(it) }
        val runnable = Runnable { connect() }
        reconnectRunnable = runnable
        mainHandler.postDelayed(runnable, delay)
    }
    
    // MARK: - WebSocket Listener
    
    private val webSocketListener = object : WebSocketListener() {
        override fun onOpen(webSocket: WebSocket, response: Response) {
            handleConnectionOpened()
        }
        
        override fun onMessage(webSocket: WebSocket, text: String) {
            processReceivedData(text.toByteArray(Charsets.UTF_8))
        }
        
        override fun onMessage(webSocket: WebSocket, bytes: okio.ByteString) {
            processReceivedData(bytes.toByteArray())
        }
        
        override fun onClosing(webSocket: WebSocket, code: Int, reason: String) {
            webSocket.close(1000, null)
        }
        
        override fun onClosed(webSocket: WebSocket, code: Int, reason: String) {
            handleConnectionClosed(code, reason)
        }
        
        override fun onFailure(webSocket: WebSocket, t: Throwable, response: Response?) {
            handleConnectionFailure(t)
        }
    }
    
    // MARK: - Message Handling
    
    private fun processReceivedData(data: ByteArray) {
        bytesReceived += data.size
        messagesReceived++
        
        // Try to extract sender ID from the message
        var senderId = "relay-server"
        try {
            val json = org.json.JSONObject(String(data, Charsets.UTF_8))
            if (json.has("sender")) {
                senderId = json.getString("sender")
            }
        } catch (e: Exception) {
            // Ignore JSON parsing errors
        }
        
        try {
            val bytes = data.map { it.toUByte() }
            protocol.internetMessageReceived(senderId, bytes)
            
            emitDiagnostic("debug", "Internet message received", mapOf(
                "senderId" to senderId,
                "size" to data.size
            ))
        } catch (e: Exception) {
            emitDiagnostic("error", "Error processing internet message", mapOf(
                "error" to (e.message ?: "unknown"),
                "exception" to e.javaClass.simpleName
            ))
        }
    }
    
    private fun startMessagePolling() {
        stopMessagePolling()
        mainHandler.post(messagePollingRunnable)
    }
    
    private fun stopMessagePolling() {
        mainHandler.removeCallbacks(messagePollingRunnable)
    }
    
    private fun pollAndSendMessages() {
        if (!isConnected.get()) return
        
        try {
            // Poll for next message from protocol
            val message = protocol.internetGetNextMessage()
            if (message != null) {
                sendMessage(message.recipientId, message.data.map { it.toByte() }.toByteArray())
            }
        } catch (e: Exception) {
            emitDiagnostic("error", "Error polling messages", mapOf(
                "error" to (e.message ?: "unknown")
            ))
        }
    }
    
    private fun sendMessage(recipientId: String, data: ByteArray) {
        val ws = webSocket
        if (!isConnected.get() || ws == null) {
            emitDiagnostic("warning", "Cannot send message - not connected")
            return
        }
        
        val sent = ws.send(okio.ByteString.of(*data))
        
        if (sent) {
            bytesSent += data.size
            messagesSent++
            
            emitDiagnostic("debug", "Message sent via WebSocket", mapOf(
                "recipientId" to recipientId,
                "size" to data.size
            ))
        } else {
            emitDiagnostic("error", "Failed to send WebSocket message", mapOf(
                "recipientId" to recipientId
            ))
        }
    }
    
    // MARK: - Ping
    
    private fun startPingTimer() {
        stopPingTimer()
        mainHandler.postDelayed(pingRunnable, PING_INTERVAL_MS)
    }
    
    private fun stopPingTimer() {
        mainHandler.removeCallbacks(pingRunnable)
    }
    
    private fun sendPing() {
        // OkHttp handles ping/pong automatically with pingInterval
        // This is just for manual pings if needed
    }
    
    // MARK: - State Management
    
    private fun updateState(newState: TransportState) {
        state = newState
        listener?.onTransportStateChanged(this, newState)
    }
    
    // MARK: - Diagnostics
    
    private fun emitDiagnostic(level: String, message: String, context: Map<String, Any?> = emptyMap()) {
        diagnosticEmitter?.invoke(level, message, context)
        listener?.onTransportDiagnostic(this, level, message, context)
    }
}

