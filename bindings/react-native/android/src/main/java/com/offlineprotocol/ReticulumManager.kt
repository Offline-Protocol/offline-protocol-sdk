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
    // unblocks an in-flight write here rather than waiting for it — and the
    // same applies to a connect that has not published one yet, which is what
    // [pendingConnectSocket] is for. Both matter more now than they did when
    // this was a per-session thread: that thread was abandoned mid-call and the
    // restart got a fresh one, whereas here the next session's work queues
    // behind whatever is still blocked.
    //
    // "Next session" includes the next *manager*, and that is the one coupling
    // the per-session thread did not have. [TransportConfinement.shared] is
    // keyed by name and process-wide, so two ReticulumManager instances share
    // this looper — reachable on a React Native reload, where the new
    // ReactContext's module can be built before the old one's `invalidate()`
    // runs. Only the *owning* manager's `disconnect` can close its own
    // `pendingConnectSocket`, so a stale instance that is never stopped holds a
    // fresh instance's connect for the rest of CONNECTION_TIMEOUT_MS. Bounded
    // at 60s and self-healing, and `invalidate()`'s stop is what normally
    // prevents it — but it is a real window, and the rule on
    // [TransportConfinement] ("nothing whose worst case is the network may
    // share the thread a stop() waits on") does not cover it: nothing waits
    // here, the cost is queueing behind a stranger.
    private val ioHandler = TransportConfinement.shared(IO_THREAD).handler

    // Message polling runnable — runs on the IO thread
    private val messagePollingRunnable = object : Runnable {
        override fun run() {
            // Returning here also stops the repost, so a paused transport's
            // poll chain terminates itself rather than depending on the
            // cross-looper `removeCallbacks` winning its race. That ordering
            // still holds (see [startMessagePolling]) — this is the belt to
            // its braces, and the only thing that covers a tick already past
            // the removal.
            if (isPaused) return
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

    // The socket a connect attempt is blocked inside, before it has anything
    // to publish. Same lock as the fields above, and for a sharper reason than
    // visibility: closing it is the only way to end a `Socket.connect` early —
    // it has no interruption point, ignores `Thread.interrupt`, and runs for
    // up to CONNECTION_TIMEOUT_MS (60s) against a host dropping SYNs.
    //
    // That is a teardown obligation because [ioHandler] is shared and never
    // quits. A `stop()` does not wait on it, so the stop itself is fine — but
    // the *restart* posts its connect behind the one still blocked, so without
    // this a stop/start cycle against an unreachable daemon stalls for the rest
    // of the timeout. The per-session HandlerThread this replaced never had
    // that problem: it was left blocked and the new session got a new thread.
    private var pendingConnectSocket: Socket? = null

    // Configuration state
    private val isConfigured = AtomicBoolean(false)

    // Connection state
    private val isConnected = AtomicBoolean(false)
    private val isConnecting = AtomicBoolean(false)

    // True between pause() and resume(). Mirrors InternetManager's flag of the
    // same name, and exists for the same reason: stopping the poll timer is not
    // the same as pausing the transport.
    //
    // Two paths re-arm the send loop behind a pause without it. The reconnect
    // edge is the durable one — a daemon that drops and reconnects while the
    // app is backgrounded reaches [handleConnectionOpened], which restarted the
    // 5s poll for the whole background stay. The other is
    // [onMessagesAvailable], the *primary* send path: the timer this manager's
    // pause stops is only the fallback, so a core callback still drained a
    // batch of ten — each one a UniFFI call plus a TCP write — straight
    // through a paused transport.
    //
    // Volatile because it is read on the IO thread — [messagePollingRunnable]'s
    // tick and the block [onMessagesAvailable] posts — and on whichever thread
    // the core calls [onMessagesAvailable] from (its pre-post check), while
    // pause/resume write on the transport thread.
    @Volatile private var isPaused = false
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
    // Checked at every point a retired attempt can still act, because those
    // are separate boundaries rather than one: before publishing the socket,
    // before claiming the flags, inside the announcement block on the
    // transport thread, and — on the other branch — inside
    // [handleConnectionClosed], which is where every teardown that was
    // *observed* on another thread (a failed connect, a dead reader, an
    // exhausted send budget) lands. All three of those hop to the transport
    // thread to get there, so all three can arrive after the session they
    // describe is gone; the check belongs on the far side of the hop, where
    // the flags are actually cleared, not beside each post.
    //
    // The connected-edge state gate cannot stand in for the announcement
    // check. It catches a connection which outlived a stop, but not one which
    // outlived a stop *and* a restart, because by then the state is
    // legitimately STARTING again and the gate passes — which was the
    // announcement firing against a closed socket: the core told the transport
    // was up with `writer` null, and the poll it starts draining the outbox
    // into reticulumSendFailed until the next attempt resolved the flags.
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

        // An explicit start() means "run": a pause() from a previous session
        // must not leave this fresh transport connected-but-mute. Mirrors
        // InternetManager.startUnsafe.
        isPaused = false

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
     * same place.
     *
     * The `removeCallbacks` alone guarantees issuance rather than quiescence,
     * which is why [isPaused] carries the rest. The removal lands on
     * [ioHandler] a hop later (see [startMessagePolling]), and that looper can
     * be inside a connect for up to CONNECTION_TIMEOUT_MS — so a poll tick
     * already queued behind it outlives this call however long it waits.
     * Closing *that* by waiting is not available: it would mean the lifecycle
     * thread blocking on the socket thread, exactly what [ioHandler] exists to
     * prevent. So the tick is allowed to run and made a no-op instead, which is
     * what the flag is for. Same for the two paths a `removeCallbacks` was
     * never going to reach at all — [onMessagesAvailable] and the reconnect
     * edge in [handleConnectionOpened].
     */
    override fun pause() {
        runConfinedSync {
            // Set before the removal is issued, so the tick that outruns the
            // removal still reads it. See the note above.
            isPaused = true
            stopMessagePolling()
        }
    }

    override fun resume() {
        runConfinedSync {
            isPaused = false
            if (state == TransportState.RUNNING && isConnected.get()) {
                // Also drains whatever queued during the pause:
                // [startMessagePolling] runs the runnable immediately rather
                // than waiting out a first interval, and the core does not
                // re-issue [onMessagesAvailable] for messages it already
                // announced.
                startMessagePolling()
            }
        }
    }

    /**
     * Called by the Rust transport callback when new outgoing messages are available.
     * This is the primary send path, replacing timer-based polling.
     *
     * Which is why it carries its own pause check: the timer [pause] stops is
     * the 5s *fallback*, so without this a paused transport still drained a
     * batch of ten per callback — a UniFFI call and a TCP write apiece — for as
     * long as the core kept announcing. The messages are not lost; they stay
     * queued in the core and [resume] drains them.
     */
    fun onMessagesAvailable() {
        if (isPaused) return
        ioHandler.post {
            // Re-read on the IO thread. The check above can be overtaken by a
            // pause() on the transport thread, and this post can additionally
            // sit behind a connect for up to CONNECTION_TIMEOUT_MS — the same
            // window the poll tick has, and the reason the flag rather than a
            // removal is what answers it.
            if (isPaused) return@post
            pollAndSendMessages()
        }
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
            val sock = Socket()
            // Published before the blocking call rather than after it, because
            // this is the only window in which the socket exists and nothing
            // else can reach it — and a teardown needs to reach it to end the
            // connect. See [pendingConnectSocket].
            synchronized(this) { pendingConnectSocket = sock }
            try {
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

                // Second checkpoint, folded into the publish rather than
                // standing beside it: the check and the write have to be one
                // step under the lock that owns these fields, or a teardown
                // landing between them installs this socket over a session
                // that has already been torn down — the field then holds a
                // socket `disconnect` will never see again. Inside the lock,
                // the teardown either wins (this closes what it opened) or
                // loses (it finds the socket published and closes it there).
                var published = false
                synchronized(this) {
                    if (connectGeneration.get() == generation) {
                        socket = sock
                        writer = w
                        reader = r
                        published = true
                    }
                }
                if (!published) {
                    try { sock.close() } catch (_: Exception) {}
                    return@post
                }

                // Send identification
                val identifyMsg = org.json.JSONObject().apply {
                    put("type", "Identify")
                    put("device_id", deviceId)
                }
                w.println(identifyMsg.toString())

                // Third checkpoint, inside [handleConnectionOpened]: sending
                // Identify sits between the publish and the flag claim, so a
                // stop() — or a stop and a restart — fits there too. It
                // refuses rather than claiming, and [startReceiveLoop] is
                // skipped with it: a reader for a retired socket has nothing
                // to read, and starting one against the `isConnected` a
                // teardown just cleared would exit immediately anyway.
                if (!handleConnectionOpened(generation)) return@post
                startReceiveLoop(r, generation)
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
                // Carried into the post rather than checked before it, because
                // a check here would be on the wrong side of the hop — the same
                // reasoning the announcement block below spells out. The
                // handler re-reads it on the transport thread.
                transportHandler.post { handleConnectionClosed(generation, -1, e.message) }
            } finally {
                // By identity, because a teardown may already have closed this
                // socket and moved the field on. Nothing else runs on this
                // looper while this block does, so the check can only ever
                // fail against a null — but the field is the one thing here a
                // *different* thread writes, and a blanket clear would undo it.
                synchronized(this) {
                    if (pendingConnectSocket === sock) pendingConnectSocket = null
                }
            }
        }
    }

    private fun disconnect() {
        synchronized(this) {
            // Clear flags before interrupting the receive thread so it sees
            // isConnected == false and skips the redundant handleConnectionClosed post.
            //
            // Under the lock, and next to the generation bump, because
            // [handleConnectionOpened] claims these same flags from the IO
            // thread. Left outside it they are two unordered writes: a teardown
            // landing between that thread's generation check and its set(true)
            // is simply overwritten, so `isConnected` stays true against a
            // socket this call has already closed. Nothing corrects it until
            // the orphaned reader errors out — and in the meantime connect()'s
            // own `isConnected` guard refuses the next start(), so a restart
            // silently does nothing.
            isConnected.set(false)
            isConnecting.set(false)

            // Any connect still inside Socket.connect on the IO thread belongs
            // to the session this call is ending, so retire its generation: it
            // closes what it opened rather than publishing it over whatever
            // comes next. See [connectGeneration].
            connectGeneration.incrementAndGet()

            receiveThread?.interrupt()
            receiveThread = null
            // Socket first. Socket.close() is the one call that makes a
            // blocked readLine()/println() throw; interrupt() above does not
            // unblock a classic socket read. The wrapped streams share a lock
            // with the I/O they wrap — BufferedReader.close() takes the lock
            // readLine() holds for its whole blocking duration, and
            // PrintWriter.close() the one println() holds — so closing them
            // first waits on the very I/O this teardown exists to interrupt:
            // with the receive thread parked in readLine() (soTimeout = 0,
            // the steady state of a connected transport), it would park this
            // thread — and stop()'s unbounded background wait behind it —
            // until the daemon happened to send a line. Pinned by the
            // Reticulum source guard in the UniFFI crate.
            try { socket?.close() } catch (_: Exception) {}
            try { writer?.close() } catch (_: Exception) {}
            try { reader?.close() } catch (_: Exception) {}
            writer = null
            reader = null
            socket = null

            // The attempt that has not published yet, for the same reason and
            // by the same mechanism: only a close ends a blocked
            // Socket.connect, and leaving one running holds the shared IO
            // looper against the next session's connect. See
            // [pendingConnectSocket].
            try { pendingConnectSocket?.close() } catch (_: Exception) {}
            pendingConnectSocket = null
        }
    }

    /**
     * Promotes this attempt's connection to the current session, or refuses
     * when the attempt was retired while it was completing.
     *
     * The generation check and the flag claim are one step under the lock
     * [disconnect] clears them in, because those two are the only writers and
     * nothing else orders them. Checked outside the lock — as the caller used
     * to do, one statement earlier — the claim is a read-then-write a teardown
     * can land inside: it clears the flags, and this sets `isConnected` back to
     * true against a socket that teardown has already closed. The announcement
     * below still refuses (its own generation check sees the bump), so nothing
     * is announced; what is left is a flag no path clears, and `connect`'s
     * guard reads exactly that flag, so the next `start()` returns having done
     * nothing. It recovers — the orphaned reader errors and the ladder picks it
     * up — but a restart should not have to wait out a backoff rung.
     */
    private fun handleConnectionOpened(generation: Int): Boolean {
        synchronized(this) {
            if (connectGeneration.get() != generation) return false
            isConnected.set(true)
            isConnecting.set(false)
        }
        reconnectAttempts.set(0)
        currentReconnectDelay.set(RECONNECT_INITIAL_DELAY_MS)
        consecutiveSendFailures.set(0)

        emitDiagnostic("info", "Connected to Reticulum daemon")

        // Update state first, then notify protocol, so state is RUNNING before
        // protocol sees the connection event
        transportHandler.post {
            // Re-checked here, and not merely before this block was posted:
            // the two are separated by however long this thread was busy, and
            // a whole stop() plus start() fits in that gap. The state gate
            // below is blind to exactly that pair — a restarted transport is
            // legitimately STARTING, so the gate passes and this announces a
            // socket the stop already closed, with `writer` null. The core is
            // told the transport is up, and the poll this starts drains the
            // outbox straight into reticulumSendFailed. Nothing to clean up
            // here: the generation only moves on when `disconnect` retires it
            // or a fresh `connect` claims it, and both leave the socket this
            // attempt opened to the checkpoint in [connect] that owns it.
            if (connectGeneration.get() != generation) return@post

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

            // Start polling + immediately flush queued messages — unless the
            // app paused the transport. The status flip above stands either
            // way (the daemon really is connected, and DORS needs to know),
            // but the timers do not: this is the durable half of what
            // [isPaused] closes, since a daemon that drops and reconnects
            // during a background stay reaches here and re-armed the 5s poll
            // for the rest of it. Mirrors
            // InternetManager.handleAuthenticated's `if (!isPaused)`.
            if (!isPaused) {
                startMessagePolling()
                ioHandler.post { pollAndSendMessages() }
            }
        }

        return true
    }

    private fun startReceiveLoop(reader: BufferedReader, generation: Int) {
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
            // Connection closed or errored. Stamped with the generation this
            // reader belongs to, because the `isConnected` check below is not
            // one: a teardown clears the flag and wakes this thread, and if a
            // start() lands before it is next scheduled the flag reads true
            // again — belonging to the session that replaced this one. See
            // [handleConnectionClosed].
            if (isConnected.get()) {
                transportHandler.post { handleConnectionClosed(generation, -1, "Connection lost") }
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

    /**
     * The teardown half of [connectGeneration], and the sixth point a retired
     * attempt can still act.
     *
     * Every caller reaches this through `transportHandler.post` from another
     * thread — the IO thread on a failed connect or an exhausted send budget,
     * the receive thread on a dropped link — so each of them names a session
     * that may already be over by the time this runs. Both flags below are
     * *session* state, and clearing them is what makes this destructive: a
     * stale post lands on whatever session holds them now.
     *
     * The `!wasConnected && !wasConnecting` check below cannot stand in for
     * this one, and that is the whole reason the parameter exists. It is a
     * duplicate-suppression check, and it answers "is some session live",
     * never "is it *this* one" — so after a stop() *and* a start() it reads
     * the successor's flags and passes. The stale post then clears
     * `isConnected` under a healthy connection, tells the core the transport
     * is down, and starts a reconnect ladder against it, while the socket that
     * was actually live is left open with its reader parked in `readLine()`
     * (soTimeout = 0) and `receiveThread` already reassigned — nothing ever
     * closes either. The next connect publishes over `socket`/`writer`/
     * `reader` under the lock without closing what it displaces, so the thread
     * and the descriptor leak for the life of the process.
     *
     * Checked before the flags are touched, for the same reason the publish
     * and the flag claim fold their checks into the write rather than standing
     * beside it: the point of a generation is that nothing destructive happens
     * on behalf of a session that has moved on.
     */
    private fun handleConnectionClosed(generation: Int, code: Int, reason: String?) {
        if (connectGeneration.get() != generation) return

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
        // The writer and the generation that owns it, sampled together under
        // the lock `disconnect` clears both in, so the pair is consistent: a
        // teardown either wins and this sees a null writer, or loses and this
        // holds a writer with the generation it belonged to. Read separately
        // they could straddle a stop/start, and the failure teardown below
        // would name the session that replaced this one.
        val w: PrintWriter?
        val generation: Int
        synchronized(this) {
            w = writer
            generation = connectGeneration.get()
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
                transportHandler.post {
                    handleConnectionClosed(generation, -1, "Send failures exceeded threshold")
                }
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
