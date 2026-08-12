package com.offlineprotocol

import android.util.Log
import uniffi.offline_protocol.OfflineProtocol
import java.io.BufferedReader
import java.io.InputStreamReader
import java.io.PrintWriter
import java.net.Socket
import java.util.concurrent.atomic.AtomicBoolean
import java.util.concurrent.atomic.AtomicInteger
import java.util.concurrent.atomic.AtomicLong

/**
 * Reticulum Manager implementing TransportManager for Reticulum daemon communication.
 * Connects to a local Reticulum daemon via TCP for long-range mesh networking
 * (LoRa, serial, I2P, TCP, UDP).
 */
class ReticulumManager(
    private val context: android.content.Context,
    private val protocol: OfflineProtocol,
    private val deviceId: String,
    private val diagnosticEmitter: ((String, String, Map<String, Any?>) -> Unit)? = null
) : TransportManager {

    // MARK: - TransportManager Implementation

    override val transportId = "reticulum"
    override val transportName = "Reticulum (Mesh)"
    @Volatile
    override var state: TransportState = TransportState.UNAVAILABLE
        private set
    override var listener: TransportManagerListener? = null

    companion object {
        private const val TAG = "ReticulumManager"

        private const val MESSAGE_POLL_INTERVAL_MS = 5000L // 5s fallback; primary path is event-driven
        private const val RECONNECT_INITIAL_DELAY_MS = 1000L
        private const val RECONNECT_MAX_DELAY_MS = 30000L
        private const val RECONNECT_BACKOFF_MULTIPLIER = 2.0
        private const val CONNECTION_TIMEOUT_MS = 60000  // 60s — Reticulum paths can be high-latency
        private const val MAX_CONSECUTIVE_FAILURES = 3
        // Name of the private looper this manager confines itself to. Shared
        // process-wide under this key, so a manager rebuilt after stop()
        // inherits the same ordered queue.
        private const val CONFINEMENT_THREAD = "offline-reticulum"
    }

    // MARK: - Properties

    private var daemonHost: String = "localhost"
    private var daemonPort: Int = 4242
    private var autoReconnect = true
    private var maxReconnectAttempts = 0 // 0 = infinite

    // The one thread this manager runs on.
    //
    // This used to be two: a per-session "ReticulumIO" HandlerThread for
    // polling and TCP writes, and the app's main looper for state updates and
    // the status flips. The split is what left OFF-2123 alive here — the poll
    // was already off main, but `reticulumStatusChanged` is a UniFFI call and
    // it ran on main at every connect and disconnect, which for a daemon that
    // is down means once per rung of the 1s→30s backoff ladder.
    //
    // Collapsing both onto one confinement removes the FFI from main and the
    // per-session thread lifecycle with it: nothing has to decide whether
    // `ioHandler` is non-null before posting, and no reconnect can race a
    // quitSafely() that already ran. See [TransportConfinement].
    private val confinement = TransportConfinement.shared(CONFINEMENT_THREAD)
    private val transportHandler = confinement.handler

    // Message polling runnable — runs on the transport thread
    private val messagePollingRunnable = object : Runnable {
        override fun run() {
            pollAndSendMessages()
            if (state == TransportState.RUNNING && isConnected.get()) {
                transportHandler.postDelayed(this, MESSAGE_POLL_INTERVAL_MS)
            }
        }
    }

    // TCP connection
    private var socket: Socket? = null
    private var writer: PrintWriter? = null
    private var reader: BufferedReader? = null
    private var receiveThread: Thread? = null

    // Configuration state
    private val isConfigured = AtomicBoolean(false)

    // Connection state
    private val isConnected = AtomicBoolean(false)
    private val isConnecting = AtomicBoolean(false)
    private var reconnectAttempts = AtomicInteger(0)
    private var currentReconnectDelay = AtomicLong(RECONNECT_INITIAL_DELAY_MS)
    private var reconnectRunnable: Runnable? = null

    // Failure tracking for DORS
    private var consecutiveSendFailures = AtomicInteger(0)

    // MARK: - Helper

    /**
     * Runs [action] on the transport thread and waits for it.
     *
     * The flat 5s bound this replaces applied to every caller, which was the
     * wrong shape twice over: it fired on the RN bridge thread — the one
     * caller that genuinely needs `stop()` to have finished — precisely when
     * the protocol mutex was most contended, leaving the transport half-down
     * and rejecting the JS promise; and it did not protect main at all, since
     * a main-thread caller took the inline fast path straight into the FFI.
     * [TransportConfinement.runSync] inverts both: main is the only bounded
     * caller, and it no longer runs the action itself.
     */
    private fun <T> runConfinedSync(action: () -> T): T = confinement.runSync(action)

    // MARK: - Configuration

    /**
     * Configure the Reticulum daemon connection.
     * @param daemonAddress TCP address in "host:port" format (default: "localhost:4242")
     * @param autoReconnect Whether to auto-reconnect on disconnect (default: true)
     * @param maxReconnectAttempts Max reconnect attempts, 0 = infinite (default: 0)
     */
    fun configure(
        daemonAddress: String = "localhost:4242",
        autoReconnect: Boolean = true,
        maxReconnectAttempts: Int = 0
    ) {
        val parts = daemonAddress.split(":")
        this.daemonHost = parts.getOrElse(0) { "localhost" }
        this.daemonPort = parts.getOrElse(1) { "4242" }.toIntOrNull() ?: 4242
        this.autoReconnect = autoReconnect
        this.maxReconnectAttempts = maxReconnectAttempts
        isConfigured.set(true)

        // Warn when connecting to a non-localhost daemon — the TCP link is unencrypted
        val localhostAliases = setOf("localhost", "127.0.0.1", "::1")
        if (daemonHost !in localhostAliases) {
            Log.w(TAG, "Reticulum daemon is not on localhost ($daemonHost) — TCP connection is unencrypted")
            emitDiagnostic("warning", "Reticulum daemon is not on localhost — TCP connection is unencrypted", mapOf(
                "daemonHost" to daemonHost
            ))
        }

        emitDiagnostic("info", "Reticulum transport configured", mapOf(
            "daemonHost" to daemonHost,
            "daemonPort" to daemonPort,
            "autoReconnect" to autoReconnect,
            "maxReconnectAttempts" to maxReconnectAttempts
        ))
    }

    // MARK: - TransportManager Implementation

    override fun isAvailable(): Boolean {
        // Only report available after configure() has been called, so DORS
        // doesn't select an unconfigured Reticulum transport.
        return isConfigured.get()
    }

    override fun start() {
        runConfinedSync {
            startUnsafe()
        }
    }

    private fun startUnsafe() {
        if (state == TransportState.RUNNING || state == TransportState.STARTING) {
            throw TransportException.AlreadyRunning()
        }

        Log.i(TAG, "Starting Reticulum transport for device: $deviceId")
        emitDiagnostic("info", "Starting Reticulum transport", mapOf(
            "deviceId" to deviceId,
            "daemonAddress" to "$daemonHost:$daemonPort"
        ))

        updateState(TransportState.STARTING)
        connect()
    }

    override fun stop() {
        runConfinedSync {
            stopUnsafe()
        }
    }

    private fun stopUnsafe() {
        if (state != TransportState.RUNNING && state != TransportState.STARTING) {
            return
        }

        updateState(TransportState.STOPPING)

        // Cancel reconnect attempts
        reconnectRunnable?.let { transportHandler.removeCallbacks(it) }
        reconnectRunnable = null

        // Stop timers
        stopMessagePolling()

        // Close TCP connection
        disconnect()

        // The transport thread is process-wide and outlives this stop() — see
        // [TransportConfinement]. Quitting it here is what the per-session
        // thread used to do, and it is exactly what must not happen now: a
        // stop() is followed by start() often enough (enableTransport, a
        // foreground heal) that a dead looper would silently swallow every
        // post the next session makes.

        // Notify protocol
        try {
            protocol.reticulumStatusChanged(false)
        } catch (e: Exception) {
            Log.e(TAG, "Error notifying protocol of disconnect", e)
        }

        updateState(TransportState.STOPPED)
        emitDiagnostic("info", "Reticulum transport stopped")
    }

    override fun pause() {
        transportHandler.post { stopMessagePolling() }
    }

    override fun resume() {
        transportHandler.post {
            if (state == TransportState.RUNNING && isConnected.get()) {
                startMessagePolling()
            }
        }
    }

    /**
     * Called by the Rust transport callback when new outgoing messages are available.
     * This is the primary send path, replacing timer-based polling.
     */
    fun onMessagesAvailable() {
        transportHandler.post { pollAndSendMessages() }
    }

    // MARK: - Connection Management

    private fun connect() {
        if (state == TransportState.STOPPING || state == TransportState.STOPPED) return
        if (isConnecting.get() || isConnected.get()) return
        isConnecting.set(true)

        emitDiagnostic("info", "Connecting to Reticulum daemon", mapOf(
            "host" to daemonHost,
            "port" to daemonPort
        ))

        // Connect on the IO thread to avoid blocking and prevent unmanaged threads
        transportHandler.post {
            try {
                val sock = Socket()
                sock.connect(
                    java.net.InetSocketAddress(daemonHost, daemonPort),
                    CONNECTION_TIMEOUT_MS
                )
                sock.soTimeout = 0 // No read timeout — blocking receive

                val w = PrintWriter(sock.getOutputStream(), true)
                val r = BufferedReader(InputStreamReader(sock.getInputStream()))

                synchronized(this) {
                    socket = sock
                    writer = w
                    reader = r
                }

                // Send identification
                val identifyMsg = org.json.JSONObject().apply {
                    put("type", "Identify")
                    put("device_id", deviceId)
                }
                w.println(identifyMsg.toString())

                handleConnectionOpened()
                startReceiveLoop(r)
            } catch (e: Exception) {
                isConnecting.set(false)
                Log.e(TAG, "Failed to connect to Reticulum daemon", e)
                emitDiagnostic("error", "Failed to connect to Reticulum daemon", mapOf(
                    "error" to (e.message ?: "unknown"),
                    "host" to daemonHost,
                    "port" to daemonPort
                ))
                transportHandler.post { handleConnectionClosed(-1, e.message) }
            }
        }
    }

    private fun disconnect() {
        // Clear flags before interrupting the receive thread so it sees
        // isConnected == false and skips the redundant handleConnectionClosed post.
        isConnected.set(false)
        isConnecting.set(false)

        receiveThread?.interrupt()
        receiveThread = null

        synchronized(this) {
            try { writer?.close() } catch (_: Exception) {}
            try { reader?.close() } catch (_: Exception) {}
            try { socket?.close() } catch (_: Exception) {}
            writer = null
            reader = null
            socket = null
        }
    }

    private fun handleConnectionOpened() {
        isConnected.set(true)
        isConnecting.set(false)
        reconnectAttempts.set(0)
        currentReconnectDelay.set(RECONNECT_INITIAL_DELAY_MS)
        consecutiveSendFailures.set(0)

        emitDiagnostic("info", "Connected to Reticulum daemon")

        // Update state first, then notify protocol, so state is RUNNING before
        // protocol sees the connection event
        transportHandler.post {
            updateState(TransportState.RUNNING)

            // Notify the protocol on the transport thread (consistent with
            // handleConnectionClosed). This is the heaviest FFI call this
            // manager makes: the false→true edge flushes the whole outbox
            // under the global protocol mutex, which is why it must not run
            // on main.
            try {
                protocol.reticulumStatusChanged(true)
            } catch (e: Exception) {
                Log.e(TAG, "Error notifying protocol of connect", e)
            }

            // Start polling + immediately flush queued messages
            startMessagePolling()
            transportHandler.post { pollAndSendMessages() }
        }
    }

    private fun startReceiveLoop(reader: BufferedReader) {
        receiveThread = Thread {
            try {
                while (isConnected.get() && !Thread.currentThread().isInterrupted) {
                    val line = reader.readLine() ?: break
                    processReceivedData(line.toByteArray(Charsets.UTF_8))
                }
            } catch (e: Exception) {
                if (isConnected.get()) {
                    Log.e(TAG, "Receive loop error", e)
                }
            }
            // Connection closed or errored
            if (isConnected.get()) {
                transportHandler.post { handleConnectionClosed(-1, "Connection lost") }
            }
        }.also { it.start() }
    }

    private fun handleConnectionClosed(code: Int, reason: String?) {
        val wasConnected = isConnected.getAndSet(false)
        val wasConnecting = isConnecting.getAndSet(false)

        // Prevent duplicate disconnect handling
        if (!wasConnected && !wasConnecting) return

        // Stop polling immediately on IO thread
        transportHandler.post {
            stopMessagePolling()
        }

        // Notify protocol
        try {
            protocol.reticulumStatusChanged(false)
        } catch (e: Exception) {
            Log.e(TAG, "Error notifying protocol of disconnect", e)
            emitDiagnostic("error", "Failed to notify protocol of disconnection", mapOf(
                "error" to (e.message ?: "unknown")
            ))
        }

        emitDiagnostic("warning", "Reticulum daemon disconnected", mapOf(
            "code" to code,
            "reason" to (reason ?: "none"),
            "wasConnected" to wasConnected
        ))

        // Attempt reconnection if enabled
        if (autoReconnect && state != TransportState.STOPPING && state != TransportState.STOPPED) {
            transportHandler.post { scheduleReconnect() }
        } else {
            transportHandler.post { updateState(TransportState.STOPPED) }
        }
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

        val delay = currentReconnectDelay.get()
        currentReconnectDelay.set(minOf(
            (delay * RECONNECT_BACKOFF_MULTIPLIER).toLong(),
            RECONNECT_MAX_DELAY_MS
        ))

        emitDiagnostic("info", "Scheduling reconnect to Reticulum daemon", mapOf(
            "attempt" to attempts,
            "delayMs" to delay
        ))

        reconnectRunnable?.let { transportHandler.removeCallbacks(it) }
        val runnable = Runnable { connect() }
        reconnectRunnable = runnable
        transportHandler.postDelayed(runnable, delay)
    }

    // MARK: - Message Handling

    private fun processReceivedData(data: ByteArray) {
        val json: org.json.JSONObject
        val messageType: String

        try {
            json = org.json.JSONObject(String(data, Charsets.UTF_8))
            messageType = json.optString("type", "")
        } catch (e: Exception) {
            // Non-JSON data with no sender information — cannot route, skip
            emitDiagnostic("warning", "Received non-JSON data from Reticulum daemon, skipping", mapOf(
                "size" to data.size,
                "error" to (e.message ?: "unknown")
            ))
            return
        }

        when (messageType) {
            "MessageReceived" -> {
                val senderId = json.optString("sender", "")
                val content = json.optString("content", "")
                val encoding = json.optString("encoding", "")

                if (senderId.isEmpty()) {
                    emitDiagnostic("warning", "Invalid MessageReceived: missing sender")
                    return
                }

                try {
                    val messageBytes: ByteArray = if (encoding == "base64") {
                        android.util.Base64.decode(content, android.util.Base64.NO_WRAP)
                    } else {
                        content.toByteArray(Charsets.UTF_8)
                    }

                    protocol.reticulumMessageReceived(
                        senderId,
                        messageBytes.map { it.toUByte() }
                    )

                    emitDiagnostic("debug", "Message received from Reticulum", mapOf(
                        "senderId" to senderId,
                        "contentLength" to content.length
                    ))
                } catch (e: Exception) {
                    emitDiagnostic("error", "Error processing Reticulum message", mapOf(
                        "error" to (e.message ?: "unknown")
                    ))
                }
            }

            "StatusUpdate" -> {
                val daemonStatus = json.optString("status", "unknown")
                emitDiagnostic("debug", "Reticulum daemon status update", mapOf(
                    "status" to daemonStatus
                ))
            }

            else -> {
                emitDiagnostic("debug", "Unknown Reticulum message type", mapOf(
                    "type" to messageType
                ))
            }
        }
    }

    private fun startMessagePolling() {
        stopMessagePolling()
        transportHandler.post(messagePollingRunnable)
    }

    private fun stopMessagePolling() {
        transportHandler.removeCallbacks(messagePollingRunnable)
    }

    private fun pollAndSendMessages() {
        if (!isConnected.get()) return

        try {
            var sent = 0
            val maxBatchSize = 10

            while (sent < maxBatchSize) {
                if (!isConnected.get()) {
                    emitDiagnostic("warning", "Connection lost mid-batch, stopping message send", mapOf(
                        "messagesSent" to sent
                    ))
                    break
                }

                val message = protocol.reticulumGetNextMessage() ?: break
                sendMessage(
                    message.messageId,
                    message.recipientId,
                    message.data.map { it.toByte() }.toByteArray(),
                    message.replyToMsg
                )
                sent++
            }

            if (sent > 1) {
                emitDiagnostic("debug", "Batch sent messages via Reticulum", mapOf(
                    "count" to sent
                ))
            }
        } catch (e: Exception) {
            emitDiagnostic("error", "Error polling Reticulum messages", mapOf(
                "error" to (e.message ?: "unknown")
            ))
        }
    }

    private fun sendMessage(
        messageId: String,
        recipientId: String,
        data: ByteArray,
        replyToMsg: String? = null
    ) {
        val w: PrintWriter?
        synchronized(this) {
            w = writer
        }

        if (!isConnected.get() || w == null) {
            emitDiagnostic("warning", "Cannot send message - not connected", mapOf(
                "messageId" to messageId,
                "recipientId" to recipientId
            ))
            try { protocol.reticulumSendFailed(messageId) } catch (e: Exception) {
                Log.e(TAG, "Failed to report send failure for $messageId", e)
            }
            return
        }

        val content = android.util.Base64.encodeToString(data, android.util.Base64.NO_WRAP)

        val reticulumMessage = org.json.JSONObject().apply {
            put("type", "SendMessage")
            put("recipient", recipientId)
            put("content", content)
            put("encoding", "base64")
            if (replyToMsg != null && replyToMsg.isNotEmpty()) {
                put("reply_to_msg", replyToMsg)
            }
        }

        try {
            val jsonString = reticulumMessage.toString()
            w.println(jsonString)

            if (w.checkError()) {
                throw java.io.IOException("PrintWriter error flag set after write")
            }

            consecutiveSendFailures.set(0)
            try { protocol.reticulumConfirmSent(messageId) } catch (e: Exception) {
                Log.e(TAG, "Failed to confirm send for $messageId", e)
            }

            emitDiagnostic("debug", "Message sent via Reticulum", mapOf(
                "messageId" to messageId,
                "recipientId" to recipientId,
                "contentLength" to content.length
            ))
        } catch (e: Exception) {
            val failures = consecutiveSendFailures.incrementAndGet()
            try { protocol.reticulumSendFailed(messageId) } catch (ex: Exception) {
                Log.e(TAG, "Failed to report send failure for $messageId", ex)
            }
            emitDiagnostic("error", "Failed to send Reticulum message", mapOf(
                "messageId" to messageId,
                "recipientId" to recipientId,
                "consecutiveFailures" to failures,
                "error" to (e.message ?: "unknown")
            ))

            if (failures >= MAX_CONSECUTIVE_FAILURES) {
                emitDiagnostic("warning", "Too many consecutive send failures, triggering reconnect", mapOf(
                    "failures" to failures
                ))
                transportHandler.post { handleConnectionClosed(-1, "Send failures exceeded threshold") }
            }
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
