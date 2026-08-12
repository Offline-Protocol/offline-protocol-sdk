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
        // Name of the second looper, for blocking socket work only.
        private const val IO_THREAD = "offline-reticulum-io"
    }

    // MARK: - Properties

    // Written by configure() on the RN bridge thread; read on the transport
    // thread (connect, scheduleReconnect) and on the IO thread (the blocking
    // connect itself, which reads the host and port it dials). The first
    // start() gets happens-before from runConfinedSync's post, but a
    // mid-session reconfigure has no such edge — volatile so a reconnect
    // cannot dial a stale host or apply a retired retry policy.
    // InternetManager annotates serverUrl/authToken for exactly this reason;
    // these mirrors were missed when it did.
    @Volatile private var daemonHost: String = "localhost"
    @Volatile private var daemonPort: Int = 4242
    @Volatile private var autoReconnect = true
    @Volatile private var maxReconnectAttempts = 0 // 0 = infinite

    // The thread this manager's state, lifecycle and FFI run on.
    //
    // This used to be the app's main looper, which is what left OFF-2123 alive
    // here — the poll was already off main, but `reticulumStatusChanged` is a
    // UniFFI call and it ran on main at every connect and disconnect, which
    // for a daemon that is down means once per rung of the 1s→30s backoff
    // ladder. See [TransportConfinement].
    //
    // Lifecycle callers block on this thread ([runConfinedSync]), and a
    // background caller waits without a bound on purpose, so nothing posted
    // here may block for longer than the protocol mutex does — which is why
    // the socket work lives on [ioHandler] and not here.
    private val confinement = TransportConfinement.shared(CONFINEMENT_THREAD)
    private val transportHandler = confinement.handler

    // The thread the blocking socket work runs on: connecting (up to
    // CONNECTION_TIMEOUT_MS), and the TCP writes, which have no timeout at all
    // — a daemon that stops reading blocks a write until the socket is closed.
    //
    // Separate from [confinement] because those durations are bounded by the
    // network rather than by us, and `stop()` waits on that thread unbounded:
    // sharing one would let an unreachable host park a stop() (and the RN
    // bridge thread behind it) for a minute, or a wedged daemon park it
    // indefinitely. Both are loopers rather than the old per-session
    // HandlerThread, so neither can be quit out from under a reconnect.
    //
    // Ordered, not pooled: this is one TCP stream and its writes must stay in
    // order. `stop()` closes the socket from [confinement], which is what
    // unblocks an in-flight write here rather than waiting for it.
    private val ioHandler = TransportConfinement.shared(IO_THREAD).handler

    // Message polling runnable — runs on the IO thread
    private val messagePollingRunnable = object : Runnable {
        override fun run() {
            pollAndSendMessages()
            if (state == TransportState.RUNNING && isConnected.get()) {
                ioHandler.postDelayed(this, MESSAGE_POLL_INTERVAL_MS)
            }
        }
    }

    // TCP connection
    private var socket: Socket? = null
    private var writer: PrintWriter? = null
    private var reader: BufferedReader? = null
    // Under the same `synchronized(this)` as the socket it reads, because it
    // crosses the same two threads: [startReceiveLoop] publishes it from the
    // IO thread, [disconnect] interrupts and clears it from the transport
    // thread. It was the one field in this group left outside the lock.
    private var receiveThread: Thread? = null

    // Configuration state
    private val isConfigured = AtomicBoolean(false)

    // Connection state
    private val isConnected = AtomicBoolean(false)
    private val isConnecting = AtomicBoolean(false)
    private var reconnectAttempts = AtomicInteger(0)
    private var currentReconnectDelay = AtomicLong(RECONNECT_INITIAL_DELAY_MS)
    private var reconnectRunnable: Runnable? = null

    // Which connect attempt is the current one.
    //
    // [connect] stamps this before handing its blocking work to [ioHandler],
    // and [disconnect] retires it, so a block that outlives the attempt that
    // queued it can tell. The flags above cannot answer that question:
    // `disconnect` clears `isConnecting` while the connect is still inside
    // `Socket.connect` — up to CONNECTION_TIMEOUT_MS against a host dropping
    // SYNs — so a stop() followed by a start() passes connect()'s guard and
    // queues a *second* attempt behind the first on the one shared IO looper.
    // Both would publish, and the loser's socket and receive thread would leak
    // with `isConnected` still true, so when that orphaned reader finally
    // errored it would tear down the session that had replaced it.
    //
    // Checked before publishing rather than folded into the connected-edge
    // gate on the transport thread: that gate catches a connection which
    // outlived a stop, but not one which outlived a stop *and* a restart,
    // because by then the state is legitimately STARTING again.
    private val connectGeneration = AtomicInteger(0)

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

    /**
     * Synchronous like every other lifecycle entry point here, and unlike the
     * fire-and-forget post this replaces.
     *
     * `OfflineProtocolModule.pause` pauses the five transports and then the
     * core, and that order is the point — it is why [InternetManager.pause]
     * can call its final drain "bounded to sends already in flight". A posted
     * pause returns before it has done anything, so the core could pause
     * first and this manager's next poll tick would re-enter UniFFI behind it.
     * BLE, Internet and Wi-Fi Direct all confine-and-wait here; these two were
     * the outliers, left over from when the post was the only way onto their
     * old per-session IO thread.
     *
     * Costs nothing extra: the module's caller is React Native's
     * native-modules thread, so the wait is the unbounded background one
     * ([TransportConfinement.runSync]) that `stop()` already takes from the
     * same place. The actual `removeCallbacks` still lands on [ioHandler] a
     * hop later — see [startMessagePolling] — which is unchanged and correct;
     * what is now guaranteed is that the cancel has been *issued* before the
     * core is paused.
     */
    override fun pause() {
        runConfinedSync { stopMessagePolling() }
    }

    override fun resume() {
        runConfinedSync {
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
        ioHandler.post { pollAndSendMessages() }
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

        val generation = connectGeneration.incrementAndGet()

        // Connect on the IO thread to avoid blocking and prevent unmanaged threads
        ioHandler.post {
            try {
                val sock = Socket()
                sock.connect(
                    java.net.InetSocketAddress(daemonHost, daemonPort),
                    CONNECTION_TIMEOUT_MS
                )

                // A teardown, or a restart, happened while this attempt was
                // still inside connect(). Nothing below is wanted: publishing
                // now would install this socket over the current session's,
                // stranding one of the two with a live reader thread. Close
                // what this opened instead — see [connectGeneration].
                if (connectGeneration.get() != generation) {
                    try { sock.close() } catch (_: Exception) {}
                    return@post
                }

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
                Log.e(TAG, "Failed to connect to Reticulum daemon", e)
                emitDiagnostic("error", "Failed to connect to Reticulum daemon", mapOf(
                    "error" to (e.message ?: "unknown"),
                    "host" to daemonHost,
                    "port" to daemonPort
                ))

                // `isConnecting` is deliberately NOT cleared here, and that is
                // the fix rather than an oversight. [handleConnectionClosed]
                // decides whether a failure is worth reacting to by reading
                // exactly these two flags — clearing this one first made that
                // read false on both counts, so it returned early and never
                // reached [scheduleReconnect]. The whole 1s→30s ladder was
                // dead for the case it exists for: a daemon that is not
                // running. The transport simply sat in STARTING, where
                // startUnsafe throws AlreadyRunning, until an explicit
                // stop()/start(). iOS clears the flag inside that handler,
                // under the same lock that samples it, which is why the ladder
                // worked there and not here.
                //
                // A superseded attempt hands nothing over: the generation that
                // replaced it owns the flags now, and reporting this failure
                // would fire the ladder against a session that already moved on.
                if (connectGeneration.get() == generation) {
                    transportHandler.post { handleConnectionClosed(-1, e.message) }
                }
            }
        }
    }

    private fun disconnect() {
        // Clear flags before interrupting the receive thread so it sees
        // isConnected == false and skips the redundant handleConnectionClosed post.
        isConnected.set(false)
        isConnecting.set(false)

        // Any connect still inside Socket.connect on the IO thread belongs to
        // the session this call is ending, so retire its generation: it closes
        // what it opened rather than publishing it over whatever comes next.
        // See [connectGeneration].
        connectGeneration.incrementAndGet()

        synchronized(this) {
            receiveThread?.interrupt()
            receiveThread = null
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
            // A stop() that landed while this connection was still being
            // established has already told the protocol we are down and moved
            // to STOPPED. Announcing the connection now would put the state
            // back to RUNNING and the protocol back to connected, against a
            // transport nothing will ever tear down again — and the next
            // start() would throw AlreadyRunning off it. The socket this
            // opened is stray, so close it here, on the thread that owns
            // disconnect().
            if (state == TransportState.STOPPING || state == TransportState.STOPPED) {
                disconnect()
                return@post
            }

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
            ioHandler.post { pollAndSendMessages() }
        }
    }

    private fun startReceiveLoop(reader: BufferedReader) {
        val thread = Thread {
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
        }
        // Published under the socket's lock, because this runs on the IO
        // thread while [disconnect] clears the field from the transport one.
        // Assigned before start() rather than after, so a teardown landing in
        // between finds the thread instead of a null: interrupting one that
        // has not started is a no-op, and the loop it then enters reads the
        // `isConnected` that teardown already cleared and exits immediately.
        synchronized(this) { receiveThread = thread }
        thread.start()
    }

    private fun handleConnectionClosed(code: Int, reason: String?) {
        val wasConnected = isConnected.getAndSet(false)
        val wasConnecting = isConnecting.getAndSet(false)

        // Prevent duplicate disconnect handling
        if (!wasConnected && !wasConnecting) return

        // Stop polling. Called directly rather than posted: every caller of
        // this reaches it through `transportHandler.post`, so a post here only
        // re-queues the cancel behind whatever else is waiting on this thread.
        // The hop that matters is inside [stopMessagePolling], which puts the
        // removal on the IO thread that owns the runnable.
        stopMessagePolling()

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

    /**
     * Both halves hop onto the IO thread before touching its queue, rather
     * than reaching across from the lifecycle thread.
     *
     * Every caller of these runs on [confinement]; [messagePollingRunnable]
     * runs — and reposts itself — on [ioHandler]. A cross-thread
     * `removeCallbacks` removes nothing while the runnable is mid-flight, so
     * it loses the race to the repost and polling survives the call. Two of
     * the three callers happen to be covered anyway: `stopUnsafe` and
     * `handleConnectionClosed` both clear `isConnected` first, and the
     * repost gate reads it. [pause] clears nothing, so it was the one that
     * could leave a paused transport polling — calling the FFI and writing to
     * the daemon for the whole background stay — and a [resume] landing on the
     * same race would stack a second runnable and double the rate.
     *
     * Posting instead queues the removal *behind* an in-flight runnable's
     * repost, so it always wins. Ordering across the two loopers is safe for
     * free: the callers are serialised on the lifecycle thread, so the IO
     * thread receives these in the order they were issued.
     */
    private fun startMessagePolling() {
        ioHandler.post {
            ioHandler.removeCallbacks(messagePollingRunnable)
            messagePollingRunnable.run()
        }
    }

    private fun stopMessagePolling() {
        ioHandler.post { ioHandler.removeCallbacks(messagePollingRunnable) }
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
