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
import java.util.UUID
import java.util.concurrent.CountDownLatch
import java.util.concurrent.TimeUnit
import java.util.concurrent.atomic.AtomicBoolean
import java.util.concurrent.atomic.AtomicInteger
import java.util.concurrent.atomic.AtomicLong

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
        private const val CONNECTION_TIMEOUT_SECONDS = 30L
        private const val PING_INTERVAL_MS = 30000L  // OkHttp WebSocket ping interval
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
            pollAndSendQueries()
            if (state == TransportState.RUNNING && isConnected.get()) {
                ioHandler?.postDelayed(this, MESSAGE_POLL_INTERVAL_MS)
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

    // Nostr identity (obtained from Rust core)
    private var publicKeyHex: String = ""

    // Configuration state
    private val isConfigured = AtomicBoolean(false)

    // Connection state
    private val isConnected = AtomicBoolean(false)
    private val reconnectAttempts = mutableMapOf<String, AtomicInteger>()
    private val currentReconnectDelay = mutableMapOf<String, AtomicLong>()
    private val reconnectRunnables = mutableMapOf<String, Runnable>()

    // Pending relay confirmations: Nostr event_id → protocol message_id.
    // Populated when a WebSocket send succeeds; removed on relay ["OK", ...].
    private val pendingEventConfirmations = mutableMapOf<String, String>()

    // Subscription ids belonging to in-flight key-package resolution queries,
    // as opposed to the standing message subscription. Events arriving under
    // one of these are records fetched on the transport's behalf, not inbound
    // messages, and go to a different entry point.
    private val activeQueryIds = mutableSetOf<String>()
    private val pendingEventLock = Object()

    // Failure tracking for DORS
    private val consecutiveSendFailures = AtomicInteger(0)

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

        // Get the Nostr signing pubkey from the Rust core. This is a
        // per-install key that settles when MLS initialization installs the
        // persisted signing secret; sendSubscription() refreshes it on every
        // relay (re)connect in case that happens after configure().
        this.publicKeyHex = protocol.nostrGetPublicKey() ?: ""

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
            .connectTimeout(CONNECTION_TIMEOUT_SECONDS, TimeUnit.SECONDS)
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
        }
    }

    override fun resume() {
        ioHandler?.post {
            if (state == TransportState.RUNNING && isConnected.get()) {
                startMessagePolling()
            }
        }
    }

    /**
     * Called by the Rust transport callback when new outgoing messages are available.
     */
    fun onMessagesAvailable() {
        ioHandler?.post { pollAndSendMessages() }
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
                ioHandler?.post { pollAndSendMessages() }
            }
        } else if (!anyConnected && wasConnected) {
            // Lost all connections
            mainHandler.post {
                stopMessagePolling()
                releaseActiveQueries()
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
        // Re-read the signing pubkey used for self-event filtering: it
        // rotates when MLS initialization installs the persisted signing
        // secret, and this runs on every relay (re)connect — before any
        // event can arrive on this subscription.
        protocol.nostrGetPublicKey()?.takeIf { it.isNotEmpty() }?.let { publicKeyHex = it }

        val subId = UUID.randomUUID().toString().replace("-", "").take(16)

        synchronized(relayLock) {
            subscriptionIds[relayUrl] = subId
        }

        // Get the subscription filter from Rust (uses the real BIP-340 pubkey)
        val reqJson = protocol.nostrGetSubscriptionFilter(subId) ?: run {
            emitDiagnostic("error", "Failed to get subscription filter from Rust core")
            return
        }
        webSocket.send(reqJson)

        emitDiagnostic("debug", "Sent subscription to relay", mapOf(
            "relayUrl" to relayUrl,
            "subscriptionId" to subId,
            "publicKey" to publicKeyHex
        ))
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

                // Route key-package resolution answers away from the message
                // path *before* anything else looks at them. They are not
                // messages: the content is sealed to a different key, and the
                // self-event filter below would be meaningless for a record we
                // deliberately fetched.
                val eventSubId = json.optString(1, "")
                if (eventSubId.isNotEmpty() && isActiveQuery(eventSubId)) {
                    handleQueryEvent(eventSubId, event)
                    return
                }

                // Skip events we published ourselves.
                //
                // This only ever catches the LEGACY unsealed form. Sealed
                // frames (NIP-59 gift wraps, kind 1059) are signed by a fresh
                // single-use key per event — that unlinkability is the point —
                // so `pubkey` never equals ours and this guard cannot fire for
                // them. Do not "fix" that by comparing something else: there is
                // nothing on a gift wrap that identifies its author, by design.
                //
                // Nothing is lost. The subscription filters on `#p` = our own
                // routing tag, so our outbound events (addressed to a *peer's*
                // tag) are not delivered here in the first place; the only way
                // to receive our own gift wrap is to message ourselves, and the
                // engine's message-id deduplication and self-suppression handle
                // that case on the Rust side.
                if (senderPubkey == publicKeyHex) return

                // NIP-01 requires `created_at`; a missing or malformed one
                // defaults to 0, which Rust ignores for the watermark rather
                // than treating as receive progress.
                val createdAt = event.optLong("created_at", 0L)

                try {
                    val messageBytes: ByteArray = try {
                        Base64.decode(content, Base64.NO_WRAP)
                    } catch (_: Exception) {
                        content.toByteArray(Charsets.UTF_8)
                    }

                    // Pass the Nostr pubkey as sender_id — Rust extracts
                    // the real protocol-level sender from Message.sender.
                    // `createdAt` advances the persisted receive watermark,
                    // which becomes the `since` on the next subscription — the
                    // bound that stops a relay replaying its whole retention
                    // window on every reconnect.
                    protocol.nostrMessageReceivedAt(
                        senderPubkey,
                        messageBytes.map { it.toUByte() },
                        createdAt
                    )

                    emitDiagnostic("debug", "Message received from Nostr", mapOf(
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
                // Relay acceptance/rejection: ["OK", event_id, accepted, reason?]
                if (json.length() >= 3) {
                    val eventId = json.optString(1, "")
                    val accepted = json.optBoolean(2, false)
                    val reason = if (json.length() >= 4) json.optString(3, "") else null

                    // Look up the protocol message_id for this Nostr event_id
                    val messageId = synchronized(pendingEventLock) {
                        pendingEventConfirmations.remove(eventId)
                    }

                    if (messageId != null) {
                        try {
                            if (accepted) {
                                protocol.nostrConfirmSent(messageId)
                            } else {
                                protocol.nostrSendFailedWithReason(
                                    messageId,
                                    reason?.ifEmpty { "Relay rejected event" } ?: "Relay rejected event"
                                )
                            }
                        } catch (e: Exception) {
                            Log.e(TAG, "Failed to report relay OK for $messageId", e)
                        }
                    }

                    emitDiagnostic("debug", "Relay event response", mapOf(
                        "eventId" to eventId.take(16) + "...",
                        "accepted" to accepted,
                        "reason" to (reason ?: "none"),
                        "tracked" to (messageId != null)
                    ))
                }
            }

            "EOSE" -> {
                // End of stored events. For the standing message subscription
                // this just means "live from here"; for a resolution query it
                // means the relay has given us everything it holds, so the
                // query is done and its subscription should not stay open.
                val eoseSubId = if (json.length() >= 2) json.optString(1, "") else ""
                if (eoseSubId.isNotEmpty() && isActiveQuery(eoseSubId)) {
                    finishQuery(eoseSubId)
                } else {
                    emitDiagnostic("debug", "End of stored events received")
                }
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


    private fun pollAndSendMessages() {
        if (!isConnected.get()) return

        try {
            var sent = 0
            val maxBatchSize = 10

            while (sent < maxBatchSize) {
                if (!isConnected.get()) break

                val message = protocol.nostrGetNextMessage() ?: break
                // event_json is the complete signed ["EVENT", {...}] from Rust
                publishMessage(message.messageId, message.eventId, message.eventJson)
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

    // MARK: - Key-Package Resolution Queries

    private fun isActiveQuery(subscriptionId: String): Boolean =
        synchronized(relayLock) { activeQueryIds.contains(subscriptionId) }

    /**
     * Drains queries the transport wants issued and sends each REQ to every
     * connected relay.
     *
     * Broadcast rather than primary-only, unlike an outgoing event: a peer's
     * published records may sit on relays we share with them but not with the
     * first one in our list, and there is no acknowledgement that would tell us
     * we asked the wrong one. Duplicate answers are free — the transport opens
     * each independently and the engine deduplicates the key package.
     */
    private fun pollAndSendQueries() {
        if (!isConnected.get()) return

        try {
            var issued = 0
            val maxBatchSize = 5

            while (issued < maxBatchSize) {
                val query = protocol.nostrGetNextQuery() ?: break
                issued++

                val connectedRelays = synchronized(relayLock) {
                    relayWebSockets.filter { relayConnected[it.key] == true }
                }

                if (connectedRelays.isEmpty()) {
                    // Nothing to ask. Release it rather than leaving the
                    // transport holding an entry no answer will ever arrive
                    // for; the next send to that peer re-queues it once the
                    // rate limit lapses.
                    protocol.nostrQueryCompleted(query.queryId)
                    break
                }

                synchronized(relayLock) { activeQueryIds.add(query.queryId) }

                for ((_, socket) in connectedRelays) {
                    socket.send(query.reqJson)
                }

                emitDiagnostic("debug", "Issued Nostr key-package query", mapOf(
                    "queryId" to query.queryId,
                    "relays" to connectedRelays.size
                ))
            }
        } catch (e: Exception) {
            emitDiagnostic("error", "Error polling Nostr key-package queries", mapOf(
                "error" to (e.message ?: "unknown")
            ))
        }
    }

    private fun handleQueryEvent(subscriptionId: String, event: JSONObject) {
        try {
            protocol.nostrQueryEventReceived(subscriptionId, event.toString())
        } catch (e: Exception) {
            emitDiagnostic("error", "Error processing Nostr key-package record", mapOf(
                "error" to (e.message ?: "unknown")
            ))
        }
    }

    /**
     * Closes a finished query on every relay and releases it in the transport.
     *
     * A query is broadcast, so more than one relay answers it and each sends
     * its own EOSE. The first one closes it: leaving the subscription open for
     * the stragglers would keep a live filter on a peer's routing tag for the
     * life of the connection, which is precisely the standing signal this
     * design avoids elsewhere. A later relay's records are simply missed, and
     * the next send to that peer re-queues the lookup.
     */
    /**
     * Releases every in-flight resolution query after the relays drop.
     *
     * A query whose relays went away before EOSE never finishes: nothing will
     * ever answer it, so without this the bridge holds its subscription id for
     * the life of the process and the transport holds the entry until its own
     * cap evicts something — possibly a live query. Letting them go costs
     * nothing, since the next send to those peers re-queues the lookup once the
     * resolution rate limit lapses.
     */
    private fun releaseActiveQueries() {
        val queryIds = synchronized(relayLock) {
            val ids = activeQueryIds.toList()
            activeQueryIds.clear()
            ids
        }
        if (queryIds.isEmpty()) return

        for (queryId in queryIds) {
            try {
                protocol.nostrQueryCompleted(queryId)
            } catch (e: Exception) {
                Log.e(TAG, "Failed to release Nostr query $queryId", e)
            }
        }

        emitDiagnostic("debug", "Released in-flight Nostr key-package queries", mapOf(
            "count" to queryIds.size
        ))
    }

    private fun finishQuery(subscriptionId: String) {
        val wasActive = synchronized(relayLock) { activeQueryIds.remove(subscriptionId) }
        if (!wasActive) return

        val closeMessage = JSONArray().put("CLOSE").put(subscriptionId).toString()
        val connectedRelays = synchronized(relayLock) {
            relayWebSockets.filter { relayConnected[it.key] == true }
        }
        for ((_, socket) in connectedRelays) {
            socket.send(closeMessage)
        }

        try {
            protocol.nostrQueryCompleted(subscriptionId)
        } catch (e: Exception) {
            Log.e(TAG, "Failed to release Nostr query $subscriptionId", e)
        }
    }

    private fun publishMessage(
        messageId: String,
        eventId: String,
        eventMessage: String
    ) {
        if (!isConnected.get()) {
            emitDiagnostic("warning", "Cannot send message - not connected", mapOf(
                "messageId" to messageId
            ))
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

                // Track event_id → message_id so we can confirm/fail
                // when the relay sends ["OK", event_id, accepted, reason].
                synchronized(pendingEventLock) {
                    pendingEventConfirmations[eventId] = messageId
                }

                emitDiagnostic("debug", "Message sent via Nostr", mapOf(
                    "messageId" to messageId,
                    "eventId" to eventId.take(16) + "...",
                    "contentLength" to eventMessage.length
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
