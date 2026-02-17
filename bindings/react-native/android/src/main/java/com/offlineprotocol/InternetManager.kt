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
        
        // Polling is kept for Internet because there is no InternetTransportCallback in
        // the UDL. WebSocket sends are cheap (OkHttp buffers internally), and the initial
        // outbox flush is already event-driven (triggered immediately after authentication).
        // This timer serves as a fallback to pick up messages queued while connected.
        private const val MESSAGE_POLL_INTERVAL_MS = 100L
        private const val RECONNECT_INITIAL_DELAY_MS = 1000L
        private const val RECONNECT_MAX_DELAY_MS = 30000L
        private const val RECONNECT_BACKOFF_MULTIPLIER = 2.0
        private const val PING_INTERVAL_MS = 10000L  // Reduced from 30s for faster failure detection
        private const val CONNECTION_TIMEOUT_MS = 10000L
        private const val MAX_CONSECUTIVE_FAILURES = 2  // Trigger disconnect after 2 consecutive failures
    }
    
    // MARK: - Properties
    
    private var serverUrl: String? = null
    private var autoReconnect = true
    private var maxReconnectAttempts = 0 // 0 = infinite
    private var authToken: String? = null
    
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
    private val isAuthenticated = AtomicBoolean(false)
    private var reconnectAttempts = AtomicInteger(0)
    private var currentReconnectDelay = RECONNECT_INITIAL_DELAY_MS
    private var reconnectRunnable: Runnable? = null
    private var transportStartAt: Long = 0L
    
    // Failure tracking for DORS
    private var consecutiveSendFailures = AtomicInteger(0)
    
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
    
    /**
     * Set the auth token for authentication.
     * If the WebSocket is already connected, this will trigger re-authentication.
     */
    fun setAuthToken(token: String?) {
        val wasAuthenticated = isAuthenticated.get()
        this.authToken = token
        
        emitDiagnostic("info", "Auth token updated", mapOf(
            "hasToken" to (token != null),
            "wasAuthenticated" to wasAuthenticated
        ))
        
        // If already connected, (re-)authenticate with the latest token.
        // This ensures token rotations take effect immediately.
        if (isConnected.get()) {
            sendAuthentication()
        }
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
            "is_authenticated" to isAuthenticated.get(),
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
        isAuthenticated.set(false)
    }
    
    private fun handleConnectionOpened() {
        isConnected.set(true)
        isConnecting.set(false)
        isAuthenticated.set(false)
        reconnectAttempts.set(0)
        currentReconnectDelay = RECONNECT_INITIAL_DELAY_MS
        consecutiveSendFailures.set(0)
        
        emitDiagnostic("info", "WebSocket connected, authenticating...", mapOf(
            "serverUrl" to (serverUrl ?: "unknown")
        ))
        
        // Authenticate with the relay server using deviceId as the user ID
        sendAuthentication()
    }
    
    private fun sendAuthentication() {
        val ws = webSocket ?: return
        
        // Use auth token if available, otherwise fall back to deviceId
        val token = authToken ?: deviceId
        val authMessage = org.json.JSONObject().apply {
            put("type", "Authenticate")
            put("token", token)
        }
        
        val sent = ws.send(authMessage.toString())
        if (sent) {
            emitDiagnostic("debug", "Auth message sent", mapOf(
                "userId" to deviceId
            ))
        } else {
            emitDiagnostic("error", "Failed to send auth message")
        }
    }
    
    private fun handleAuthenticated(userId: String, username: String) {
        isAuthenticated.set(true)
        
        mainHandler.post {
            updateState(TransportState.RUNNING)
        }
        
        // Notify protocol - this will trigger outbox flush for pending messages
        try {
            protocol.internetStatusChanged(true)
        } catch (e: Exception) {
            Log.e(TAG, "Error notifying protocol of connect", e)
        }
        
        // Start polling and pinging
        mainHandler.post {
            startMessagePolling()
            startPingTimer()
            
            // Immediately poll for messages to flush outbox after reconnection
            // This ensures messages queued during disconnection are sent promptly
            pollAndSendMessages()
        }
        
        emitDiagnostic("info", "Authenticated with relay server", mapOf(
            "userId" to userId,
            "username" to username
        ))
    }
    
    private fun handleConnectionClosed(code: Int, reason: String?) {
        val wasConnected = isConnected.getAndSet(false)
        val wasAuthenticated = isAuthenticated.getAndSet(false)
        isConnecting.set(false)
        
        // Stop polling and pinging immediately to prevent sending on dead connection
        mainHandler.post {
            stopMessagePolling()
            stopPingTimer()
        }
        
        // Always notify protocol of disconnection
        // This ensures the protocol knows the transport is unavailable
        // even if we weren't fully authenticated
        if (wasConnected || wasAuthenticated) {
            // Notify protocol of disconnection - this keeps outbox messages pending
            try {
                protocol.internetStatusChanged(false)
            } catch (e: Exception) {
                Log.e(TAG, "Error notifying protocol of disconnect", e)
                emitDiagnostic("error", "Failed to notify protocol of disconnection", mapOf(
                    "error" to (e.message ?: "unknown")
                ))
            }
        }
        
        emitDiagnostic("warning", "WebSocket disconnected", mapOf(
            "code" to code,
            "reason" to (reason ?: "none"),
            "wasConnected" to wasConnected,
            "wasAuthenticated" to wasAuthenticated
        ))
        
        // Attempt reconnection if enabled
        // Messages in outbox will be flushed on successful reconnection
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
        
        val json: org.json.JSONObject
        val messageType: String
        
        try {
            json = org.json.JSONObject(String(data, Charsets.UTF_8))
            messageType = json.optString("type", "")
        } catch (e: Exception) {
            emitDiagnostic("warning", "Received non-JSON or invalid message", mapOf(
                "size" to data.size
            ))
            return
        }
        
        when (messageType) {
            "Authenticated" -> {
                // Handle authentication success
                val userId = json.optString("user_id", deviceId)
                val username = json.optString("username", deviceId)
                handleAuthenticated(userId, username)
            }
            
            "AuthError" -> {
                // Handle authentication error
                val reason = json.optString("reason", "Unknown error")
                emitDiagnostic("error", "Authentication failed", mapOf(
                    "reason" to reason
                ))
                handleConnectionClosed(-1, reason)
            }
            
            "MessageSent" -> {
                // Handle MessageSent event from WebSocket server
                // This contains the server-generated message_id that we should use
                val messageId = json.optString("message_id", null)
                val recipient = json.optString("recipient", "")
                val timestamp = json.optString("timestamp", "")
                
                if (messageId != null && messageId.isNotEmpty()) {
                    // The server has confirmed the message was sent with this message_id
                    // We need to notify the protocol so it can update the message ID
                    // The protocol will emit a message_sent event with this server-generated ID
                    emitDiagnostic("debug", "MessageSent from relay server", mapOf(
                        "messageId" to messageId,
                        "recipient" to recipient,
                        "timestamp" to timestamp
                    ))
                    // Note: The protocol SDK will handle the message_sent event internally
                    // The frontend will receive it via the normal event stream
                }
            }
            
            "MessageReceived" -> {
                // Handle incoming direct message
                val senderId = json.optString("sender", "")
                val content = json.optString("content", "")
                val replyToMsg = json.optString("reply_to_msg", null)
                val messageId = json.optString("message_id", null)
                val timestamp = json.optString("timestamp", "")
                
                if (senderId.isEmpty()) {
                    emitDiagnostic("warning", "Invalid MessageReceived format")
                    return
                }
                
                messagesReceived++
                
                try {
                    // The protocol expects the full serialized Message JSON bytes
                    // The WebSocket server sends the message content, which should be the full Message JSON
                    // (since that's what we sent). However, the server also extracts reply_to_msg and message_id as
                    // separate fields, so we need to ensure they're included in the Message JSON.
                    var messageBytes: ByteArray
                    
                    try {
                        // Try to parse content as JSON to see if it's already a full Message
                        val contentJson = org.json.JSONObject(content)
                        if (contentJson.has("sender") && contentJson.has("recipient")) {
                            // It's already a full Message JSON
                            // Ensure message_id is included if present in WebSocket event
                            if (messageId != null && messageId.isNotEmpty() && !contentJson.has("id") && !contentJson.has("message_id")) {
                                contentJson.put("id", messageId)
                            }
                            // Ensure reply_to_msg is included if present in WebSocket event but missing from content
                            if (replyToMsg != null && replyToMsg.isNotEmpty() && !contentJson.has("reply_to_msg")) {
                                contentJson.put("reply_to_msg", replyToMsg)
                            }
                            messageBytes = contentJson.toString().toByteArray(Charsets.UTF_8)
                        } else {
                            // Content is just the message text, reconstruct full Message JSON
                            // This shouldn't happen if server forwards the full Message, but handle it anyway
                            val messageJson = org.json.JSONObject().apply {
                                if (messageId != null && messageId.isNotEmpty()) {
                                    put("id", messageId)
                                }
                                put("sender", senderId)
                                put("recipient", deviceId) // Will be corrected by protocol
                                put("content", content)
                                put("app_id", "offline-messenger") // Default app ID
                                put("priority", "Medium")
                                put("ttl", 8)
                                put("hop_count", 0)
                                put("requires_ack", true)
                                if (replyToMsg != null && replyToMsg.isNotEmpty()) {
                                    put("reply_to_msg", replyToMsg)
                                }
                            }
                            messageBytes = messageJson.toString().toByteArray(Charsets.UTF_8)
                        }
                    } catch (e: org.json.JSONException) {
                        // Content is not JSON (plain text), reconstruct full Message JSON
                        val messageJson = org.json.JSONObject().apply {
                            if (messageId != null && messageId.isNotEmpty()) {
                                put("id", messageId)
                            }
                            put("sender", senderId)
                            put("recipient", deviceId) // Will be corrected by protocol
                            put("content", content)
                            put("app_id", "offline-messenger") // Default app ID
                            put("priority", "Medium")
                            put("ttl", 8)
                            put("hop_count", 0)
                            put("requires_ack", true)
                            if (replyToMsg != null && replyToMsg.isNotEmpty()) {
                                put("reply_to_msg", replyToMsg)
                            }
                        }
                        messageBytes = messageJson.toString().toByteArray(Charsets.UTF_8)
                    }
                    
                    val bytes = messageBytes.map { it.toUByte() }
                    protocol.internetMessageReceived(senderId, bytes)
                    
                    emitDiagnostic("debug", "Message received from relay", mapOf(
                        "senderId" to senderId,
                        "messageId" to (messageId ?: "none"),
                        "contentLength" to content.length,
                        "hasReplyToMsg" to (replyToMsg != null && replyToMsg.isNotEmpty())
                    ))
                } catch (e: Exception) {
                    emitDiagnostic("error", "Error processing relay message", mapOf(
                        "error" to (e.message ?: "unknown")
                    ))
                }
            }
            
            "DeliveryError" -> {
                // Handle delivery error
                val recipient = json.optString("recipient", "unknown")
                val reason = json.optString("reason", "Unknown error")
                emitDiagnostic("warning", "Message delivery failed", mapOf(
                    "recipient" to recipient,
                    "reason" to reason
                ))
            }
            
            "PresenceStatus", "PresenceStatusWithLastSeen" -> {
                // Handle presence updates (optional logging)
                val userId = json.optString("user_id", "unknown")
                val online = json.optBoolean("online", false)
                emitDiagnostic("debug", "Presence update", mapOf(
                    "userId" to userId,
                    "online" to online
                ))
            }
            
            "ConnectionRequestReceived" -> {
                // Forward connection request to JavaScript with full data so it emits connection_request_received
                val senderId = json.optString("sender", "")
                val senderName = json.optString("sender_name", senderId)
                val timestampStr = json.optString("timestamp", "")
                val keyPackage = if (json.has("key_package")) {
                    try {
                        val keyPackageArray = json.getJSONArray("key_package")
                        (0 until keyPackageArray.length()).map { keyPackageArray.getInt(it).toByte() }
                    } catch (e: Exception) { null }
                } else null
                if (senderId.isEmpty()) {
                    emitDiagnostic("warning", "Invalid ConnectionRequestReceived format: missing sender")
                    return
                }
                val timestampMs = parseTimestampToMs(timestampStr)
                val payloadJson = org.json.JSONObject().apply {
                    put("sender_name", senderName)
                    put("timestamp_ms", timestampMs)
                    if (keyPackage != null) {
                        put("key_package", org.json.JSONArray(keyPackage.map { it.toInt().and(0xFF) }))
                    }
                }
                val content = "__CONN_REQ__" + payloadJson.toString()
                try {
                    val messageBytes = buildInternalMessageBytes(senderId, content)
                    protocol.internetMessageReceived(senderId, messageBytes.map { it.toUByte() })
                    emitDiagnostic("debug", "Connection request received from relay", mapOf(
                        "sender" to senderId,
                        "sender_name" to senderName
                    ))
                } catch (e: Exception) {
                    emitDiagnostic("error", "Error processing ConnectionRequestReceived", mapOf(
                        "error" to (e.message ?: "unknown")
                    ))
                }
            }
            
            "ConnectionAccepted" -> {
                val acceptedBy = json.optString("accepted_by", json.optString("sender", ""))
                val acceptedByName = json.optString("accepted_by_name", json.optString("sender_name", acceptedBy))
                val timestampStr = json.optString("timestamp", "")
                val keyPackage = if (json.has("key_package")) {
                    try {
                        val keyPackageArray = json.getJSONArray("key_package")
                        (0 until keyPackageArray.length()).map { keyPackageArray.getInt(it).toByte() }
                    } catch (e: Exception) { null }
                } else null
                if (acceptedBy.isEmpty()) {
                    emitDiagnostic("warning", "Invalid ConnectionAccepted format: missing accepted_by")
                    return
                }
                val timestampMs = parseTimestampToMs(timestampStr)
                val payloadJson = org.json.JSONObject().apply {
                    put("accepted_by_name", acceptedByName)
                    put("timestamp_ms", timestampMs)
                    if (keyPackage != null) {
                        put("key_package", org.json.JSONArray(keyPackage.map { it.toInt().and(0xFF) }))
                    }
                }
                val content = "__CONN_ACC__" + payloadJson.toString()
                try {
                    val messageBytes = buildInternalMessageBytes(acceptedBy, content)
                    protocol.internetMessageReceived(acceptedBy, messageBytes.map { it.toUByte() })
                    emitDiagnostic("debug", "Connection accepted from relay", mapOf(
                        "accepted_by" to acceptedBy,
                        "accepted_by_name" to acceptedByName
                    ))
                } catch (e: Exception) {
                    emitDiagnostic("error", "Error processing ConnectionAccepted", mapOf(
                        "error" to (e.message ?: "unknown")
                    ))
                }
            }
            
            "ConnectionRejected" -> {
                val rejectedBy = json.optString("rejected_by", json.optString("sender", ""))
                if (rejectedBy.isEmpty()) {
                    emitDiagnostic("warning", "Invalid ConnectionRejected format: missing rejected_by")
                    return
                }
                val content = "__CONN_REJ__"
                try {
                    val messageBytes = buildInternalMessageBytes(rejectedBy, content)
                    protocol.internetMessageReceived(rejectedBy, messageBytes.map { it.toUByte() })
                    emitDiagnostic("debug", "Connection rejected from relay", mapOf(
                        "rejected_by" to rejectedBy
                    ))
                } catch (e: Exception) {
                    emitDiagnostic("error", "Error processing ConnectionRejected", mapOf(
                        "error" to (e.message ?: "unknown")
                    ))
                }
            }
            
            "ConnectionRequestError" -> {
                val recipient = json.optString("recipient", "")
                val reason = json.optString("reason", "Unknown error")
                emitDiagnostic("debug", "Received relay message", mapOf(
                    "type" to messageType,
                    "recipient" to recipient,
                    "reason" to reason
                ))
            }
            
            "GroupCreated" -> {
                val groupId = json.optString("group_id", "")
                val name = json.optString("name", "")
                if (groupId.isEmpty()) return
                val payloadJson = org.json.JSONObject().apply {
                    put("group_id", groupId)
                    put("name", name)
                }
                injectGroupInternalMessage("relay", "__GROUP_CREATED__", payloadJson)
            }
            
            "GroupMessageReceived" -> {
                val groupId = json.optString("group_id", "")
                val sender = json.optString("sender", "")
                val content = json.optString("content", "")
                val timestamp = json.optString("timestamp", "")
                val messageId = json.optString("message_id", "")
                val replyToMsg = json.optString("reply_to_msg", null)
                if (groupId.isEmpty() || messageId.isEmpty()) return
                val payloadJson = org.json.JSONObject().apply {
                    put("group_id", groupId)
                    put("sender", sender)
                    put("content", content)
                    put("timestamp", timestamp)
                    put("message_id", messageId)
                    if (replyToMsg != null && replyToMsg.isNotEmpty()) put("reply_to_msg", replyToMsg)
                }
                injectGroupInternalMessage(if (sender.isNotEmpty()) sender else "relay", "__GROUP_MSG__", payloadJson)
            }
            
            "GroupMemberAdded" -> {
                val groupId = json.optString("group_id", "")
                val userId = json.optString("user_id", "")
                val addedBy = json.optString("added_by", "")
                if (groupId.isEmpty()) return
                val payloadJson = org.json.JSONObject().apply {
                    put("group_id", groupId)
                    put("user_id", userId)
                    put("added_by", addedBy)
                }
                injectGroupInternalMessage(if (addedBy.isNotEmpty()) addedBy else "relay", "__GROUP_MEMBER_ADDED__", payloadJson)
            }
            
            "GroupMemberRemoved" -> {
                val groupId = json.optString("group_id", "")
                val userId = json.optString("user_id", "")
                val removedBy = json.optString("removed_by", "")
                if (groupId.isEmpty()) return
                val payloadJson = org.json.JSONObject().apply {
                    put("group_id", groupId)
                    put("user_id", userId)
                    put("removed_by", removedBy)
                }
                injectGroupInternalMessage(if (removedBy.isNotEmpty()) removedBy else "relay", "__GROUP_MEMBER_REMOVED__", payloadJson)
            }
            
            "GroupInfo" -> {
                val groupId = json.optString("group_id", "")
                val name = json.optString("name", "")
                val createdBy = json.optString("created_by", "")
                val createdAt = json.optString("created_at", "")
                val membersArray = json.optJSONArray("members")
                if (groupId.isEmpty()) return
                val membersJson = org.json.JSONArray()
                if (membersArray != null) {
                    for (i in 0 until membersArray.length()) {
                        val m = membersArray.getJSONObject(i)
                        membersJson.put(org.json.JSONObject().apply {
                            put("user_id", m.optString("user_id", ""))
                            put("role", m.optString("role", "member"))
                            put("joined_at", m.optString("joined_at", ""))
                        })
                    }
                }
                val payloadJson = org.json.JSONObject().apply {
                    put("group_id", groupId)
                    put("name", name)
                    put("created_by", createdBy)
                    put("created_at", createdAt)
                    put("members", membersJson)
                }
                injectGroupInternalMessage("relay", "__GROUP_INFO__", payloadJson)
            }
            
            "UserGroups" -> {
                val groupsArray = json.optJSONArray("groups")
                if (groupsArray == null) return
                val groupsJson = org.json.JSONArray()
                for (i in 0 until groupsArray.length()) {
                    val g = groupsArray.getJSONObject(i)
                    groupsJson.put(org.json.JSONObject().apply {
                        put("group_id", g.optString("group_id", ""))
                        put("name", g.optString("name", ""))
                        put("created_at", g.optString("created_at", ""))
                    })
                }
                val payloadJson = org.json.JSONObject().apply { put("groups", groupsJson) }
                injectGroupInternalMessage("relay", "__USER_GROUPS__", payloadJson)
            }
            
            "GroupError" -> {
                val reason = json.optString("reason", "Unknown error")
                val payloadJson = org.json.JSONObject().apply { put("reason", reason) }
                injectGroupInternalMessage("relay", "__GROUP_ERROR__", payloadJson)
            }
            
            else -> {
                emitDiagnostic("debug", "Received relay message", mapOf(
                    "type" to messageType
                ))
            }
        }
    }
    
    /** Parse ISO-8601 timestamp string to Unix ms, or return current time if invalid. */
    private fun parseTimestampToMs(timestampStr: String): Long {
        if (timestampStr.isEmpty()) return System.currentTimeMillis()
        return try {
            java.time.Instant.parse(timestampStr).toEpochMilli()
        } catch (e: Exception) {
            System.currentTimeMillis()
        }
    }
    
    /** Build serialized Message JSON bytes for an internal (relay) message, same shape as MessageReceived. */
    private fun buildInternalMessageBytes(senderId: String, content: String): ByteArray {
        val messageJson = org.json.JSONObject().apply {
            put("id", java.util.UUID.randomUUID().toString())
            put("sender", senderId)
            put("recipient", deviceId)
            put("content", content)
            put("app_id", "offline-messenger")
            put("priority", "Medium")
            put("ttl", 8)
            put("hop_count", 0)
            put("requires_ack", true)
            put("timestamp", System.currentTimeMillis())
        }
        return messageJson.toString().toByteArray(Charsets.UTF_8)
    }
    
    /** Inject a group (relay) internal message into the protocol so it emits the corresponding event. */
    private fun injectGroupInternalMessage(senderId: String, prefix: String, payloadJson: org.json.JSONObject) {
        try {
            val content = prefix + payloadJson.toString()
            val messageBytes = buildInternalMessageBytes(senderId, content)
            protocol.internetMessageReceived(senderId, messageBytes.map { it.toUByte() })
        } catch (e: Exception) {
            emitDiagnostic("error", "Error injecting group message", mapOf(
                "prefix" to prefix,
                "error" to (e.message ?: "unknown")
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
        // Double-check connection state to handle race conditions
        // This prevents sending messages right after transport disconnect
        if (!isConnected.get() || !isAuthenticated.get()) return
        
        try {
            // Poll for next message from protocol - batch send up to 10 messages per poll
            // to efficiently flush the outbox after reconnection
            var messagesSent = 0
            val maxBatchSize = 10
            
            while (messagesSent < maxBatchSize) {
                // Re-check connection state between messages to handle mid-batch disconnects
                if (!isConnected.get() || !isAuthenticated.get()) {
                    emitDiagnostic("warning", "Connection lost mid-batch, stopping message send", mapOf(
                        "messagesSent" to messagesSent
                    ))
                    break
                }
                
                val message = protocol.internetGetNextMessage() ?: break
                sendMessage(message.messageId, message.recipientId, message.data.map { it.toByte() }.toByteArray(), message.replyToMsg)
                messagesSent++
            }
            
            if (messagesSent > 1) {
                emitDiagnostic("debug", "Batch sent messages", mapOf(
                    "count" to messagesSent
                ))
            }
        } catch (e: Exception) {
            emitDiagnostic("error", "Error polling messages", mapOf(
                "error" to (e.message ?: "unknown")
            ))
        }
    }
    
    private fun sendMessage(messageId: String, recipientId: String, data: ByteArray, replyToMsg: String? = null) {
        val ws = webSocket
        // Re-check connection state right before sending
        // This handles race conditions where connection drops between poll and send
        if (!isConnected.get() || !isAuthenticated.get() || ws == null) {
            emitDiagnostic("warning", "Cannot send message - not connected or not authenticated", mapOf(
                "messageId" to messageId,
                "recipientId" to recipientId,
                "isConnected" to isConnected.get(),
                "isAuthenticated" to isAuthenticated.get(),
                "hasSocket" to (ws != null)
            ))
            // Report failure so DORS metrics stay accurate
            try { protocol.internetSendFailed(messageId) } catch (e: Exception) { Log.e(TAG, "Failed to report send failure for $messageId", e) }
            return
        }
        
        // Convert data to string content for the relay protocol
        val content = String(data, Charsets.UTF_8)
        
        // Wrap in relay server protocol format
        // reply_to_msg is now provided directly from the Rust SDK via InternetMessage
        val relayMessage = org.json.JSONObject().apply {
            put("type", "SendMessage")
            put("recipient", recipientId)
            put("content", content)
            if (replyToMsg != null && replyToMsg.isNotEmpty()) {
                put("reply_to_msg", replyToMsg)
            }
        }
        
        val jsonString = relayMessage.toString()
        val sent = ws.send(jsonString)
        
        if (sent) {
            // Reset failure counter on successful send
            consecutiveSendFailures.set(0)
            bytesSent += jsonString.length
            messagesSent++
            try { protocol.internetConfirmSent(messageId) } catch (e: Exception) { Log.e(TAG, "Failed to confirm send for $messageId", e) }
            
            emitDiagnostic("debug", "Message sent via relay", mapOf(
                "messageId" to messageId,
                "recipientId" to recipientId,
                "contentLength" to content.length
            ))
        } else {
            val failures = consecutiveSendFailures.incrementAndGet()
            try { protocol.internetSendFailed(messageId) } catch (e: Exception) { Log.e(TAG, "Failed to report send failure for $messageId", e) }
            emitDiagnostic("error", "Failed to send WebSocket message", mapOf(
                "messageId" to messageId,
                "recipientId" to recipientId,
                "consecutiveFailures" to failures
            ))
            
            // If too many consecutive send failures, the connection is likely dead
            // Trigger disconnect so DORS can switch to another transport
            if (failures >= MAX_CONSECUTIVE_FAILURES) {
                emitDiagnostic("warning", "Too many consecutive send failures, triggering reconnect for DORS", mapOf(
                    "failures" to failures
                ))
                mainHandler.post { handleConnectionClosed(-1, "Send failures exceeded threshold") }
            }
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

