package com.offlineprotocol

import android.os.Handler
import android.os.HandlerThread
import android.os.Looper
import android.util.Log
import uniffi.offline_protocol.OfflineProtocol
import java.io.BufferedReader
import java.io.InputStreamReader
import java.io.PrintWriter
import java.net.Socket
import java.util.concurrent.CountDownLatch
import java.util.concurrent.TimeUnit
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
    }

    // MARK: - Properties

    private var daemonHost: String = "localhost"
    private var daemonPort: Int = 4242
    private var autoReconnect = true
    private var maxReconnectAttempts = 0 // 0 = infinite

    // Handler for main thread operations (state updates, listener callbacks)
    private val mainHandler = Handler(Looper.getMainLooper())

    // Background handler thread for message polling and TCP I/O
    private var ioThread: HandlerThread? = null
    private var ioHandler: Handler? = null

    // Message polling runnable — runs on ioHandler (background thread)
    private val messagePollingRunnable = object : Runnable {
        override fun run() {
            pollAndSendMessages()
            if (state == TransportState.RUNNING && isConnected.get()) {
                ioHandler?.postDelayed(this, MESSAGE_POLL_INTERVAL_MS)
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
        runOnMainSync {
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

        // Start background IO thread for polling and TCP writes
        val thread = HandlerThread("ReticulumIO").also { it.start() }
        ioThread = thread
        ioHandler = Handler(thread.looper)

        updateState(TransportState.STARTING)
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

        // Close TCP connection
        disconnect()

        // Shut down IO thread
        ioThread?.quitSafely()
        ioThread = null
        ioHandler = null

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
        ioHandler?.post { stopMessagePolling() }
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
     * This is the primary send path, replacing timer-based polling.
     */
    fun onMessagesAvailable() {
        ioHandler?.post { pollAndSendMessages() }
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
        ioHandler?.post {
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
                mainHandler.post { handleConnectionClosed(-1, e.message) }
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
        mainHandler.post {
            updateState(TransportState.RUNNING)

            // Notify protocol on main thread (consistent with handleConnectionClosed)
            try {
                protocol.reticulumStatusChanged(true)
            } catch (e: Exception) {
                Log.e(TAG, "Error notifying protocol of connect", e)
            }

            // Start polling + immediately flush queued messages on IO thread
            startMessagePolling()
            ioHandler?.post { pollAndSendMessages() }
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
                mainHandler.post { handleConnectionClosed(-1, "Connection lost") }
            }
        }.also { it.start() }
    }

    private fun handleConnectionClosed(code: Int, reason: String?) {
        val wasConnected = isConnected.getAndSet(false)
        val wasConnecting = isConnecting.getAndSet(false)

        // Prevent duplicate disconnect handling
        if (!wasConnected && !wasConnecting) return

        // Stop polling immediately on IO thread
        ioHandler?.post {
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
            mainHandler.post { scheduleReconnect() }
        } else {
            mainHandler.post { updateState(TransportState.STOPPED) }
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

        reconnectRunnable?.let { mainHandler.removeCallbacks(it) }
        val runnable = Runnable { connect() }
        reconnectRunnable = runnable
        mainHandler.postDelayed(runnable, delay)
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
                mainHandler.post { handleConnectionClosed(-1, "Send failures exceeded threshold") }
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
