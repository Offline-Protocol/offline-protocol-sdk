package com.offlineprotocol

import android.os.Handler
import android.os.HandlerThread
import android.os.Looper
import android.util.Base64
import android.util.Log
import okhttp3.OkHttpClient
import okhttp3.Request
import okhttp3.Response
import okhttp3.WebSocket
import okhttp3.WebSocketListener
import org.json.JSONArray
import org.json.JSONObject
import uniffi.offline_protocol.OfflineProtocol
import java.security.MessageDigest
import java.util.UUID
import java.util.concurrent.CountDownLatch
import java.util.concurrent.TimeUnit
import java.util.concurrent.atomic.AtomicBoolean
import java.util.concurrent.atomic.AtomicInteger
import java.util.concurrent.atomic.AtomicLong
import javax.crypto.Mac
import javax.crypto.spec.SecretKeySpec

/**
 * Nostr Manager implementing TransportManager for Nostr relay communication.
 * Connects to Nostr relays via WebSocket and uses NIP-04 (kind 4) direct messages
 * for protocol message routing.
 */
class NostrManager(
    private val context: android.content.Context,
    private val protocol: OfflineProtocol,
    private val deviceId: String,
    private val diagnosticEmitter: ((String, String, Map<String, Any?>) -> Unit)? = null
) : TransportManager {

    // MARK: - TransportManager Implementation

    override val transportId = "nostr"
    override val transportName = "Nostr (Relay)"
    @Volatile
    override var state: TransportState = TransportState.UNAVAILABLE
        private set
    override var listener: TransportManagerListener? = null

    companion object {
        private const val TAG = "NostrManager"

        private const val MESSAGE_POLL_INTERVAL_MS = 100L   // 100ms — same as InternetManager
        private const val RECONNECT_INITIAL_DELAY_MS = 1000L
        private const val RECONNECT_MAX_DELAY_MS = 30000L
        private const val RECONNECT_BACKOFF_MULTIPLIER = 2.0
        private const val CONNECTION_TIMEOUT_MS = 30L  // seconds for OkHttp
        private const val PING_INTERVAL_MS = 30000L
        private const val MAX_CONSECUTIVE_FAILURES = 2
    }

    // MARK: - Properties

    private var relayUrls: List<String> = emptyList()
    private var autoReconnect = true
    private var maxReconnectAttempts = 0 // 0 = infinite

    // Handler for main thread operations
    private val mainHandler = Handler(Looper.getMainLooper())

    // Background handler thread for message polling
    private var ioThread: HandlerThread? = null
    private var ioHandler: Handler? = null

    // Message polling runnable
    private val messagePollingRunnable = object : Runnable {
        override fun run() {
            pollAndSendMessages()
            if (state == TransportState.RUNNING && isConnected.get()) {
                ioHandler?.postDelayed(this, MESSAGE_POLL_INTERVAL_MS)
            }
        }
    }

    // Ping runnable
    private val pingRunnable = object : Runnable {
        override fun run() {
            sendPings()
            if (state == TransportState.RUNNING && isConnected.get()) {
                ioHandler?.postDelayed(this, PING_INTERVAL_MS)
            }
        }
    }

    // OkHttp client for WebSocket connections
    private var okHttpClient: OkHttpClient? = null

    // Relay connections
    private val relayWebSockets = mutableMapOf<String, WebSocket>()
    private val relayConnected = mutableMapOf<String, Boolean>()
    private val subscriptionIds = mutableMapOf<String, String>()
    private val relayLock = Object()

    // Nostr identity (deterministic from deviceId)
    private var privateKeyBytes: ByteArray = ByteArray(0)
    private var publicKeyHex: String = ""

    // Configuration state
    private val isConfigured = AtomicBoolean(false)

    // Connection state
    private val isConnected = AtomicBoolean(false)
    private val reconnectAttempts = mutableMapOf<String, AtomicInteger>()
    private val currentReconnectDelay = mutableMapOf<String, AtomicLong>()
    private val reconnectRunnables = mutableMapOf<String, Runnable>()

    // Failure tracking for DORS
    private val consecutiveSendFailures = AtomicInteger(0)

    // Metrics
    private val bytesSent = AtomicLong(0)
    private val bytesReceived = AtomicLong(0)
    private val messagesSent = AtomicLong(0)
    private val messagesReceived = AtomicLong(0)

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
            if (!latch.await(5, TimeUnit.SECONDS)) {
                throw RuntimeException("Timed out waiting for main thread execution (5s)")
            }
        } catch (ie: InterruptedException) {
            Thread.currentThread().interrupt()
            throw RuntimeException("Interrupted while executing on main thread", ie)
        }

        return outcome!!.getOrThrow()
    }

    // MARK: - Configuration

    /**
     * Configure the Nostr relay connections.
     * @param relayUrls List of Nostr relay WebSocket URLs
     * @param autoReconnect Whether to auto-reconnect on disconnect (default: true)
     * @param maxReconnectAttempts Max reconnect attempts per relay, 0 = infinite (default: 0)
     */
    fun configure(
        relayUrls: List<String>,
        autoReconnect: Boolean = true,
        maxReconnectAttempts: Int = 0
    ) {
        this.relayUrls = relayUrls
        this.autoReconnect = autoReconnect
        this.maxReconnectAttempts = maxReconnectAttempts

        // Derive deterministic Nostr keypair from deviceId
        deriveKeyPair()

        isConfigured.set(true)

        emitDiagnostic("info", "Nostr transport configured", mapOf(
            "relayCount" to relayUrls.size,
            "relayUrls" to relayUrls,
            "autoReconnect" to autoReconnect,
            "maxReconnectAttempts" to maxReconnectAttempts,
            "publicKey" to publicKeyHex
        ))
    }

    // MARK: - TransportManager Implementation

    override fun isAvailable(): Boolean {
        return isConfigured.get() && relayUrls.isNotEmpty()
    }

    override fun start() {
        runOnMainSync {
            startUnsafe()
        }
    }

    private fun startUnsafe() {
        if (state == TransportState.RUNNING || state == TransportState.STARTING) {
            throw TransportException.AlreadyRunning()
        }

        if (relayUrls.isEmpty()) {
            throw TransportException.NotAvailable("No relay URLs configured. Call configure(relayUrls:) first.")
        }

        Log.i(TAG, "Starting Nostr transport for device: $deviceId")
        emitDiagnostic("info", "Starting Nostr transport", mapOf(
            "deviceId" to deviceId,
            "relayCount" to relayUrls.size,
            "publicKey" to publicKeyHex
        ))

        // Start background IO thread
        val thread = HandlerThread("NostrIO").also { it.start() }
        ioThread = thread
        ioHandler = Handler(thread.looper)

        // Create OkHttp client
        okHttpClient = OkHttpClient.Builder()
            .connectTimeout(CONNECTION_TIMEOUT_MS, TimeUnit.SECONDS)
            .readTimeout(0, TimeUnit.SECONDS) // No read timeout for WebSocket
            .writeTimeout(10, TimeUnit.SECONDS)
            .pingInterval(PING_INTERVAL_MS, TimeUnit.MILLISECONDS)
            .build()

        updateState(TransportState.STARTING)

        // Connect to all relays
        for (relayUrl in relayUrls) {
            connectToRelay(relayUrl)
        }
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

        // Cancel all reconnect attempts
        for ((_, runnable) in reconnectRunnables) {
            mainHandler.removeCallbacks(runnable)
        }
        reconnectRunnables.clear()

        // Stop timers
        stopMessagePolling()
        stopPingTimer()

        // Close all WebSocket connections
        disconnectAll()

        // Shut down IO thread
        ioThread?.quitSafely()
        ioThread = null
        ioHandler = null

        // Notify protocol
        try {
            protocol.nostrStatusChanged(false)
        } catch (e: Exception) {
            Log.e(TAG, "Error notifying protocol of disconnect", e)
        }

        updateState(TransportState.STOPPED)
        emitDiagnostic("info", "Nostr transport stopped")
    }

    override fun pause() {
        ioHandler?.post {
            stopMessagePolling()
            stopPingTimer()
        }
    }

    override fun resume() {
        ioHandler?.post {
            if (state == TransportState.RUNNING && isConnected.get()) {
                startMessagePolling()
                startPingTimer()
            }
        }
    }

    /**
     * Called by the Rust transport callback when new outgoing messages are available.
     */
    fun onMessagesAvailable() {
        ioHandler?.post { pollAndSendMessages() }
    }

    override fun getMetrics(): Map<String, Any> {
        val connectedCount = synchronized(relayLock) {
            relayConnected.values.count { it }
        }
        return mapOf(
            "bytes_sent" to bytesSent.get(),
            "bytes_received" to bytesReceived.get(),
            "messages_sent" to messagesSent.get(),
            "messages_received" to messagesReceived.get(),
            "is_connected" to isConnected.get(),
            "connected_relays" to connectedCount,
            "total_relays" to relayUrls.size
        )
    }

    // MARK: - Relay Connection Management

    private fun connectToRelay(relayUrl: String) {
        if (state == TransportState.STOPPING || state == TransportState.STOPPED) return

        emitDiagnostic("info", "Connecting to Nostr relay", mapOf(
            "relayUrl" to relayUrl
        ))

        val request = Request.Builder()
            .url(relayUrl)
            .build()

        val ws = okHttpClient?.newWebSocket(request, object : WebSocketListener() {
            override fun onOpen(webSocket: WebSocket, response: Response) {
                handleRelayConnected(relayUrl, webSocket)
            }

            override fun onMessage(webSocket: WebSocket, text: String) {
                bytesReceived.addAndGet(text.length.toLong())
                processNostrMessage(text)
            }

            override fun onClosing(webSocket: WebSocket, code: Int, reason: String) {
                webSocket.close(1000, null)
            }

            override fun onClosed(webSocket: WebSocket, code: Int, reason: String) {
                handleRelayDisconnected(relayUrl, "Closed: $reason")
            }

            override fun onFailure(webSocket: WebSocket, t: Throwable, response: Response?) {
                handleRelayDisconnected(relayUrl, t.message ?: "Unknown error")
            }
        })

        if (ws != null) {
            synchronized(relayLock) {
                relayWebSockets[relayUrl] = ws
                relayConnected[relayUrl] = false
            }
        }
    }

    private fun disconnectAll() {
        synchronized(relayLock) {
            for ((_, ws) in relayWebSockets) {
                try { ws.close(1000, "Stopping") } catch (_: Exception) {}
            }
            relayWebSockets.clear()
            relayConnected.clear()
            subscriptionIds.clear()
        }
        isConnected.set(false)
    }

    private fun handleRelayConnected(relayUrl: String, webSocket: WebSocket) {
        synchronized(relayLock) {
            relayWebSockets[relayUrl] = webSocket
            relayConnected[relayUrl] = true
        }

        // Reset reconnection state
        reconnectAttempts[relayUrl]?.set(0)
        currentReconnectDelay[relayUrl]?.set(RECONNECT_INITIAL_DELAY_MS)

        emitDiagnostic("info", "Connected to Nostr relay", mapOf(
            "relayUrl" to relayUrl
        ))

        // Send subscription for messages addressed to this device
        sendSubscription(relayUrl, webSocket)

        // Update overall connection status
        updateConnectionStatus()
    }

    private fun handleRelayDisconnected(relayUrl: String, reason: String?) {
        val wasConnected: Boolean
        synchronized(relayLock) {
            wasConnected = relayConnected[relayUrl] ?: false
            relayConnected[relayUrl] = false
            relayWebSockets.remove(relayUrl)
            subscriptionIds.remove(relayUrl)
        }

        emitDiagnostic("warning", "Nostr relay disconnected", mapOf(
            "relayUrl" to relayUrl,
            "reason" to (reason ?: "none"),
            "wasConnected" to wasConnected
        ))

        // Update overall connection status
        updateConnectionStatus()

        // Attempt reconnection if enabled
        if (autoReconnect && state != TransportState.STOPPING && state != TransportState.STOPPED) {
            mainHandler.post { scheduleReconnect(relayUrl) }
        }
    }

    private fun updateConnectionStatus() {
        val anyConnected = synchronized(relayLock) {
            relayConnected.values.any { it }
        }

        val wasConnected = isConnected.getAndSet(anyConnected)

        if (anyConnected && !wasConnected) {
            // Became connected
            consecutiveSendFailures.set(0)
            mainHandler.post {
                updateState(TransportState.RUNNING)
                try {
                    protocol.nostrStatusChanged(true)
                } catch (e: Exception) {
                    Log.e(TAG, "Error notifying protocol of connect", e)
                }
                startMessagePolling()
                startPingTimer()
                ioHandler?.post { pollAndSendMessages() }
            }
        } else if (!anyConnected && wasConnected) {
            // Lost all connections
            mainHandler.post {
                stopMessagePolling()
                stopPingTimer()
                try {
                    protocol.nostrStatusChanged(false)
                } catch (e: Exception) {
                    Log.e(TAG, "Error notifying protocol of disconnect", e)
                }
                if (!autoReconnect) {
                    updateState(TransportState.STOPPED)
                }
            }
        }
    }

    private fun scheduleReconnect(relayUrl: String) {
        if (!autoReconnect) return

        val counter = reconnectAttempts.getOrPut(relayUrl) { AtomicInteger(0) }
        val attempts = counter.incrementAndGet()

        if (maxReconnectAttempts > 0 && attempts > maxReconnectAttempts) {
            emitDiagnostic("error", "Max reconnect attempts reached for relay", mapOf(
                "relayUrl" to relayUrl,
                "attempts" to attempts,
                "maxAttempts" to maxReconnectAttempts
            ))
            return
        }

        val delayHolder = currentReconnectDelay.getOrPut(relayUrl) { AtomicLong(RECONNECT_INITIAL_DELAY_MS) }
        val delay = delayHolder.get()
        delayHolder.set(minOf((delay * RECONNECT_BACKOFF_MULTIPLIER).toLong(), RECONNECT_MAX_DELAY_MS))

        emitDiagnostic("info", "Scheduling reconnect to Nostr relay", mapOf(
            "relayUrl" to relayUrl,
            "attempt" to attempts,
            "delayMs" to delay
        ))

        reconnectRunnables[relayUrl]?.let { mainHandler.removeCallbacks(it) }
        val runnable = Runnable { connectToRelay(relayUrl) }
        reconnectRunnables[relayUrl] = runnable
        mainHandler.postDelayed(runnable, delay)
    }

    // MARK: - Nostr Protocol (NIP-01 / NIP-04)

    private fun sendSubscription(relayUrl: String, webSocket: WebSocket) {
        val subId = UUID.randomUUID().toString().replace("-", "").take(16)

        synchronized(relayLock) {
            subscriptionIds[relayUrl] = subId
        }

        // NIP-01 REQ: subscribe to kind 4 (DM) events tagged with our pubkey
        val reqJson = "[\"REQ\",\"$subId\",{\"#p\":[\"$publicKeyHex\"],\"kinds\":[4]}]"
        webSocket.send(reqJson)

        emitDiagnostic("debug", "Sent subscription to relay", mapOf(
            "relayUrl" to relayUrl,
            "subscriptionId" to subId,
            "publicKey" to publicKeyHex
        ))
    }

    private fun createNostrEvent(content: String, recipientPubkey: String): String? {
        val createdAt = System.currentTimeMillis() / 1000
        val kind = 4 // NIP-04 DM

        // Build event for signing (NIP-01 serialization)
        val serialized = "[0,\"$publicKeyHex\",$createdAt,$kind,[[\"p\",\"$recipientPubkey\"]],${JSONObject.quote(content)}]"

        // Compute event ID (SHA-256)
        val eventId = sha256Hex(serialized)

        // Sign event
        val signature = signEvent(eventId)

        val eventJson = JSONObject().apply {
            put("id", eventId)
            put("pubkey", publicKeyHex)
            put("created_at", createdAt)
            put("kind", kind)
            put("tags", JSONArray().apply {
                put(JSONArray().apply {
                    put("p")
                    put(recipientPubkey)
                })
            })
            put("content", content)
            put("sig", signature)
        }

        return "[\"EVENT\",${eventJson}]"
    }

    private fun processNostrMessage(text: String) {
        val json: JSONArray
        try {
            json = JSONArray(text)
        } catch (e: Exception) {
            return
        }

        val messageType = json.optString(0, "")

        when (messageType) {
            "EVENT" -> {
                if (json.length() < 3) return
                val event = json.optJSONObject(2) ?: return
                val senderPubkey = event.optString("pubkey", "")
                val content = event.optString("content", "")

                if (senderPubkey.isEmpty() || content.isEmpty()) return

                // Skip events from self
                if (senderPubkey == publicKeyHex) return

                messagesReceived.incrementAndGet()

                val senderId = deviceIdFromPubkey(senderPubkey)

                try {
                    val messageBytes: ByteArray = try {
                        Base64.decode(content, Base64.NO_WRAP)
                    } catch (_: Exception) {
                        content.toByteArray(Charsets.UTF_8)
                    }

                    protocol.nostrMessageReceived(
                        senderId,
                        messageBytes.map { it.toUByte() }
                    )

                    emitDiagnostic("debug", "Message received from Nostr", mapOf(
                        "senderId" to senderId,
                        "senderPubkey" to senderPubkey.take(16) + "...",
                        "contentLength" to content.length
                    ))
                } catch (e: Exception) {
                    emitDiagnostic("error", "Error processing Nostr message", mapOf(
                        "error" to (e.message ?: "unknown")
                    ))
                }
            }

            "OK" -> {
                if (json.length() >= 3) {
                    val eventId = json.optString(1, "")
                    val accepted = json.optBoolean(2, false)
                    emitDiagnostic("debug", "Relay event response", mapOf(
                        "eventId" to eventId.take(16) + "...",
                        "accepted" to accepted
                    ))
                }
            }

            "EOSE" -> {
                emitDiagnostic("debug", "End of stored events received")
            }

            "NOTICE" -> {
                if (json.length() >= 2) {
                    val message = json.optString(1, "")
                    emitDiagnostic("warning", "Relay notice", mapOf("message" to message))
                }
            }

            else -> {
                emitDiagnostic("debug", "Unknown Nostr message type", mapOf("type" to messageType))
            }
        }
    }

    // MARK: - Message Polling

    private fun startMessagePolling() {
        stopMessagePolling()
        ioHandler?.post(messagePollingRunnable)
    }

    private fun stopMessagePolling() {
        ioHandler?.removeCallbacks(messagePollingRunnable)
    }

    private fun startPingTimer() {
        stopPingTimer()
        ioHandler?.postDelayed(pingRunnable, PING_INTERVAL_MS)
    }

    private fun stopPingTimer() {
        ioHandler?.removeCallbacks(pingRunnable)
    }

    private fun sendPings() {
        // OkHttp handles WebSocket pings via pingInterval, so this is a no-op.
        // We keep the timer for consistency with the iOS implementation.
    }

    private fun pollAndSendMessages() {
        if (!isConnected.get()) return

        try {
            var sent = 0
            val maxBatchSize = 10

            while (sent < maxBatchSize) {
                if (!isConnected.get()) break

                val message = protocol.nostrGetNextMessage() ?: break
                publishMessage(
                    message.messageId,
                    message.recipientId,
                    message.data.map { it.toByte() }.toByteArray(),
                    message.replyToMsg
                )
                sent++
            }

            if (sent > 1) {
                emitDiagnostic("debug", "Batch sent messages via Nostr", mapOf(
                    "count" to sent
                ))
            }
        } catch (e: Exception) {
            emitDiagnostic("error", "Error polling Nostr messages", mapOf(
                "error" to (e.message ?: "unknown")
            ))
        }
    }

    private fun publishMessage(
        messageId: String,
        recipientId: String,
        data: ByteArray,
        replyToMsg: String? = null
    ) {
        if (!isConnected.get()) {
            emitDiagnostic("warning", "Cannot send message - not connected", mapOf(
                "messageId" to messageId,
                "recipientId" to recipientId
            ))
            try { protocol.nostrSendFailed(messageId) } catch (e: Exception) {
                Log.e(TAG, "Failed to report send failure for $messageId", e)
            }
            return
        }

        // Derive recipient's Nostr pubkey from their deviceId
        val recipientPubkey = pubkeyFromDeviceId(recipientId)

        // Encode message data as base64
        val content = Base64.encodeToString(data, Base64.NO_WRAP)

        // Create signed Nostr event
        val eventMessage = createNostrEvent(content, recipientPubkey) ?: run {
            emitDiagnostic("error", "Failed to create Nostr event")
            try { protocol.nostrSendFailed(messageId) } catch (e: Exception) {
                Log.e(TAG, "Failed to report send failure for $messageId", e)
            }
            return
        }

        // Get connected relays
        val connectedRelays = synchronized(relayLock) {
            relayWebSockets.filter { relayConnected[it.key] == true }
        }

        if (connectedRelays.isEmpty()) {
            try { protocol.nostrSendFailed(messageId) } catch (e: Exception) {
                Log.e(TAG, "Failed to report send failure for $messageId", e)
            }
            return
        }

        // Send to first relay for confirmation, fan out to rest
        val entries = connectedRelays.entries.toList()
        val primary = entries[0]
        val others = entries.drop(1)

        try {
            val sent = primary.value.send(eventMessage)
            if (sent) {
                consecutiveSendFailures.set(0)
                bytesSent.addAndGet(eventMessage.length.toLong())
                messagesSent.incrementAndGet()
                try { protocol.nostrConfirmSent(messageId) } catch (e: Exception) {
                    Log.e(TAG, "Failed to confirm send for $messageId", e)
                }

                emitDiagnostic("debug", "Message sent via Nostr", mapOf(
                    "messageId" to messageId,
                    "recipientId" to recipientId,
                    "contentLength" to content.length
                ))
            } else {
                throw Exception("WebSocket send returned false")
            }
        } catch (e: Exception) {
            val failures = consecutiveSendFailures.incrementAndGet()
            try { protocol.nostrSendFailed(messageId) } catch (ex: Exception) {
                Log.e(TAG, "Failed to report send failure for $messageId", ex)
            }
            emitDiagnostic("error", "Failed to send Nostr message", mapOf(
                "messageId" to messageId,
                "recipientId" to recipientId,
                "relayUrl" to primary.key,
                "consecutiveFailures" to failures,
                "error" to (e.message ?: "unknown")
            ))

            if (failures >= MAX_CONSECUTIVE_FAILURES) {
                emitDiagnostic("warning", "Too many consecutive send failures, triggering disconnect", mapOf(
                    "failures" to failures
                ))
                handleRelayDisconnected(primary.key, "Send failures exceeded threshold")
            }
        }

        // Fan out to other relays (best-effort)
        for ((_, ws) in others) {
            try { ws.send(eventMessage) } catch (_: Exception) {}
        }
    }

    // MARK: - Nostr Crypto Helpers

    /**
     * Derive a deterministic secp256k1 keypair from the deviceId.
     * Both peers can compute each other's pubkeys from device IDs.
     */
    private fun deriveKeyPair() {
        privateKeyBytes = sha256Bytes(deviceId)
        // Deterministic pubkey from private key (simplified — use secp256k1 library for production)
        publicKeyHex = sha256Hex(Base64.encodeToString(privateKeyBytes, Base64.NO_WRAP))

        emitDiagnostic("debug", "Nostr keypair derived", mapOf(
            "publicKey" to publicKeyHex,
            "deviceId" to deviceId
        ))
    }

    private fun pubkeyFromDeviceId(deviceId: String): String {
        val privKeyBytes = sha256Bytes(deviceId)
        return sha256Hex(Base64.encodeToString(privKeyBytes, Base64.NO_WRAP))
    }

    private fun deviceIdFromPubkey(pubkey: String): String {
        // In a full implementation, maintain a pubkey->deviceId cache
        return pubkey
    }

    /**
     * Sign a Nostr event ID.
     * Note: Simplified HMAC signature. Replace with BIP-340 Schnorr when secp256k1-kmp is added.
     */
    private fun signEvent(eventId: String): String {
        val mac = Mac.getInstance("HmacSHA256")
        mac.init(SecretKeySpec(privateKeyBytes, "HmacSHA256"))
        val hmac = mac.doFinal(eventId.toByteArray(Charsets.UTF_8))
        val hmacHex = hmac.joinToString("") { "%02x".format(it) }
        // Pad to 128 hex chars (64 bytes) as Nostr expects
        return hmacHex + hmacHex
    }

    // MARK: - Utility

    private fun sha256Bytes(input: String): ByteArray {
        return MessageDigest.getInstance("SHA-256").digest(input.toByteArray(Charsets.UTF_8))
    }

    private fun sha256Hex(input: String): String {
        return sha256Bytes(input).joinToString("") { "%02x".format(it) }
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
