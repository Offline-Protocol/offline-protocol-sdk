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
    // Written on main (connect/disconnect) and on the OkHttp reader thread
    // (AuthError teardown); read from RN bridge threads (sendRawCommand,
    // checkPresence). Volatile so those reads don't depend on an incidental
    // happens-before chain through the connection-state atomics.
    @Volatile private var webSocket: WebSocket? = null
    
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

    // Presence watch runnable: periodically asks the relay about peers with
    // undelivered traffic (CheckPresence), so parked welcomes re-arm the
    // moment a peer comes back — the relay never stores content, so presence
    // is the only authoritative recovery signal for offline recipients.
    private val presenceWatchRunnable = object : Runnable {
        override fun run() {
            presenceWatchTick()
            if (state == TransportState.RUNNING && isConnected.get()) {
                mainHandler.postDelayed(this, PresenceWatchPolicy.DEFAULT_TICK_INTERVAL_MS)
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

    // Correlates the relay's recipient-keyed failure signals (DeliveryError /
    // ConnectionRequestError carry no message_id) back to in-flight sends.
    private val inFlightTracker = RecipientInFlightTracker()

    // Which peers to query via CheckPresence, and how many per tick.
    private val presenceWatch = PresenceWatchPolicy()

    // Translates core-tagged server-plane control frames (control_op on
    // InternetMessage) into relay-native ops.
    private val controlOpTranslator = RelayControlOpTranslator(deviceId)

    /**
     * Receives raw relay frames that are app/server concerns rather than SDK
     * concerns (invite links, role changes, rate limiting, unknown types) —
     * the module forwards them to JS as the `internet_server_message` event.
     */
    var serverMessageEmitter: ((String) -> Unit)? = null
    
    // Metrics. Atomic: send paths mutate on main, receive paths on the
    // OkHttp reader thread, and getMetrics() reads from the caller's thread.
    private val bytesSent = java.util.concurrent.atomic.AtomicLong(0)
    private val bytesReceived = java.util.concurrent.atomic.AtomicLong(0)
    private val messagesSent = java.util.concurrent.atomic.AtomicLong(0)
    private val messagesReceived = java.util.concurrent.atomic.AtomicLong(0)
    
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
        stopPresenceWatch()

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
            // A backgrounded app must not keep spending battery and relay
            // rate-limit budget on CheckPresence ticks; parked welcomes
            // re-arm from the watch loop after resume().
            stopPresenceWatch()
        }
    }

    override fun resume() {
        runOnMainSync {
            if (state == TransportState.RUNNING && isConnected.get()) {
                startMessagePolling()
                startPingTimer()
                startPresenceWatch()
            }
        }
    }
    
    override fun getMetrics(): Map<String, Any> {
        return mapOf(
            "bytes_sent" to bytesSent.get(),
            "bytes_received" to bytesReceived.get(),
            "messages_sent" to messagesSent.get(),
            "messages_received" to messagesReceived.get(),
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
        
        // Start polling, pinging, and the presence watch
        mainHandler.post {
            startMessagePolling()
            startPingTimer()
            startPresenceWatch()

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
            stopPresenceWatch()
            // Wire outcomes for anything in flight are now owned by the
            // transport layer (fail_all_pending on disconnect).
            inFlightTracker.clear()
            // Registration diffs are per-connection: a reconnect re-registers
            // groups from scratch (sync_groups_to_relay re-sends on the
            // internet 0→1 transition).
            controlOpTranslator.reset()
        }
        
        // Always notify protocol of disconnection so DORS excludes Internet from
        // available transports and can switch to BLE (or WiFi Direct).
        try {
            protocol.internetStatusChanged(false)
        } catch (e: Exception) {
            Log.e(TAG, "Error notifying protocol of disconnect", e)
            emitDiagnostic("error", "Failed to notify protocol of disconnection", mapOf(
                "error" to (e.message ?: "unknown")
            ))
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
        /**
         * True when the callback belongs to a socket that is no longer the
         * manager's current one (replaced by a reconnect or detached by a
         * teardown). A stale socket's terminal callbacks must not clear the
         * in-flight tracker, reset the translator, or report the transport
         * down while a newer, healthy connection is live.
         */
        private fun isStale(ws: WebSocket): Boolean = ws !== webSocket

        override fun onOpen(webSocket: WebSocket, response: Response) {
            if (isStale(webSocket)) return
            handleConnectionOpened()
        }

        override fun onMessage(webSocket: WebSocket, text: String) {
            if (isStale(webSocket)) return
            processReceivedData(text.toByteArray(Charsets.UTF_8))
        }

        override fun onMessage(webSocket: WebSocket, bytes: okio.ByteString) {
            if (isStale(webSocket)) return
            processReceivedData(bytes.toByteArray())
        }

        override fun onClosing(webSocket: WebSocket, code: Int, reason: String) {
            webSocket.close(1000, null)
        }

        override fun onClosed(webSocket: WebSocket, code: Int, reason: String) {
            if (isStale(webSocket)) return
            handleConnectionClosed(code, reason)
        }

        override fun onFailure(webSocket: WebSocket, t: Throwable, response: Response?) {
            if (isStale(webSocket)) return
            handleConnectionFailure(t)
        }
    }

    /**
     * Cancels and detaches the current socket, then runs the closed handler
     * exactly once for it. Detaching before cancel makes the cancel-triggered
     * onFailure/onClosed no-ops (the listener ignores stale sockets), so a
     * dead socket can never tear down the connection rebuilt after it.
     */
    private fun teardownCurrentSocket(reason: String) {
        val socket = webSocket
        webSocket = null
        socket?.cancel()
        handleConnectionClosed(-1, reason)
    }
    
    // MARK: - Message Handling
    
    private fun processReceivedData(data: ByteArray) {
        bytesReceived.addAndGet(data.size.toLong())
        
        val json: org.json.JSONObject
        val messageType: String
        
        try {
            json = org.json.JSONObject(String(data, Charsets.UTF_8))
            messageType = json.safeOptString("type")
        } catch (e: Exception) {
            emitDiagnostic("warning", "Received non-JSON or invalid message", mapOf(
                "size" to data.size
            ))
            return
        }
        
        when (messageType) {
            "Authenticated" -> {
                // Handle authentication success
                val userId = json.safeOptString("user_id", deviceId)
                val username = json.safeOptString("username", deviceId)
                handleAuthenticated(userId, username)
            }
            
            "AuthError" -> {
                // Handle authentication error
                val reason = json.safeOptString("reason", "Unknown error")
                emitDiagnostic("error", "Authentication failed", mapOf(
                    "reason" to reason
                ))
                // The auth-failed socket must actually be closed — left open,
                // its eventual onClosed would race the reconnect's fresh
                // connection (the teardown detaches it first, so the
                // cancel-triggered callbacks are ignored as stale).
                teardownCurrentSocket(reason)
            }
            
            "MessageSent" -> {
                // Handle MessageSent event from WebSocket server
                // This contains the server-generated message_id that we should use
                val messageId = json.optNullableString("message_id")
                val recipient = json.safeOptString("recipient")
                val timestamp = json.safeOptString("timestamp")

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
                val senderId = json.safeOptString("sender")
                val content = json.safeOptString("content")
                val replyToMsg = json.optNullableString("reply_to_msg")
                val messageId = json.optNullableString("message_id")
                val timestamp = json.safeOptString("timestamp")
                
                if (senderId.isEmpty()) {
                    emitDiagnostic("warning", "Invalid MessageReceived format")
                    return
                }

                messagesReceived.incrementAndGet()
                
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

                    // Inbound traffic proves the peer reachable — stop
                    // presence-polling them (core re-arms via the
                    // internetMessageReceived → reachability path).
                    presenceWatch.unwatch(senderId)

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
                // The relay's authoritative "recipient offline" signal. It
                // arrives well before the SDK's confirm timeout, so fail-fast
                // every in-flight message to this recipient with the
                // recipient_unreachable reason (parks welcomes instead of
                // burning their retry budget) and start watching presence.
                val recipient = json.safeOptString("recipient")
                val reason = json.safeOptString("reason", "Unknown error")
                handleRecipientUnreachable(recipient, reason, "DeliveryError")
            }

            "PresenceStatus", "PresenceStatusWithLastSeen" -> {
                // Relay presence answer: feed core (re-arms parked welcomes
                // and flushes queues on online; parks pending welcomes on
                // offline) and maintain the watch set.
                val userId = json.safeOptString("user_id")
                val online = json.opt("online") as? Boolean
                if (userId.isEmpty() || online == null) {
                    emitDiagnostic("warning", "Invalid presence format: missing user_id/online")
                    return
                }
                // Type-branched: Android's org.json coerces numbers via
                // getString but the JVM org.json (unit tests) throws, and the
                // relay may send epoch numbers directly.
                val lastSeenMs = when (val rawLastSeen = json.opt("last_seen")) {
                    is Number -> rawLastSeen.toLong()
                    is String -> RelayTimestamps.parseToMsOrNull(rawLastSeen)
                    else -> null
                }
                if (online) {
                    presenceWatch.unwatch(userId)
                }
                try {
                    protocol.internetPeerPresence(userId, online, lastSeenMs)
                } catch (e: Exception) {
                    Log.e(TAG, "Failed to ingest presence for $userId", e)
                }
                emitDiagnostic("debug", "Presence update", mapOf(
                    "userId" to userId,
                    "online" to online,
                    "lastSeenMs" to (lastSeenMs ?: "none")
                ))
            }

            "TypingUpdate" -> {
                // Bridge the relay's server-mediated typing event (produced by
                // SetTyping/ClearTyping relay clients) into the SDK's __TYPING__
                // path, so apps receive the same typing_indicator_received event
                // regardless of which stack the sender uses.
                val typingUserId = json.safeOptString("user_id")
                val conversationId = json.safeOptString("conversation_id")
                // Strict "typing" check: if the relay renames or drops the
                // field, that must surface as a diagnostic instead of silently
                // degrading every event to typing=false.
                val typing = json.opt("typing") as? Boolean
                if (typingUserId.isEmpty() || conversationId.isEmpty() || typing == null) {
                    emitDiagnostic("warning", "Invalid TypingUpdate format: missing user_id/conversation_id/typing")
                    return
                }
                val typingPayload = org.json.JSONObject().apply {
                    put("conversation_id", conversationId)
                    put("is_typing", typing)
                    put("timestamp_ms", System.currentTimeMillis())
                }
                injectGroupInternalMessage(typingUserId, "__TYPING__", typingPayload)
                emitDiagnostic("debug", "Typing update bridged from relay", mapOf(
                    "userId" to typingUserId,
                    "typing" to typing
                ))
            }

            "ConnectionRequestReceived" -> {
                // Forward connection request to JavaScript with full data so it emits connection_request_received
                val senderId = json.safeOptString("sender")
                val senderName = json.safeOptString("sender_name", senderId)
                val timestampStr = json.safeOptString("timestamp")
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
                val sender = json.safeOptString("sender")
                val acceptedBy = json.safeOptString("accepted_by", sender)
                val acceptedByName = json.safeOptString("accepted_by_name", json.safeOptString("sender_name", acceptedBy))
                val timestampStr = json.safeOptString("timestamp")
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
                val rejectedBy = json.safeOptString("rejected_by", json.safeOptString("sender"))
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
                // Same authoritative offline signal as DeliveryError, for
                // relay-native connection-request ops (the relay does not
                // store requests for offline recipients).
                val recipient = json.safeOptString("recipient")
                val reason = json.safeOptString("reason", "Unknown error")
                handleRecipientUnreachable(recipient, reason, "ConnectionRequestError")
            }
            
            "GroupCreated" -> {
                val groupId = json.safeOptString("group_id")
                val name = json.safeOptString("name")
                if (groupId.isEmpty()) return
                val payloadJson = org.json.JSONObject().apply {
                    put("group_id", groupId)
                    put("name", name)
                }
                injectGroupInternalMessage("relay", "__GROUP_CREATED__", payloadJson)
            }
            
            "GroupMessageReceived" -> {
                val groupId = json.safeOptString("group_id")
                val sender = json.safeOptString("sender")
                val content = json.safeOptString("content")
                val timestamp = json.safeOptString("timestamp")
                val messageId = json.safeOptString("message_id")
                val replyToMsg = json.optNullableString("reply_to_msg")
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
                val groupId = json.safeOptString("group_id")
                val userId = json.safeOptString("user_id")
                val addedBy = json.safeOptString("added_by")
                if (groupId.isEmpty()) return
                val payloadJson = org.json.JSONObject().apply {
                    put("group_id", groupId)
                    put("user_id", userId)
                    put("added_by", addedBy)
                }
                injectGroupInternalMessage(if (addedBy.isNotEmpty()) addedBy else "relay", "__GROUP_MEMBER_ADDED__", payloadJson)
            }
            
            "GroupMemberRemoved" -> {
                val groupId = json.safeOptString("group_id")
                val userId = json.safeOptString("user_id")
                val removedBy = json.safeOptString("removed_by")
                if (groupId.isEmpty()) return
                val payloadJson = org.json.JSONObject().apply {
                    put("group_id", groupId)
                    put("user_id", userId)
                    put("removed_by", removedBy)
                }
                injectGroupInternalMessage(if (removedBy.isNotEmpty()) removedBy else "relay", "__GROUP_MEMBER_REMOVED__", payloadJson)
            }
            
            "GroupInfo" -> {
                val groupId = json.safeOptString("group_id")
                val name = json.safeOptString("name")
                val createdBy = json.safeOptString("created_by")
                val createdAt = json.safeOptString("created_at")
                val membersArray = json.optJSONArray("members")
                if (groupId.isEmpty()) return
                val membersJson = org.json.JSONArray()
                if (membersArray != null) {
                    for (i in 0 until membersArray.length()) {
                        val m = membersArray.getJSONObject(i)
                        val memberId = m.safeOptString("user_id")
                        if (memberId.isEmpty()) continue
                        membersJson.put(org.json.JSONObject().apply {
                            put("user_id", memberId)
                            put("role", m.safeOptString("role", "member"))
                            put("joined_at", m.safeOptString("joined_at"))
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
                    val gId = g.safeOptString("group_id")
                    if (gId.isEmpty()) continue
                    groupsJson.put(org.json.JSONObject().apply {
                        put("group_id", gId)
                        put("name", g.safeOptString("name"))
                        put("created_at", g.safeOptString("created_at"))
                    })
                }
                val payloadJson = org.json.JSONObject().apply { put("groups", groupsJson) }
                injectGroupInternalMessage("relay", "__USER_GROUPS__", payloadJson)
            }
            
            "GroupError" -> {
                val reason = json.safeOptString("reason", "Unknown error")
                val groupId = json.safeOptString("group_id")
                // Admin-denied registration must stop member-delta attempts.
                controlOpTranslator.onGroupError(groupId, reason)
                val payloadJson = org.json.JSONObject().apply {
                    put("reason", reason)
                    // group_id lets the core revoke relay_synced so group
                    // sends fall back to per-member delivery.
                    if (groupId.isNotEmpty()) put("group_id", groupId)
                }
                injectGroupInternalMessage("relay", "__GROUP_ERROR__", payloadJson)
                // Dual-emit: apps correlating request_id-carrying errors
                // (invite-link ops ride the raw channel) need the full frame.
                serverMessageEmitter?.invoke(json.toString())
            }

            "RateLimited" -> {
                // The relay dropped whatever exceeded the bucket — possibly a
                // best-effort member delta whose membership snapshot the
                // translator has already committed. Reset so the next
                // register re-derives deltas from scratch; the worst case is
                // an idempotent re-registration.
                controlOpTranslator.reset()
                serverMessageEmitter?.invoke(json.toString())
                emitDiagnostic("warning", "Relay rate limit hit — translator state reset")
            }

            // Server-plane frames that are app concerns, not SDK concerns —
            // forwarded verbatim as the internet_server_message event so the
            // invite-link lifecycle and misc server events can ride the
            // SDK's socket without a second WebSocket in the app.
            "GroupInviteLinkCreated", "GroupInviteLinkRevoked", "GroupJoinedViaInvite",
            "GroupInviteJoinPending", "GroupRoleChanged", "GroupDeleted" -> {
                serverMessageEmitter?.invoke(json.toString())
                emitDiagnostic("debug", "Relay server message forwarded", mapOf(
                    "type" to messageType
                ))
            }

            else -> {
                // Unknown types are forwarded too — future relay additions
                // surface to the app instead of being silently dropped.
                serverMessageEmitter?.invoke(json.toString())
                emitDiagnostic("debug", "Received relay message", mapOf(
                    "type" to messageType
                ))
            }
        }
    }

    /**
     * Sends a raw, caller-built relay command verbatim (RN
     * `internetSendRawCommand`). The JSON must parse; returns false when
     * invalid or not connected+authenticated. Responses the SDK doesn't
     * consume arrive as `internet_server_message` events.
     */
    fun sendRawCommand(json: String): Boolean {
        if (!isConnected.get() || !isAuthenticated.get()) return false
        val ws = webSocket ?: return false
        val validated = try {
            org.json.JSONObject(json)
        } catch (e: Exception) {
            emitDiagnostic("warning", "Rejected invalid raw server command", mapOf(
                "error" to (e.message ?: "unknown")
            ))
            return false
        }
        return ws.send(validated.toString())
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

        inFlightTracker.prune(System.currentTimeMillis())

        try {
            // Poll for next message from protocol - batch send up to 10 messages per poll
            // to efficiently flush the outbox after reconnection.
            // Batch counter — deliberately NOT the messagesSent metric, which
            // the send paths own.
            var batchSent = 0
            val maxBatchSize = 10

            while (batchSent < maxBatchSize) {
                // Re-check connection state between messages to handle mid-batch disconnects
                if (!isConnected.get() || !isAuthenticated.get()) {
                    emitDiagnostic("warning", "Connection lost mid-batch, stopping message send", mapOf(
                        "messagesSent" to batchSent
                    ))
                    break
                }

                val message = protocol.internetGetNextMessage() ?: break
                val controlOp = message.controlOp
                if (controlOp != null) {
                    sendControlOp(
                        message.messageId,
                        message.recipientId,
                        controlOp,
                        message.controlPayload ?: "",
                        message.data.map { it.toByte() }.toByteArray(),
                        message.replyToMsg
                    )
                } else {
                    sendMessage(message.messageId, message.recipientId, message.data.map { it.toByte() }.toByteArray(), message.replyToMsg)
                }
                batchSent++
            }

            if (batchSent > 1) {
                emitDiagnostic("debug", "Batch sent messages", mapOf(
                    "count" to batchSent
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
            bytesSent.addAndGet(jsonString.toByteArray(Charsets.UTF_8).size.toLong())
            messagesSent.incrementAndGet()
            // Track for recipient-keyed failure correlation: a later
            // DeliveryError for this recipient fails-fast this message id.
            inFlightTracker.recordSent(recipientId, messageId, System.currentTimeMillis())
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
                mainHandler.post { teardownCurrentSocket("Send failures exceeded threshold") }
            }
        }
    }
    
    /**
     * Sends a core-tagged server-plane control frame via the relay-native
     * protocol (see [RelayControlOpTranslator]). Wire-outcome contract is the
     * same as [sendMessage]: the original message id is confirmed on the
     * primary frame's socket-write success, failed otherwise.
     */
    private fun sendControlOp(
        messageId: String,
        recipientId: String,
        controlOp: String,
        controlPayload: String,
        data: ByteArray,
        replyToMsg: String?
    ) {
        when (val translation = controlOpTranslator.translate(controlOp, controlPayload, recipientId)) {
            is RelayControlOpTranslator.Translation.PassThrough -> {
                sendMessage(messageId, recipientId, data, replyToMsg)
            }

            is RelayControlOpTranslator.Translation.Tap -> {
                // Verbatim delivery owns the message id outcome; the extra
                // relay-native frames are best-effort. The translator's state
                // commits only once every extra frame was written — a dropped
                // frame must be re-sent by a later translation, not assumed
                // applied.
                sendMessage(messageId, recipientId, data, replyToMsg)
                if (sendRelayFramesBestEffort(translation.frames, controlOp)) {
                    translation.commit?.invoke()
                }
            }

            is RelayControlOpTranslator.Translation.Replace -> {
                val ws = webSocket
                if (!isConnected.get() || !isAuthenticated.get() || ws == null) {
                    try { protocol.internetSendFailed(messageId) } catch (e: Exception) { Log.e(TAG, "Failed to report send failure for $messageId", e) }
                    return
                }
                val primary = translation.frames.firstOrNull()
                if (primary == null) {
                    // Nothing to send (fully deduped) — the intent is already
                    // reflected server-side; confirm so the core moves on.
                    translation.commit?.invoke()
                    try { protocol.internetConfirmSent(messageId) } catch (e: Exception) { Log.e(TAG, "Failed to confirm send for $messageId", e) }
                    return
                }
                val primaryJson = primary.toString()
                val sent = ws.send(primaryJson)
                if (sent) {
                    consecutiveSendFailures.set(0)
                    bytesSent.addAndGet(primaryJson.toByteArray(Charsets.UTF_8).size.toLong())
                    messagesSent.incrementAndGet()
                    inFlightTracker.recordSent(recipientId, messageId, System.currentTimeMillis())
                    try { protocol.internetConfirmSent(messageId) } catch (e: Exception) { Log.e(TAG, "Failed to confirm send for $messageId", e) }
                    if (sendRelayFramesBestEffort(translation.frames.drop(1), controlOp)) {
                        translation.commit?.invoke()
                    }
                    emitDiagnostic("debug", "Control op sent relay-native", mapOf(
                        "controlOp" to controlOp,
                        "messageId" to messageId,
                        "frames" to translation.frames.size
                    ))
                } else {
                    val failures = consecutiveSendFailures.incrementAndGet()
                    try { protocol.internetSendFailed(messageId) } catch (e: Exception) { Log.e(TAG, "Failed to report send failure for $messageId", e) }
                    emitDiagnostic("error", "Failed to send relay-native control op", mapOf(
                        "controlOp" to controlOp,
                        "messageId" to messageId,
                        "consecutiveFailures" to failures
                    ))
                    if (failures >= MAX_CONSECUTIVE_FAILURES) {
                        mainHandler.post { teardownCurrentSocket("Send failures exceeded threshold") }
                    }
                }
            }
        }
    }

    /** Returns true only when every frame was written to the socket. */
    private fun sendRelayFramesBestEffort(frames: List<org.json.JSONObject>, controlOp: String): Boolean {
        if (frames.isEmpty()) return true
        val ws = webSocket ?: return false
        for (frame in frames) {
            if (!isConnected.get() || !isAuthenticated.get()) return false
            if (!ws.send(frame.toString())) {
                emitDiagnostic("warning", "Best-effort relay frame dropped", mapOf(
                    "controlOp" to controlOp,
                    "frameType" to frame.optString("type")
                ))
                return false
            }
        }
        return true
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

    // MARK: - Presence Watch

    /**
     * Fail-fast handler for the relay's recipient-keyed offline signals
     * (DeliveryError / ConnectionRequestError). Fails every live in-flight
     * message to the recipient with the recipient_unreachable reason (the
     * core classifies it as per-peer no-carrier and parks welcomes without
     * burning budget), ingests an authoritative offline presence, and adds
     * the recipient to the presence watch set.
     */
    private fun handleRecipientUnreachable(recipient: String, reason: String, source: String) {
        if (recipient.isEmpty()) {
            emitDiagnostic("warning", "Recipient-unreachable signal without recipient", mapOf(
                "source" to source,
                "reason" to reason
            ))
            return
        }
        val now = System.currentTimeMillis()
        val failedIds = inFlightTracker.drainRecipient(recipient, now)
        for (id in failedIds) {
            try {
                protocol.internetSendFailedWithReason(id, "recipient_unreachable: $reason")
            } catch (e: Exception) {
                Log.e(TAG, "Failed to fail-fast in-flight message $id", e)
            }
        }
        // Never watch self: a malformed self-addressed frame's DeliveryError
        // would otherwise occupy a rotation slot until the idle TTL.
        if (recipient != deviceId) {
            presenceWatch.watch(recipient, now)
        }
        try {
            protocol.internetPeerPresence(recipient, false, null)
        } catch (e: Exception) {
            Log.e(TAG, "Failed to ingest offline presence for $recipient", e)
        }
        emitDiagnostic("warning", "Recipient unreachable", mapOf(
            "recipient" to recipient,
            "reason" to reason,
            "source" to source,
            "failedInFlight" to failedIds.size
        ))
    }

    private fun startPresenceWatch() {
        stopPresenceWatch()
        mainHandler.postDelayed(presenceWatchRunnable, PresenceWatchPolicy.DEFAULT_TICK_INTERVAL_MS)
    }

    private fun stopPresenceWatch() {
        mainHandler.removeCallbacks(presenceWatchRunnable)
    }

    private fun presenceWatchTick() {
        if (!isConnected.get() || !isAuthenticated.get()) return
        val ws = webSocket ?: return
        try {
            val coreWatchlist = try {
                protocol.internetPresenceWatchlist()
            } catch (e: Exception) {
                Log.e(TAG, "Failed to read presence watchlist", e)
                emptyList()
            }
            val now = System.currentTimeMillis()
            val peers = presenceWatch.peersToQuery(coreWatchlist, now).filter { it != deviceId }
            for (peer in peers) {
                val checkMessage = org.json.JSONObject().apply {
                    put("type", "CheckPresence")
                    put("username", peer)
                }
                ws.send(checkMessage.toString())
            }
            if (peers.isNotEmpty()) {
                emitDiagnostic("debug", "Presence watch tick", mapOf(
                    "queried" to peers.size,
                    "watched" to presenceWatch.watchedPeers().size
                ))
            }
        } catch (e: Exception) {
            emitDiagnostic("error", "Presence watch tick failed", mapOf(
                "error" to (e.message ?: "unknown")
            ))
        }
    }

    /**
     * App-facing one-shot presence query (RN `checkInternetPresence`). The
     * answer arrives as the SDK's `presence_updated` event — fire-and-event,
     * matching relay semantics. Returns true if the query was written to the
     * socket.
     */
    fun checkPresence(userId: String): Boolean {
        if (userId.isEmpty() || !isConnected.get() || !isAuthenticated.get()) return false
        val ws = webSocket ?: return false
        val checkMessage = org.json.JSONObject().apply {
            put("type", "CheckPresence")
            put("username", userId)
        }
        return ws.send(checkMessage.toString())
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

