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
    // Volatile: written on main (updateState) but read from RN threads.
    @Volatile override var state: TransportState = TransportState.UNAVAILABLE
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
        // Keepalive is OkHttp's built-in WebSocket ping/pong (pingInterval on
        // the client builder) — a failed ping surfaces as onFailure and feeds
        // the normal reconnect funnel. 10s (down from 30s) for faster failure
        // detection.
        private const val PING_INTERVAL_MS = 10000L
        private const val CONNECTION_TIMEOUT_MS = 10000L
        private const val MAX_CONSECUTIVE_FAILURES = 2  // Trigger disconnect after 2 consecutive failures
        private const val AUTH_RESPONSE_TIMEOUT_MS = 10_000L
        // Forced presence checks: how long a checkPresence(force = true)
        // may park waiting for authenticated + rate-admitted, and how often
        // the parked queue re-attempts. Mirror the iOS bridge's
        // forcedCheckDeadlineMs / forcedCheckRetryInterval — keep in sync.
        private const val FORCED_CHECK_DEADLINE_MS = 8_000L
        private const val FORCED_CHECK_RETRY_INTERVAL_MS = 500L
        // Tracker ids for app-authored raw SendMessage frames (sendRawCommand):
        // recorded to keep the per-recipient FIFO honest, never reported to
        // the core. Mirrors InternetManager.swift — keep in sync.
        private const val RAW_SEND_SENTINEL_PREFIX = "raw:"
        // Body-only `sender` placeholder for bridge-synthesized relay frames
        // that name no real actor. The Rust `UserId` rejects an empty string,
        // so the serialized Message needs *something*; it must never be handed
        // to the FFI as a senderId. See injectGroupInternalMessage. Mirrors
        // InternetManager.swift — keep in sync.
        private const val RELAY_PLACEHOLDER_SENDER = "relay"
    }
    
    // MARK: - Properties
    
    // Written by configure()/setAuthToken() on the RN bridge thread; read on
    // main (connect/scheduleReconnect) and on the OkHttp reader thread
    // (sendAuthentication after a reconnect's onOpen). First-start flows get
    // happens-before from the start() main-sync, but a mid-session token
    // rotation or reconfigure has no such edge — volatile so a reconnect
    // can't re-authenticate with a stale token.
    @Volatile private var serverUrl: String? = null
    @Volatile private var autoReconnect = true
    @Volatile private var maxReconnectAttempts = 0 // 0 = infinite
    @Volatile private var authToken: String? = null
    
    // OkHttp components
    private var okHttpClient: OkHttpClient? = null
    // Written ONLY on main (connect/disconnect/terminateSocket — every
    // terminal signal posts to main); read from the OkHttp reader thread and
    // RN bridge threads (sendRawCommand, checkPresence). Volatile for those
    // reads; the single-writer rule is what makes terminateSocket's
    // compare-then-detach race-free.
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
    // True between pause() and resume(): a background reconnect must not
    // restart the poll/ping/presence timers the app paused.
    @Volatile private var isPaused = false
    // The relay-displacement latch (close 4000, or a SessionSuperseded notice
    // on the current socket). While latched, auto- AND force-reconnect refuse
    // until an explicit start() — a blind reconnect just re-displaces the peer
    // socket in a tight loop. Owns the boolean + decision; this manager owns
    // the threading (every call on main, the single writer). Mirrors
    // InternetManager.swift's supersedeLatch.
    private val supersedeLatch = SupersededLatchPolicy()
    private var reconnectAttempts = AtomicInteger(0)
    // Atomic for cross-thread reads; written only on main (scheduleReconnect
    // grows it, handleAuthenticated's posted reaction resets it).
    private val currentReconnectDelay =
        java.util.concurrent.atomic.AtomicLong(RECONNECT_INITIAL_DELAY_MS)
    private var reconnectRunnable: Runnable? = null
    // Watchdog for a relay that opens the socket but never answers
    // Authenticate: with isConnected already true, connect() short-circuits
    // and no timer ever starts — a permanently wedged transport without
    // this. Main-thread only.
    private var authTimeoutRunnable: Runnable? = null
    private var transportStartAt: Long = 0L
    
    // Failure tracking for DORS
    private var consecutiveSendFailures = AtomicInteger(0)

    // Correlates the relay's recipient-keyed failure signal (DeliveryError
    // carries no message_id) back to in-flight sends.
    private val inFlightTracker = RecipientInFlightTracker()

    // Which peers to query via CheckPresence, and how many per tick.
    private val presenceWatch = PresenceWatchPolicy()

    // Translates core-tagged server-plane control frames (control_op on
    // InternetMessage) into relay-native ops.
    //
    // Two identities, deliberately: `deviceId` is the profile (the relay
    // username by convention) and matches relay-fed answers, while the lambda
    // resolves the derived address that core-fed roster payloads are named in.
    // Resolved per call, never captured: MLS identity may not exist yet at
    // construction and an identity rebuild replaces the address. See
    // RelayControlOpTranslator's namespace note.
    private val controlOpTranslator = RelayControlOpTranslator(deviceId) {
        try {
            protocol.localAddress()
        } catch (e: Exception) {
            null
        }
    }

    // Client-side mirror of the relay's token bucket: every relay-bound
    // frame takes a token before the socket write (a server-side drop after
    // a "successful" local write is invisible to the sender).
    private val rateLimiter = RelayRateLimiter()

    // Forced presence checks (checkPresence(force = true)): explicit
    // app-driven queries that must survive the chat-open/focus window where
    // the socket is still resuming or the token bucket is momentarily
    // empty. The park/expire/fail-fast/drain policy lives in the Looper-free
    // ForcedPresenceCheckQueue (JVM-tested); only the Handler shell — the
    // retry tick and its no-stacking flag — is here. Main-confined (like the
    // rest of the timer state). Never bypasses the rate limiter — the client
    // bucket mirrors the relay's server bucket, and an over-budget frame is
    // dropped server-side *after* the local write "succeeded", which is
    // strictly worse than deferring.
    private val forcedChecks = ForcedPresenceCheckQueue()
    private var forcedCheckRetryScheduled = false
    private val forcedCheckRetryRunnable = Runnable {
        forcedCheckRetryScheduled = false
        serviceForcedChecks()
    }

    /**
     * Time source for the rate limiter, the in-flight tracker, and the
     * presence watch policy: monotonic (and sleep-inclusive), so a
     * wall-clock step (NTP correction, manual change) can never freeze or
     * over-mint token refill, mass-expire in-flight sends, or evict the
     * whole watch set. Every call into those three must use this — mixing
     * time sources per call site would look like clock jumps to their
     * state. Timestamps that leave the process (conn-request timestamp_ms,
     * relay frames) stay wall-clock.
     */
    private fun monotonicNowMs(): Long = android.os.SystemClock.elapsedRealtime()

    /**
     * Control-op frames deferred by the rate limiter, drained (oldest first)
     * at the start of each poll tick. A translation's commit closure runs
     * only after its LAST frame is written. Main-thread only; cleared on
     * disconnect/stop/RateLimited — the frames are per-connection and their
     * commits are generation-guarded anyway.
     */
    private class PendingControlFrames(
        val controlOp: String,
        val frames: ArrayDeque<org.json.JSONObject>,
        val commit: (() -> Unit)?
    )

    private val pendingControlFrames = ArrayDeque<PendingControlFrames>()

    /**
     * Receives raw relay frames apps need outside or in addition to
     * SDK-owned processing (group snapshot extensions, invite links, role
     * changes, rate limiting, unknown types) — the module forwards them as the
     * `internet_server_message` event.
     */
    var serverMessageEmitter: ((String) -> Unit)? = null

    /**
     * Receives (connected, authenticated) transitions — the module forwards
     * them as the `internet_status_changed` event, the positive readiness
     * signal apps gate raw server commands on (`authenticated: true`
     * replaces polling `sendRawServerCommand` for a non-false return).
     * Deduplicated in [emitConnectionStatus]; every flag flip funnels
     * through it, so the pair is only ever published on actual change.
     */
    var connectionStatusEmitter: ((connected: Boolean, authenticated: Boolean) -> Unit)? = null

    /**
     * Fires once when the relay displaces this connection (close 4000 or a
     * SessionSuperseded notice) — the module forwards it as the
     * `internet_session_superseded` event. The SDK will not auto-reconnect;
     * the app surfaces "connected elsewhere" and reconnects only on explicit
     * user action (re-enabling the transport). Reason is the close/notice
     * reason, if any. Mirrors InternetManager.swift.
     */
    var supersededEmitter: ((reason: String?) -> Unit)? = null

    /**
     * Last (connected, authenticated) pair published, or null before the
     * first. All mutation funnels run on the main handler (or, for
     * [disconnect], are serialized against it by the stop path), matching
     * the single-writer rule the state flags already follow.
     */
    private var lastEmittedStatus: Pair<Boolean, Boolean>? = null

    /**
     * Publishes the current (connected, authenticated) pair when it
     * differs from the last published one — the single choke point for the
     * `internet_status_changed` event, so scattered flag writes cannot
     * double-fire or skip a transition. Call after every flag mutation.
     */
    private fun emitConnectionStatus() {
        val status = Pair(isConnected.get(), isAuthenticated.get())
        if (status == lastEmittedStatus) return
        lastEmittedStatus = status
        connectionStatusEmitter?.invoke(status.first, status.second)
    }

    /**
     * True when the socket is connected AND relay-authenticated — the gate
     * `sendRawCommand` checks. Point-in-time; transitions arrive as
     * `internet_status_changed` events.
     */
    fun isReady(): Boolean = isConnected.get() && isAuthenticated.get()

    /**
     * True while the relay has displaced this session and the transport is
     * latched stopped — it will not reconnect on its own, ever, until an
     * explicit start().
     *
     * The pull half of the supersede contract, and the answer to a question
     * [isReady] structurally cannot resolve: a `false` from an ordinary
     * disconnect (which reconnects itself) and a `false` from a displacement
     * (which never will) are identical there.
     */
    fun isSessionSuperseded(): Boolean = supersedeLatch.isSuperseded

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
        // STARTING is as much "already running" as RUNNING: a manager
        // mid-handshake holds a live OkHttp client and the isConnecting
        // latch, so proceeding would replace the client without shutting it
        // down and then no-op in connect() — the caller must stop() first
        // (enableTransport does).
        if (state == TransportState.RUNNING || state == TransportState.STARTING) {
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

        // An explicit start() means "run": a pause() from a previous
        // session must not leave this fresh transport authenticated-but-mute
        // (e.g. pause → stop → push-triggered enableTransport, which would
        // otherwise skip the poll/ping/presence timers on Authenticated).
        // The reconnect backoff is likewise per-session state: a stale 30s
        // delay must not slow the first retry of a brand-new start.
        isPaused = false
        // A fresh start() is the deliberate re-enable that clears a prior
        // relay-superseded latch: the app has resolved the "connected
        // elsewhere" condition (e.g. signed the other session out) and now
        // wants this device connected again.
        supersedeLatch.clear()
        reconnectAttempts.set(0)
        currentReconnectDelay.set(RECONNECT_INITIAL_DELAY_MS)

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
        // Even a transport that already stopped itself (e.g. after
        // max-reconnect-attempts set STOPPED) still holds an OkHttp client
        // plus per-connection state; stop() must always release those
        // instead of early-returning and leaking the client's threads until
        // process exit.
        val wasActive = state == TransportState.RUNNING || state == TransportState.STARTING

        if (wasActive) {
            updateState(TransportState.STOPPING)
        }

        // Cancel reconnect attempts
        reconnectRunnable?.let { mainHandler.removeCallbacks(it) }
        reconnectRunnable = null

        cancelAuthTimeout()

        // Stop timers
        stopMessagePolling()
        stopPresenceWatch()

        // Close WebSocket
        disconnect()

        // Per-connection state must not survive a stop()/start() cycle:
        // disconnect() detaches the socket before closing it, so the
        // listener's onClosed is suppressed as stale and
        // handleConnectionClosed's clear/reset never runs for this path.
        inFlightTracker.clear()
        controlOpTranslator.reset()
        pendingControlFrames.clear()
        // The watch set survives *reconnects* on purpose (pending traffic is
        // still pending), but an explicit stop() ends the session: without
        // this, a stop/start cycle spends up to the idle TTL of CheckPresence
        // tokens on the previous session's peers.
        presenceWatch.clear()
        // Parked forced presence checks resolve false immediately: an
        // explicit stop() ends the session, and dangling their RN promises
        // until the deadline helps nobody. (A mere disconnect keeps them —
        // the deadline gives the reconnect its chance.)
        mainHandler.removeCallbacks(forcedCheckRetryRunnable)
        forcedCheckRetryScheduled = false
        forcedChecks.drainAll()

        if (wasActive) {
            // Notify protocol
            try {
                protocol.internetStatusChanged(false)
            } catch (e: Exception) {
                Log.e(TAG, "Error notifying protocol of disconnect", e)
            }
        }

        // Shutdown OkHttp
        okHttpClient?.dispatcher?.executorService?.shutdown()
        okHttpClient = null

        if (wasActive) {
            updateState(TransportState.STOPPED)
        }
        emitDiagnostic("info", "Internet transport stopped")
    }
    
    override fun pause() {
        runOnMainSync {
            // The flag makes the pause durable: a background network blip
            // reconnects and re-authenticates, and handleAuthenticated must
            // not restart the timers the app paused.
            isPaused = true
            stopMessagePolling()
            // A backgrounded app must not keep spending battery and relay
            // rate-limit budget on CheckPresence ticks; parked welcomes
            // re-arm from the watch loop after resume().
            stopPresenceWatch()
            // Final drain: flush messages already queued in the Rust queue
            // (still marked Available to DORS) instead of leaving them
            // stranded until resume(). The module pauses the core right
            // after the transports, so the remaining window — a send racing
            // pause() itself — is bounded to sends already in flight.
            // (Safe to run inline here: polling and pause share the main
            // handler, unlike the iOS bridge's messageQueue confinement.)
            if (state == TransportState.RUNNING && isConnected.get()) {
                pollAndSendMessages()
            }
        }
    }

    override fun resume() {
        runOnMainSync {
            isPaused = false
            if (state == TransportState.RUNNING && isConnected.get()) {
                startMessagePolling()
                startPresenceWatch()
            }
        }
    }
    
    /**
     * Forces an immediate teardown + reconnect + re-authenticate of the
     * internet socket, bypassing the exponential backoff. The app calls this
     * on foreground-after-background when the cached ready flags may be stale:
     * a suspend can kill the TCP connection before a clean WS close, leaving
     * [isReady] reporting true against a dead (or relay-deregistered) socket.
     * A liveness probe cannot distinguish either case reliably — only a full
     * reconnect, which re-runs the relay's authenticate/register handshake,
     * heals both — so this is the honest recovery primitive.
     *
     * No-op unless the transport is running/starting (respects the app's
     * enable/disable lifecycle). The actual reconnect honors [autoReconnect];
     * with it disabled this tears the socket down without rebuilding it.
     * Emits a transient `internet_status_changed` down→up.
     */
    fun forceReconnect() {
        runOnMainSync {
            if (state != TransportState.RUNNING && state != TransportState.STARTING) return@runOnMainSync

            // A forced reconnect is a fresh attempt: cancel any pending
            // backoff-scheduled reconnect and reset the backoff so this
            // reconnect (and any that follow it) starts from the initial delay
            // instead of a stale 30s ceiling.
            reconnectRunnable?.let { mainHandler.removeCallbacks(it) }
            reconnectRunnable = null
            currentReconnectDelay.set(RECONNECT_INITIAL_DELAY_MS)
            reconnectAttempts.set(0)

            emitDiagnostic("info", "Force reconnect requested")

            val ws = webSocket
            if (ws != null) {
                // teardownSocket runs the full per-connection cleanup exactly
                // once (stale-socket guarded); with autoReconnect,
                // handleConnectionClosed schedules the reconnect at the reset
                // (initial) delay.
                teardownSocket(ws, "Force reconnect")
            } else {
                // No live socket (e.g. mid-backoff, its pending reconnect just
                // cancelled above): connect immediately. connect() throws on a
                // malformed server URL (reachable if configure() changed it
                // mid-session); a throw here would escape to the RN bridge and
                // reject the promise, so — like scheduleReconnect's posted
                // runnable — a reconnect that cannot even build its request
                // stops the transport instead of surfacing as a rejection.
                try {
                    connect()
                } catch (e: Exception) {
                    emitDiagnostic("error", "Force reconnect failed", mapOf(
                        "error" to (e.message ?: "unknown")
                    ))
                    updateState(TransportState.STOPPED)
                }
            }
        }
    }

    // MARK: - Connection Management
    
    private fun connect() {
        val url = serverUrl ?: return
        // A relay-superseded transport must not reconnect until an explicit
        // start() clears the latch (see markSuperseded).
        if (supersedeLatch.isSuperseded) return
        if (isConnecting.get() || isConnected.get()) return
        // stop() may have run between a reconnect being scheduled and firing.
        // The client null-check must precede the isConnecting latch: with no
        // socket there is no callback to ever clear the flag, and every
        // future connect() would early-return — a wedged transport.
        val client = okHttpClient ?: return

        // The request must build BEFORE the isConnecting latch: url() throws
        // IllegalArgumentException on a malformed app-provided server URL,
        // and a throw after the latch would leave isConnecting=true with no
        // socket callback to ever clear it — every future connect() would
        // early-return, a wedged transport.
        val request = try {
            Request.Builder()
                .url(url)
                .addHeader("X-Device-ID", deviceId)
                .build()
        } catch (e: IllegalArgumentException) {
            emitDiagnostic("error", "Invalid relay server URL", mapOf(
                "url" to url
            ))
            throw TransportException.NotAvailable("Invalid server URL: $url")
        }

        isConnecting.set(true)

        // Invariant: OkHttp dispatches listener callbacks asynchronously
        // (DNS/TCP take milliseconds), so a terminal callback cannot observe
        // `webSocket` before the volatile store below publishes it. If that
        // ever changed, the callback's staleness check would suppress it and
        // `isConnecting` would latch forever (a wedged transport) — keep the
        // assignment immediately after newWebSocket, nothing in between.
        webSocket = client.newWebSocket(request, webSocketListener)

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
        emitConnectionStatus()
    }
    
    private fun handleConnectionOpened(ws: WebSocket) {
        // ONE main-posted block gated on the socket still being current,
        // like handleAuthenticated and terminateSocket: mutating the flags
        // on the reader thread races stopUnsafe() — an onOpen that passed
        // the listener's staleness pre-check, was preempted by disconnect()
        // (webSocket=null, flags cleared), and then resumed would latch
        // isConnected=true with no socket alive to ever clear it, and every
        // future connect() would early-return — a wedged transport.
        mainHandler.post {
            if (ws !== webSocket) return@post
            if (state == TransportState.STOPPING || state == TransportState.STOPPED) return@post

            isConnected.set(true)
            isConnecting.set(false)
            isAuthenticated.set(false)
            emitConnectionStatus()
            // Backoff deliberately NOT reset here: only a full authenticate
            // proves the connection good (handleAuthenticated). Resetting on
            // TCP open would let a persistently bad token cycle
            // connect → AuthError → teardown at the initial 1s delay forever,
            // hammering the relay.
            consecutiveSendFailures.set(0)

            emitDiagnostic("info", "WebSocket connected, authenticating...", mapOf(
                "serverUrl" to (serverUrl ?: "unknown")
            ))

            // A relay that opens the socket but never answers must not wedge
            // the transport (isConnected=true short-circuits connect() and
            // the timers only start on Authenticated).
            scheduleAuthTimeout(ws)

            // Authenticate with the configured auth token (fails closed if unset).
            sendAuthentication()
        }
    }

    /**
     * Arms the auth watchdog for [ws]. Fires through the terminal funnel;
     * cancelled on Authenticated and by any teardown of the socket (the
     * funnel itself cancels it).
     */
    private fun scheduleAuthTimeout(ws: WebSocket) {
        mainHandler.post {
            if (ws !== webSocket) return@post
            cancelAuthTimeout()
            val runnable = object : Runnable {
                override fun run() {
                    if (authTimeoutRunnable === this) authTimeoutRunnable = null
                    if (ws !== webSocket || isAuthenticated.get()) return
                    emitDiagnostic("error", "No auth response from relay within timeout", mapOf(
                        "timeoutMs" to AUTH_RESPONSE_TIMEOUT_MS
                    ))
                    teardownSocket(ws, "Auth response timeout")
                }
            }
            authTimeoutRunnable = runnable
            mainHandler.postDelayed(runnable, AUTH_RESPONSE_TIMEOUT_MS)
        }
    }

    /** Main-thread only. */
    private fun cancelAuthTimeout() {
        authTimeoutRunnable?.let { mainHandler.removeCallbacks(it) }
        authTimeoutRunnable = null
    }
    
    private fun sendAuthentication() {
        val ws = webSocket ?: return

        // Fail closed: only ever present a real auth token (JWT). Never fall back
        // to deviceId — the relay treats the token as the caller's identity, so
        // sending deviceId (== userId == relay username) authenticates as an
        // unverified, forgeable identity (impersonation). Without a token we
        // simply don't authenticate: on the connect path the armed auth watchdog
        // (scheduleAuthTimeout) then tears the un-authenticated socket down; on
        // the setAuthToken(null)-while-connected path any existing session
        // (auth'd under the prior token) is left untouched.
        val token = authToken
        if (token.isNullOrEmpty()) {
            emitDiagnostic("error", "No auth token set; refusing to authenticate with deviceId (forgeable identity). Call setAuthToken with a valid token before connecting.")
            return
        }
        val authMessage = org.json.JSONObject().apply {
            put("type", "Authenticate")
            put("token", token)
        }
        
        val sent = ws.send(authMessage.toString())
        if (sent) {
            // Don't log the token (a secret) or deviceId (not the
            // authenticated identity) — just record that the frame went out.
            emitDiagnostic("debug", "Auth message sent")
        } else {
            emitDiagnostic("error", "Failed to send auth message")
        }
    }
    
    private fun handleAuthenticated(
        ws: WebSocket,
        userId: String,
        username: String?,
        capabilities: List<String>,
        addressChallenge: String?
    ) {
        // The whole reaction is ONE main-posted block gated on the socket
        // still being current and the transport not stopping: reacting on
        // the reader thread would race stopUnsafe() — the core would be told
        // internet is up and route to a transport whose poll loop never
        // starts, and the unguarded RUNNING write would wedge state so the
        // next start() throws AlreadyRunning.
        mainHandler.post {
            if (ws !== webSocket) return@post
            if (state == TransportState.STOPPING || state == TransportState.STOPPED) return@post

            isAuthenticated.set(true)
            emitConnectionStatus()
            // The relay accepted us — this, not the TCP open, is what proves
            // the connection good and earns a backoff reset.
            reconnectAttempts.set(0)
            currentReconnectDelay.set(RECONNECT_INITIAL_DELAY_MS)
            cancelAuthTimeout()

            updateState(TransportState.RUNNING)

            // The address declaration MUST precede both calls below: the relay
            // attributes each inbound frame by whatever this connection has
            // proved at the moment it reads that frame, and never re-stamps
            // retroactively — so a send that leaves before the declaration is
            // attributed by account name for good, and its `Message.sender`
            // (an address) then fails the receiver's
            // `validate_transport_sender`. The status flip below flushes the
            // outbox, which is exactly what produces those sends.
            //
            // Unlike iOS, this whole block runs on main rather than hopping to
            // a serial queue, so the ordering is simply sequential here — and
            // the declaration's two FFI calls land on the main looper next to
            // the far heavier flush that already does. If that flush ever
            // moves off main (the iOS messageQueue shape), this must move with
            // it, ahead of it. Mirrors InternetManager.swift — keep in sync.
            declareAddress(ws, capabilities, addressChallenge, username)

            // Capabilities MUST reach the SDK before the status flip: the
            // false→true transition flushes queued sends, and the group
            // broadcast gate reads the capability set. An older relay omits
            // the field; the empty list is still injected so a stale set
            // from a previous relay can never leak across connections.
            try {
                protocol.internetRelayCapabilities(capabilities)
            } catch (e: Exception) {
                Log.e(TAG, "Error injecting relay capabilities", e)
            }

            // Notify protocol - this will trigger outbox flush for pending messages
            try {
                protocol.internetStatusChanged(true)
            } catch (e: Exception) {
                Log.e(TAG, "Error notifying protocol of connect", e)
            }

            // Start polling and the presence watch — unless the app paused
            // the transport; a background reconnect must stay quiet and
            // resume() restarts the timers. (Keepalive is OkHttp's
            // pingInterval, not a timer of ours.)
            if (!isPaused) {
                startMessagePolling()
                startPresenceWatch()

                // Immediately poll for messages to flush outbox after reconnection
                // This ensures messages queued during disconnection are sent promptly
                pollAndSendMessages()
            }

            // Forced presence checks parked during the reconnect window can
            // go now — even while paused: they are explicit app actions
            // with a bounded deadline, not a recurring timer the pause gate
            // exists for.
            serviceForcedChecks()

            emitDiagnostic("info", "Authenticated with relay server", mapOf(
                "userId" to userId,
                "username" to (username ?: deviceId)
            ))
        }
    }

    /**
     * Proves this connection's `off1…` address to the relay, so it is
     * attributed by address rather than by account name.
     *
     * Called from [handleAuthenticated]'s main-posted block and deliberately
     * the first thing it does: everything the relay attributes downstream —
     * the outbox flush's sends, and anything the poll loop drains after it —
     * is stamped with whatever this connection has proved by the time the relay
     * reads the frame, with no retroactive re-stamping.
     *
     * Nothing waits on the answer. The relay binds the address before it reads
     * the next frame off the socket, so ordering is established by the write
     * alone; `AddressDeclared` and `AddressError` are reported when they arrive
     * but gate nothing.
     *
     * Every failure path is a diagnostic and a return, never a throw: this runs
     * immediately before the capability injection and the status flip, and an
     * undeclared connection is a working connection — the relay simply
     * attributes it the legacy way. Failing the connection over a refused
     * declaration would turn a degraded path into no path.
     */
    private fun declareAddress(
        ws: WebSocket,
        capabilities: List<String>,
        addressChallenge: String?,
        username: String?
    ) {
        val outcome = AddressDeclarationPolicy.decide(capabilities, addressChallenge, username)
        val declaration = when (outcome) {
            is AddressDeclarationPolicy.Outcome.Declare -> outcome
            is AddressDeclarationPolicy.Outcome.Skip -> {
                emitDiagnostic("debug", "Not declaring an address to the relay", mapOf(
                    "reason" to outcome.reason
                ))
                return
            }
        }

        val frame = try {
            val address = protocol.localAddress()
            if (address.isNullOrEmpty()) {
                emitDiagnostic("debug", "Not declaring an address to the relay", mapOf(
                    "reason" to AddressDeclarationPolicy.Reason.ADDRESS_UNAVAILABLE
                ))
                return
            }
            // UniFFI carries `sequence<u8>` as List<UByte> in Kotlin; the
            // policy speaks ByteArray on both platforms.
            val publicKey = protocol.getIdentityPublicKey()
            val signature = protocol.signData(
                AddressDeclarationPolicy.proofPayload(
                    declaration.account,
                    declaration.challenge
                ).map { it.toUByte() }
            )
            AddressDeclarationPolicy.declarationJson(
                address,
                publicKey.map { it.toByte() }.toByteArray(),
                signature.map { it.toByte() }.toByteArray()
            )
        } catch (e: Exception) {
            emitDiagnostic("error", "Could not sign the address declaration", mapOf(
                "reason" to AddressDeclarationPolicy.Reason.SIGNING_FAILED,
                "error" to (e.message ?: e.toString())
            ))
            return
        }

        if (frame == null) {
            emitDiagnostic("error", "Could not build the address declaration", mapOf(
                "reason" to AddressDeclarationPolicy.Reason.FRAME_UNSERIALIZABLE
            ))
            return
        }

        // Unmetered, like the authentication frame this follows: both are
        // one-per-connection handshake frames, and the client limiter's
        // headroom under the relay's own bucket is sized for exactly the two
        // (see RelayRateLimiter). Metering it would let a full bucket defer the
        // one frame every later send's attribution depends on.
        if (!ws.send(frame)) {
            emitDiagnostic("warning", "Address declaration write failed", emptyMap())
        }
    }

    /**
     * The transport's reaction to a dead socket. Only reachable through
     * [terminateSocket]'s detach-first funnel, so it runs on main and at
     * most once per socket — a queued send-failure teardown racing an
     * organic onFailure used to run it twice, double-incrementing the
     * reconnect attempt and double-doubling the backoff.
     */
    private fun handleConnectionClosed(code: Int, reason: String?) {
        val wasConnected = isConnected.getAndSet(false)
        val wasAuthenticated = isAuthenticated.getAndSet(false)
        isConnecting.set(false)
        emitConnectionStatus()

        // Stop polling immediately to prevent sending on dead connection
        stopMessagePolling()
        stopPresenceWatch()
        // Wire outcomes for anything in flight are now owned by the
        // transport layer (fail_all_pending on disconnect).
        inFlightTracker.clear()
        // Registration diffs are per-connection: a reconnect re-registers
        // groups from scratch (sync_groups_to_relay re-sends on the
        // internet 0→1 transition).
        controlOpTranslator.reset()
        // Deferred frames belong to the dead connection; their commits
        // are generation-dead after the reset above.
        pendingControlFrames.clear()

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

        // The relay displaced this connection (close 4000, or a
        // SessionSuperseded notice already flipped the flag on the live
        // socket). A blind reconnect would just re-displace the peer socket in
        // a ~1s eviction loop, so stop for good and let the app decide when to
        // reconnect (explicit user action / foreground with long jitter).
        // Recovery is an explicit start(), which clears isSuperseded.
        //
        // ORDERING NOTE (differs from InternetManager.swift — don't "unify"):
        // this supersede check sits *after* terminateSocket's identity guard,
        // so a 4000 that loses the terminal-signal race to a concurrent local
        // teardown (ping/auth/send-failure, code -1) arrives stale and does not
        // latch this cycle. It self-heals via the relay's *other* displacement
        // signal: the relay always sends an application-level SessionSuperseded
        // notice on the live socket BEFORE close 4000, and that notice (handled
        // race-free while the socket is still current — see the message-dispatch
        // path) latches first. Even absent the notice, the reconnect gets
        // displaced again and the next onClosed(4000) latches. iOS instead marks
        // before its identity guard (so it can't miss the code) but must then
        // exclude a bygone socket by SOCKET GENERATION (SocketGenerationTracker),
        // because its pre-mark path also sees a nil reconnect window that object
        // identity can't classify. Android needs no such generation tracking:
        // dropping non-current sockets here, before the decision, already makes
        // it immune to the bygone-generation false-latch iOS's generation guard
        // fixes. Opposite orderings, same end state.
        //
        // hasNewerSuccessor = false: terminateSocket's identity guard already
        // dropped any close whose socket isn't the current one, so a successor
        // (or any bygone socket) can never reach here (unlike the iOS didClose
        // funnel).
        if (supersedeLatch.shouldMark(code, hasNewerSuccessor = false)) {
            markSuperseded(reason)
            if (state != TransportState.STOPPED) updateState(TransportState.STOPPED)
            return
        }

        // Attempt reconnection if enabled
        // Messages in outbox will be flushed on successful reconnection
        if (autoReconnect && state != TransportState.STOPPING && state != TransportState.STOPPED) {
            scheduleReconnect()
        } else {
            updateState(TransportState.STOPPED)
        }
    }

    /**
     * Marks the connection displaced by the relay and latches it stopped:
     * cancels any pending reconnect, sets [supersedeLatch] so auto- and
     * force-reconnect refuse until the next start(), and fires the one-shot
     * superseded event. Idempotent (via [SupersededLatchPolicy.mark]) — the
     * relay emits both a SessionSuperseded notice and close 4000, so several
     * paths reach here for one displacement. Main-thread only.
     */
    private fun markSuperseded(reason: String?) {
        // The reason goes to the latch, not just the emitter: it has to outlive
        // this call so the report can be re-derived from state (see
        // SupersededLatchPolicy.restatementEventJson) rather than existing only
        // as an argument to an emit that may not land.
        if (!supersedeLatch.mark(reason)) return
        reconnectRunnable?.let { mainHandler.removeCallbacks(it) }
        reconnectRunnable = null
        emitDiagnostic("warning", "Relay superseded this session; not auto-reconnecting", mapOf(
            "reason" to (reason ?: "none")
        ))
        supersededEmitter?.invoke(reason)
    }

    private fun scheduleReconnect() {
        if (!autoReconnect) return
        // Defense in depth: handleConnectionClosed already returns before here
        // on a superseded connection, but never schedule a reconnect for one.
        if (supersedeLatch.isSuperseded) return
        // A close can race stop(): its posted scheduleReconnect must not
        // revive a transport the app already stopped (the delayed connect()
        // would find okHttpClient nulled and leave the transport wedged).
        if (state == TransportState.STOPPING || state == TransportState.STOPPED) return

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
        currentReconnectDelay.set(
            minOf(
                (delay * RECONNECT_BACKOFF_MULTIPLIER).toLong(),
                RECONNECT_MAX_DELAY_MS
            )
        )
        
        emitDiagnostic("info", "Scheduling reconnect", mapOf(
            "attempt" to attempts,
            "delayMs" to delay
        ))
        
        reconnectRunnable?.let { mainHandler.removeCallbacks(it) }
        // connect() throws on a malformed server URL (reachable if configure()
        // changed it mid-session); a throw out of a posted runnable would
        // crash the app, so a reconnect that cannot even build its request
        // stops the transport instead.
        val runnable = Runnable {
            try {
                connect()
            } catch (e: Exception) {
                emitDiagnostic("error", "Reconnect failed", mapOf(
                    "error" to (e.message ?: "unknown")
                ))
                updateState(TransportState.STOPPED)
            }
        }
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
            handleConnectionOpened(webSocket)
        }

        override fun onMessage(webSocket: WebSocket, text: String) {
            if (isStale(webSocket)) return
            processReceivedData(webSocket, text.toByteArray(Charsets.UTF_8))
        }

        override fun onMessage(webSocket: WebSocket, bytes: okio.ByteString) {
            if (isStale(webSocket)) return
            processReceivedData(webSocket, bytes.toByteArray())
        }

        override fun onClosing(webSocket: WebSocket, code: Int, reason: String) {
            webSocket.close(1000, null)
        }

        override fun onClosed(webSocket: WebSocket, code: Int, reason: String) {
            if (isStale(webSocket)) return
            terminateSocket(webSocket, code, reason)
        }

        override fun onFailure(webSocket: WebSocket, t: Throwable, response: Response?) {
            if (isStale(webSocket)) return
            emitDiagnostic("error", "WebSocket connection failed", mapOf(
                "error" to (t.message ?: "unknown"),
                "exception" to t.javaClass.simpleName
            ))
            terminateSocket(webSocket, -1, t.message)
        }
    }

    /**
     * The single terminal funnel: onClosed, onFailure, and every local
     * teardown ([teardownSocket]) end a socket's life here. Detaching [ws]
     * BEFORE the closed handler — on main, the only thread that writes
     * [webSocket] — makes [handleConnectionClosed] unreachable twice for
     * the same socket: whichever signal wins the post detaches it and every
     * later signal fails the identity check. The identity scope also closes
     * the reverse race: a stale path (late AuthError, queued send-failure
     * teardown) can never cancel a newer, healthy socket.
     */
    private fun terminateSocket(ws: WebSocket, code: Int, reason: String?, cancel: Boolean = false) {
        mainHandler.post {
            if (ws !== webSocket) return@post
            webSocket = null
            cancelAuthTimeout()
            if (cancel) ws.cancel()
            handleConnectionClosed(code, reason)
        }
    }

    /**
     * Locally ends [expected]'s life (auth failure, send-failure threshold,
     * auth timeout). Unlike the organic callbacks the socket may still be
     * open, so it is cancelled — after the funnel's detach, which makes the
     * cancel-triggered onFailure/onClosed no-ops (the listener ignores
     * stale sockets).
     */
    private fun teardownSocket(expected: WebSocket, reason: String) {
        terminateSocket(expected, -1, reason, cancel = true)
    }
    
    // MARK: - Message Handling
    
    private fun processReceivedData(ws: WebSocket, data: ByteArray) {
        val rawText = String(data, Charsets.UTF_8)
        val json: org.json.JSONObject
        val messageType: String

        try {
            json = org.json.JSONObject(rawText)
            messageType = json.safeOptString("type")
        } catch (e: Exception) {
            emitDiagnostic("warning", "Received non-JSON or invalid message", mapOf(
                "size" to data.size
            ))
            return
        }

        try {
            dispatchRelayFrame(ws, json, messageType, rawText)
        } catch (e: Exception) {
            // One malformed frame must degrade to a diagnostic, never
            // propagate: an exception escaping OkHttp's onMessage fails the
            // whole WebSocket, turning a single bad field into a
            // transport-wide reconnect cycle.
            emitDiagnostic("error", "Failed to process relay frame", mapOf(
                "type" to messageType,
                "error" to (e.message ?: e.javaClass.simpleName)
            ))
        }
    }

    /**
     * [rawText] is the frame exactly as it arrived; it — not a re-serialized
     * [json] — is what `internet_server_message` forwards, per the TS
     * contract ("the verbatim relay frame"). org.json reorders keys and
     * canonicalizes numbers (25.0 -> 25), the same reason sendRawCommand
     * refuses to re-serialize outbound frames.
     */
    private fun dispatchRelayFrame(
        ws: WebSocket,
        json: org.json.JSONObject,
        messageType: String,
        rawText: String
    ) {
        if (RelayGroupSnapshotBridge.dispatch(
                messageType = messageType,
                json = json,
                rawText = rawText,
                emitTyped = { prefix, payload ->
                    injectGroupInternalMessage(null, prefix, payload)
                },
                emitRaw = { frame -> serverMessageEmitter?.invoke(frame) }
            )
        ) {
            return
        }

        when (messageType) {
            "Authenticated" -> {
                // Handle authentication success
                val userId = json.safeOptString("user_id", deviceId)
                // Kept RAW — no deviceId fallback. This is the account name the
                // relay resolved for the connection, and it is signed into the
                // address proof: the relay verifies against its own copy, so a
                // local substitute would produce a signature that cannot
                // verify. Its only other use is the diagnostic in
                // handleAuthenticated, which falls back there instead.
                val username = json.optNullableString("username")
                // Capability tokens this relay deployment supports (e.g.
                // "group_delivery_v3"). Older relays omit the field → empty.
                val capabilities = json.optJSONArray("capabilities")?.let { arr ->
                    (0 until arr.length()).mapNotNull { i -> arr.optString(i).takeIf { it.isNotEmpty() } }
                } ?: emptyList()
                // Base64 challenge for the optional address declaration,
                // present only on relays advertising `address_routing_v1`.
                val addressChallenge = json.optNullableString("address_challenge")
                handleAuthenticated(ws, userId, username, capabilities, addressChallenge)
            }

            "AddressDeclared" -> {
                // The relay bound this connection to the address we proved.
                // From its next inbound frame on, it attributes us by address
                // instead of account name. Informational: the binding took
                // effect before the relay answered (its frame loop is
                // sequential), so nothing waits on this, and the frame is
                // forwarded like any other.
                //
                // The echo goes to the SDK, which is where the lockstep check
                // lives: it compares the bound address against local_address()
                // and reports a disagreement as
                // RELAY_ADDRESS_BINDING_MISMATCH. A dedicated entry point, not
                // message-plane injection, so an acknowledgement cannot be
                // synthesized through the notification ciphertext injector.
                //
                // No staleness guard here: the listener's onMessage already
                // drops a stale socket's frames before they reach this
                // dispatch, so an answer arriving here belongs to the current
                // connection by construction.
                val declaredAddress = json.safeOptString("address", "")
                protocol.internetAddressDeclared(declaredAddress)
                emitDiagnostic("info", "Relay accepted the address declaration", mapOf(
                    "address" to declaredAddress
                ))
                serverMessageEmitter?.invoke(rawText)
            }

            "AddressError" -> {
                // The declaration was refused. Non-fatal by contract: the
                // connection stays authenticated and keeps working in
                // account-name space, which is exactly how it behaved before
                // addresses existed. No retry here — a refusal is either
                // permanent for this connection (bad material, or a different
                // address already declared) or means this socket was displaced
                // by a newer login, in which case the successor declares for
                // itself. The next reconnect re-declares from scratch.
                //
                // Reported to the SDK as RELAY_ADDRESS_DECLARATION_REFUSED so
                // the degradation is visible to the app: this connection keeps
                // delivering on established sessions but cannot establish new
                // ones, because key-package and welcome frames are identity-
                // checked and ours will not match the account name the relay
                // stamps.
                //
                // Same as above: onMessage's staleness guard means a refusal
                // reaching here is this connection's, not a displaced
                // predecessor's — that socket's frames never get this far.
                val addressErrorReason = json.safeOptString("reason", "Unknown error")
                protocol.internetAddressDeclarationRefused(addressErrorReason)
                emitDiagnostic(
                    "error",
                    "Relay refused the address declaration; staying in account-name space",
                    mapOf("reason" to addressErrorReason)
                )
                serverMessageEmitter?.invoke(rawText)
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
                teardownSocket(ws, reason)
            }

            "SessionSuperseded" -> {
                // The relay is displacing this connection — a newer
                // registration for the same identity took the slot. It also
                // closes with code 4000, but honor the notice too so we never
                // blind-reconnect even if the close code is lost. Run the mark
                // + teardown on main like the other lifecycle funnels; the
                // ws-identity re-check there scopes it to the current socket.
                val supersedeReason = json.optNullableString("reason")
                mainHandler.post {
                    if (ws !== webSocket) return@post
                    markSuperseded(supersedeReason)
                    // Close the live socket through the shared funnel;
                    // handleConnectionClosed sees isSuperseded and stops
                    // without reconnecting (markSuperseded already emitted).
                    teardownSocket(ws, supersedeReason ?: "Session superseded")
                }
            }

            "MessageSent" -> {
                // Handle MessageSent event from WebSocket server
                // This contains the server-generated message_id that we should use
                val messageId = json.optNullableString("message_id")
                val recipient = json.safeOptString("recipient")
                val timestamp = json.safeOptString("timestamp")

                // The relay accepted this frame (forwarded, or FCM-poked an
                // offline recipient) — either way it is no longer in flight
                // and must not be swept into a later recipient-keyed
                // DeliveryError. NOT a delivery signal (the poke case), so
                // the recipient is deliberately not unwatched here.
                if (recipient.isNotEmpty()) {
                    inFlightTracker.resolveOnRelayAccepted(
                        recipient,
                        messageId?.takeIf { it.isNotEmpty() },
                        monotonicNowMs()
                    )
                }

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

                try {
                    // The protocol expects the full serialized Message JSON bytes
                    // The WebSocket server sends the message content, which should be the full Message JSON
                    // (since that's what we sent). However, the server also extracts reply_to_msg and message_id as
                    // separate fields, so we need to ensure they're included in the Message JSON.
                    var messageBytes: ByteArray
                    // The SDK-level content inside the Message, for the
                    // server-plane firewall below.
                    var innerContent: String = content

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
                            innerContent = contentJson.optString("content", "")
                            messageBytes = contentJson.toString().toByteArray(Charsets.UTF_8)
                        } else {
                            // Content is just the message text, reconstruct full Message JSON
                            // (legacy JS-relay senders). LegacyRelayMessage carries every
                            // field the Rust Message deserializer requires — a missing
                            // id/timestamp or non-lowercase priority makes the transport
                            // silently drop the frame.
                            messageBytes = LegacyRelayMessage.buildJson(
                                senderId = senderId,
                                recipientId = deviceId, // Will be corrected by protocol
                                content = content,
                                timestampMs = parseTimestampToMs(timestamp),
                                messageId = messageId,
                                replyToMsg = replyToMsg
                            ).toString().toByteArray(Charsets.UTF_8)
                        }
                    } catch (e: org.json.JSONException) {
                        // Content is not JSON (plain text), reconstruct full Message JSON
                        // (same required-field constraints as above).
                        messageBytes = LegacyRelayMessage.buildJson(
                            senderId = senderId,
                            recipientId = deviceId, // Will be corrected by protocol
                            content = content,
                            timestampMs = parseTimestampToMs(timestamp),
                            messageId = messageId,
                            replyToMsg = replyToMsg
                        ).toString().toByteArray(Charsets.UTF_8)
                    }
                    
                    // Server-plane firewall: peers must never originate
                    // relay-answer frames (__GROUP_CREATED__ & co.). The
                    // relay forwards content verbatim, and the core trusts
                    // these answers from the internet path — one forged
                    // GroupCreated could mark a group relay-synced against a
                    // relay that never registered it. Legitimate answers
                    // enter via injectGroupInternalMessage, not this path.
                    if (RelayControlOpTranslator.isForgedServerPlaneAnswer(innerContent)) {
                        emitDiagnostic("warning", "Dropped peer frame impersonating a relay server answer", mapOf(
                            "senderId" to senderId,
                            "prefix" to innerContent.take(32)
                        ))
                        return
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
                    is Number -> RelayTimestamps.normalizeEpochToMs(rawLastSeen.toLong())
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

            // TypingUpdate is deliberately unhandled: SDK peers send
            // __TYPING__ verbatim as signed SendMessage frames (arriving via
            // MessageReceived), and the relay only produces TypingUpdate
            // from the relay-native SetTyping/ClearTyping frames pre-SDK
            // clients used. The old rebuild injected an unsigned gated
            // control message the core dropped for TOFU-pinned senders
            // anyway.

            // Relay-native Connection* frames (ConnectionRequestReceived /
            // ConnectionAccepted / ConnectionRejected / ConnectionRequestError)
            // are deliberately unhandled: connection ops travel verbatim as
            // signed SendMessage frames and arrive via MessageReceived, so a
            // relay-native connection frame can only come from a pre-SDK client.

            "GroupCreated" -> {
                val groupId = json.safeOptString("group_id")
                val name = json.safeOptString("name")
                if (groupId.isEmpty()) return
                // A success answer on the group channel closes the
                // translator's admin-denial correlation window — without
                // this only errors close it, and it would stay armed for
                // the rest of the connection after a successful register.
                controlOpTranslator.onGroupAnswered(groupId)
                val payloadJson = org.json.JSONObject().apply {
                    put("group_id", groupId)
                    put("name", name)
                }
                injectGroupInternalMessage(null, "__GROUP_CREATED__", payloadJson)
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
                    // Forward attribution, when the relay carries it through.
                    // Core's GroupMessageReceivedPayload has always parsed
                    // this field (`#[serde(default)]`), but no relay populated
                    // it — so a forwarded group message rendered its
                    // attribution over mesh and lost it over relay. Sender
                    // side is the translator's forward_info passthrough; this
                    // is the receiving half.
                    json.optJSONObject("forward_info")
                        ?.takeIf { it.length() > 0 }
                        ?.let { put("forward_info", it) }
                }
                injectGroupInternalMessage(sender.ifEmpty { null }, "__GROUP_MSG__", payloadJson)
            }
            
            "GroupMemberAdded" -> {
                val groupId = json.safeOptString("group_id")
                val userId = json.safeOptString("user_id")
                val addedBy = json.safeOptString("added_by")
                if (groupId.isEmpty()) return
                controlOpTranslator.onGroupAnswered(groupId)
                val payloadJson = org.json.JSONObject().apply {
                    put("group_id", groupId)
                    put("user_id", userId)
                    put("added_by", addedBy)
                }
                // Unattributed: a relay answer must keep the synthesized shape
                // the core's signature-gate exemption recognizes. See
                // injectGroupInternalMessage, INVARIANT 2. `added_by` rides the
                // payload above, which is what the core's handler reads.
                injectGroupInternalMessage(null, "__GROUP_MEMBER_ADDED__", payloadJson)
            }
            
            "GroupMemberRemoved" -> {
                val groupId = json.safeOptString("group_id")
                val userId = json.safeOptString("user_id")
                val removedBy = json.safeOptString("removed_by")
                if (groupId.isEmpty()) return
                controlOpTranslator.onGroupAnswered(groupId)
                val payloadJson = org.json.JSONObject().apply {
                    put("group_id", groupId)
                    put("user_id", userId)
                    put("removed_by", removedBy)
                }
                // Unattributed, as above. Note this frame's admin check reads
                // the wire `sender`, which is the "relay" placeholder, so relay
                // reconciliation cannot pass it — structurally, since an
                // unsignable frame can never authenticate an admin. The
                // functioning path is the removing admin's own signed
                // notification; this one stays inert, and now does so quietly
                // instead of raising a security warning.
                injectGroupInternalMessage(null, "__GROUP_MEMBER_REMOVED__", payloadJson)
            }
            
            "GroupError" -> {
                val reason = json.safeOptString("reason", "Unknown error")
                val groupId = json.safeOptString("group_id")
                // Admin-denied registration must stop member-delta attempts —
                // but only when the error is ours: the request_id (echoed for
                // app raw-channel ops, never tagged by the translator) lets
                // the translator disown errors that answer someone else's
                // frame.
                controlOpTranslator.onGroupError(
                    groupId,
                    reason,
                    json.optNullableString("request_id")
                )
                val payloadJson = org.json.JSONObject().apply {
                    put("reason", reason)
                    // group_id lets the core revoke relay_synced so group
                    // sends fall back to per-member delivery.
                    if (groupId.isNotEmpty()) put("group_id", groupId)
                }
                injectGroupInternalMessage(null, "__GROUP_ERROR__", payloadJson)
                // Dual-emit: apps correlating request_id-carrying errors
                // (invite-link ops ride the raw channel) need the full frame.
                serverMessageEmitter?.invoke(rawText)
            }

            "RateLimited" -> {
                // The relay dropped whatever exceeded the bucket — possibly a
                // member delta whose membership snapshot a commit is about to
                // record. Reset so in-flight commits die (generation guard)
                // and the next register re-derives deltas from scratch; the
                // worst case is an idempotent re-registration. Drain the
                // local bucket too: it was clearly too optimistic.
                //
                // The whole reaction runs as ONE main post so it serializes
                // with poll ticks: resetting the translator here on the
                // reader thread while the clear is still queued would let an
                // interleaved tick translate a post-reset register whose
                // delta frames the clear then wipes (their commits never
                // run, and the group's relay membership stays stale until
                // the next register trigger).
                mainHandler.post {
                    controlOpTranslator.reset()
                    rateLimiter.drain(monotonicNowMs())
                    pendingControlFrames.clear()
                }
                serverMessageEmitter?.invoke(rawText)
                emitDiagnostic("warning", "Relay rate limit hit — translator state reset")
            }

            "GroupRoleChanged" -> {
                // A promotion of this device to admin re-enables member
                // deltas an earlier denial suppressed.
                // The relay names the promoted account in `username` (its
                // group path is username-keyed). `user_id` is kept only as a
                // fallback for a future relay that renames the field —
                // reading it FIRST was the defect: the relay never emits it,
                // so the self-check compared against an empty string and no
                // promotion ever landed.
                controlOpTranslator.onRoleChanged(
                    json.safeOptString("group_id"),
                    json.safeOptString("username", json.safeOptString("user_id")),
                    json.safeOptString("new_role", json.safeOptString("role"))
                )
                serverMessageEmitter?.invoke(rawText)
                emitDiagnostic("debug", "Relay server message forwarded", mapOf(
                    "type" to messageType
                ))
            }

            "GroupMessageSent" -> {
                // The relay's settled per-recipient delivery report for a
                // group broadcast. Forwarded verbatim into the SDK's
                // dedicated report entry point — it correlates by
                // message_id, settles the broadcast tracker, and re-sends
                // per-member copies to members the relay did not reach.
                // Deliberately NOT message-plane injection, so the report
                // path cannot be forged through the notification ciphertext
                // injector. Also passed through as internet_server_message
                // like before, for app observers.
                try {
                    protocol.internetGroupReportReceived(rawText)
                } catch (e: Exception) {
                    emitDiagnostic("warning", "Group delivery report rejected", mapOf(
                        "error" to (e.message ?: e.javaClass.simpleName)
                    ))
                }
                serverMessageEmitter?.invoke(rawText)
                emitDiagnostic("debug", "Group delivery report forwarded", mapOf(
                    "type" to messageType
                ))
            }

            // Server-plane frames that are app concerns, not SDK concerns —
            // forwarded verbatim as the internet_server_message event so the
            // invite-link lifecycle and misc server events can ride the
            // SDK's socket without a second WebSocket in the app.
            "GroupInviteLinkCreated", "GroupInviteLinkRevoked", "GroupJoinedViaInvite",
            "GroupInviteJoinPending", "GroupDeleted" -> {
                serverMessageEmitter?.invoke(rawText)
                emitDiagnostic("debug", "Relay server message forwarded", mapOf(
                    "type" to messageType
                ))
            }

            else -> {
                // Unknown types are forwarded too — future relay additions
                // surface to the app instead of being silently dropped.
                serverMessageEmitter?.invoke(rawText)
                emitDiagnostic("debug", "Received relay message", mapOf(
                    "type" to messageType
                ))
            }
        }
    }

    /**
     * Sends a raw, caller-built relay command verbatim (RN
     * `internetSendRawCommand`). The JSON must parse; returns false when
     * invalid, not connected+authenticated, or deferred by the client-side
     * rate limiter (the caller may retry). Responses the SDK doesn't
     * consume arrive as `internet_server_message` events.
     */
    fun sendRawCommand(json: String): Boolean {
        if (!isConnected.get() || !isAuthenticated.get()) return false
        val ws = webSocket ?: return false
        val parsed = try {
            // Parse purely as validation — the ORIGINAL string is what goes
            // out. Re-serializing (org.json reorders keys and canonicalizes
            // numbers, e.g. 25.0 -> 25) would alter app-authored frames and
            // diverge from iOS, which sends verbatim.
            org.json.JSONObject(json)
        } catch (e: Exception) {
            emitDiagnostic("warning", "Rejected invalid raw server command", mapOf(
                "error" to (e.message ?: "unknown")
            ))
            return false
        }
        if (!rateLimiter.tryAcquire(monotonicNowMs())) return false
        // An app-authored SendMessage joins the same per-recipient FIFO the
        // relay answers in order: without a tracker entry its MessageSent
        // would consume the oldest SDK entry via the oldest-first fallback,
        // costing that message its DeliveryError fail-fast. Sentinel-id
        // entries resolve/drain like any other but are never reported to
        // the core (the app owns raw-frame outcomes) — see
        // handleRecipientUnreachable.
        var sentinel: Pair<String, String>? = null
        if (parsed.optString("type") == "SendMessage") {
            val recipient = parsed.optString("recipient")
            if (recipient.isNotEmpty()) {
                val id = RAW_SEND_SENTINEL_PREFIX + java.util.UUID.randomUUID()
                inFlightTracker.recordSent(recipient, id, monotonicNowMs())
                sentinel = recipient to id
            }
        }
        val sent = ws.send(json)
        if (!sent) {
            rateLimiter.refund()
            // Never written: no relay outcome will ever correlate.
            sentinel?.let { (recipient, id) -> inFlightTracker.unrecord(recipient, id) }
        }
        return sent
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
    private fun buildInternalMessageBytes(senderId: String, content: String): ByteArray =
        LegacyRelayMessage.buildJson(
            senderId = senderId,
            recipientId = deviceId,
            content = content,
            timestampMs = System.currentTimeMillis(),
            // Nothing transmitted this frame, so no sender is awaiting a
            // delivery confirmation — and the core addresses that ACK to the
            // frame's `sender`. See injectGroupInternalMessage.
            requiresAck = false
        ).toString().toByteArray(Charsets.UTF_8)

    /**
     * Inject a group (relay) internal message into the protocol so it emits
     * the corresponding event.
     *
     * INVARIANT: a synthesized frame's identity is either a real
     * relay-reported actor or nothing — never a fabricated string. The FFI
     * `senderId` is a *reachability assertion*: `internet_message_received`
     * routes it into `notify_neighbor_reachable`, so a placeholder there makes
     * the core track it as a live peer (auto key-package DM,
     * NeighborDiscovered, service-discovery fan-out), whose undeliverable DMs
     * then pin it in the presence-watch set forever. Passing `null` selects
     * unattributed ingest, which the core supports and tests explicitly
     * (`test_internet_message_received_empty_sender_*`).
     *
     * The message *body* still needs a non-empty `sender` (Rust `UserId`
     * rejects empty), so it keeps the "relay" placeholder — inert, because no
     * reachability or ACK path acts on it once the two changes above are in
     * place.
     *
     * INVARIANT 2: every prefix in the core's `RELAY_ANSWER_PREFIXES` must be
     * injected with `actorId = null`. Control traffic is signature-gated
     * unconditionally, and nothing here can sign — no peer sent these, so no
     * key exists anywhere in the path. The core exempts them, but the exemption
     * requires the frame to carry *no transport peer identity*, which is what a
     * locally synthesized answer looks like. A non-null `actorId` selects
     * `on_data_received_from`, which sets that identity, so the frame stops
     * looking synthesized and is dropped as unsigned — with a spurious
     * `UNSIGNED_CONTROL_REJECTED` warning raised against a legitimate relay
     * notification.
     *
     * The cost is that these frames no longer assert the actor's reachability.
     * That is the right trade: the frame is the point, reachability was a side
     * effect, and `__GROUP_MSG__` still carries it — being a data-plane prefix
     * it is never gated, so it keeps its attribution. The actor itself is not
     * lost either; it rides the payload (`added_by` / `removed_by`), which is
     * what the core's handlers actually read.
     */
    private fun injectGroupInternalMessage(actorId: String?, prefix: String, payloadJson: org.json.JSONObject) {
        try {
            // INVARIANT 2, enforced rather than trusted to the call sites: a
            // relay answer reaches the core unattributed or it is dropped as
            // unsigned.
            val actor = RelayAnswerPrefixes.attributableActor(prefix, actorId)
            val content = prefix + payloadJson.toString()
            val messageBytes = buildInternalMessageBytes(actor ?: RELAY_PLACEHOLDER_SENDER, content)
            protocol.internetMessageReceived(actor ?: "", messageBytes.map { it.toUByte() })
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

        inFlightTracker.prune(monotonicNowMs())

        // Deferred control frames first: they are older than anything the
        // queue will hand us and their commits are still pending.
        drainPendingControlFrames()

        try {
            // Poll for next message from protocol - batch send up to 10 messages per poll
            // to efficiently flush the outbox after reconnection.
            // Counts this poll's batch only; it bounds the loop and rides the
            // diagnostic below. Not a lifetime total.
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

                // Every relay-bound frame takes a token (see
                // RelayRateLimiter): the poll cadence alone could burst 100
                // frames/s at the relay's 10/s bucket, and over-budget frames
                // are dropped server-side AFTER the local write "succeeded".
                // Out of tokens: leave the rest queued in the core.
                if (!rateLimiter.tryAcquire(monotonicNowMs())) break

                val message = try {
                    protocol.internetGetNextMessage()
                } catch (e: Exception) {
                    // The token was taken for a frame that will never exist —
                    // refund before the error reaches the catch below.
                    rateLimiter.refund()
                    throw e
                }
                if (message == null) {
                    rateLimiter.refund()
                    break
                }
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
            // The poll loop acquired a token for this frame; nothing was
            // written, so return it ("false means defer, never drop").
            rateLimiter.refund()
            // Report failure so DORS metrics stay accurate
            try { protocol.internetSendFailed(messageId) } catch (e: Exception) { Log.e(TAG, "Failed to report send failure for $messageId", e) }
            return
        }

        // Convert data to string content for the relay protocol
        val content = String(data, Charsets.UTF_8)
        
        // Wrap in relay server protocol format
        // reply_to_msg is now provided directly from the Rust SDK via InternetMessage
        //
        // message_id is the core's outbox id, stable across retries of the
        // same logical message. The relay echoes it in MessageReceived /
        // MessageSent / DeliveryError and its push payload, and uses it to
        // suppress duplicate push notifications when an un-ACKed message is
        // retried against a still-offline recipient (a deduped retry comes
        // back as DeliveryError → recipient_unreachable → park, the designed
        // offline path). Older relays ignore the extra field.
        val relayMessage = org.json.JSONObject().apply {
            put("type", "SendMessage")
            put("recipient", recipientId)
            put("content", content)
            put("message_id", messageId)
            if (replyToMsg != null && replyToMsg.isNotEmpty()) {
                put("reply_to_msg", replyToMsg)
            }
        }
        
        val jsonString = relayMessage.toString()
        val sent = ws.send(jsonString)
        
        if (sent) {
            // Reset failure counter on successful send
            consecutiveSendFailures.set(0)
            // Track for recipient-keyed failure correlation: a later
            // DeliveryError for this recipient fails-fast this message id.
            inFlightTracker.recordSent(recipientId, messageId, monotonicNowMs())
            try { protocol.internetConfirmSent(messageId) } catch (e: Exception) { Log.e(TAG, "Failed to confirm send for $messageId", e) }
            
            emitDiagnostic("debug", "Message sent via relay", mapOf(
                "messageId" to messageId,
                "recipientId" to recipientId,
                "contentLength" to content.length
            ))
        } else {
            // Nothing reached the relay's bucket — return the token.
            rateLimiter.refund()
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
                teardownSocket(ws, "Send failures exceeded threshold")
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
                // Every op the core emits should translate to Replace or Tap;
                // PassThrough here means an unknown op (translator behind the
                // Rust registry — see test_internet_control_op_registry_is_closed)
                // or a malformed payload. The frame still ships verbatim as
                // SendMessage (the relay echoes/forwards it without acting),
                // but make the degradation observable instead of silent.
                emitDiagnostic("warning", "Unhandled control op sent verbatim as SendMessage", mapOf(
                    "controlOp" to controlOp,
                    "recipientId" to recipientId
                ))
                sendMessage(messageId, recipientId, data, replyToMsg)
            }

            is RelayControlOpTranslator.Translation.Tap -> {
                // Verbatim delivery owns the message id outcome; the extra
                // relay-native frames are best-effort. The translator's state
                // commits only once every extra frame was written — a dropped
                // frame must be re-sent by a later translation, not assumed
                // applied. Frames the rate limiter defers spill to the next
                // poll tick with the commit still attached.
                sendMessage(messageId, recipientId, data, replyToMsg)
                enqueueControlFrames(controlOp, translation.frames, translation.commit)
            }

            is RelayControlOpTranslator.Translation.Replace -> {
                val ws = webSocket
                if (!isConnected.get() || !isAuthenticated.get() || ws == null) {
                    rateLimiter.refund()
                    try { protocol.internetSendFailed(messageId) } catch (e: Exception) { Log.e(TAG, "Failed to report send failure for $messageId", e) }
                    return
                }
                val primary = translation.frames.firstOrNull()
                if (primary == null) {
                    // Nothing to send (fully deduped) — the intent is already
                    // reflected server-side; confirm so the core moves on.
                    // No frame was written: return the poll loop's token.
                    rateLimiter.refund()
                    translation.commit?.invoke()
                    try { protocol.internetConfirmSent(messageId) } catch (e: Exception) { Log.e(TAG, "Failed to confirm send for $messageId", e) }
                    return
                }
                val primaryJson = primary.toString()
                val sent = ws.send(primaryJson)
                if (sent) {
                    consecutiveSendFailures.set(0)
                    // Group primaries (CreateGroup / SendGroupMessage /
                    // LeaveGroup) are never recorded in the recipient-keyed
                    // in-flight tracker: they answer on the group-scoped
                    // GroupError channel instead — tracked here, one would
                    // absorb a data frame's MessageSent (the oldest-first
                    // fallback) and leave the delivered message for a later
                    // DeliveryError to false-fail.
                    try { protocol.internetConfirmSent(messageId) } catch (e: Exception) { Log.e(TAG, "Failed to confirm send for $messageId", e) }
                    enqueueControlFrames(controlOp, translation.frames.drop(1), translation.commit)
                    emitDiagnostic("debug", "Control op sent relay-native", mapOf(
                        "controlOp" to controlOp,
                        "messageId" to messageId,
                        "frames" to translation.frames.size
                    ))
                } else {
                    // Nothing reached the relay's bucket — return the token.
                    rateLimiter.refund()
                    val failures = consecutiveSendFailures.incrementAndGet()
                    try { protocol.internetSendFailed(messageId) } catch (e: Exception) { Log.e(TAG, "Failed to report send failure for $messageId", e) }
                    emitDiagnostic("error", "Failed to send relay-native control op", mapOf(
                        "controlOp" to controlOp,
                        "messageId" to messageId,
                        "consecutiveFailures" to failures
                    ))
                    if (failures >= MAX_CONSECUTIVE_FAILURES) {
                        teardownSocket(ws, "Send failures exceeded threshold")
                    }
                }
            }
        }
    }

    /**
     * Queues a translation's extra frames for token-gated delivery and
     * drains immediately (the common small case goes out in the same tick).
     * The commit runs only after the translation's last frame is written.
     * Main-thread only.
     */
    private fun enqueueControlFrames(
        controlOp: String,
        frames: List<org.json.JSONObject>,
        commit: (() -> Unit)?
    ) {
        if (frames.isEmpty()) {
            commit?.invoke()
            return
        }
        pendingControlFrames.addLast(
            PendingControlFrames(controlOp, ArrayDeque(frames), commit)
        )
        drainPendingControlFrames()
    }

    /**
     * Sends deferred control frames, oldest first, as tokens allow. A socket
     * write failure drops everything pending: the frames are per-connection,
     * their commits stay uninvoked (and are generation-dead after the
     * disconnect reset), and the reconnect's re-register re-derives them.
     * Main-thread only.
     */
    private fun drainPendingControlFrames() {
        if (pendingControlFrames.isEmpty()) return
        val ws = webSocket ?: return
        while (pendingControlFrames.isNotEmpty()) {
            if (!isConnected.get() || !isAuthenticated.get()) return
            val pending = pendingControlFrames.first()
            while (pending.frames.isNotEmpty()) {
                if (!rateLimiter.tryAcquire(monotonicNowMs())) return
                val frame = pending.frames.first()
                val frameJson = frame.toString()
                if (!ws.send(frameJson)) {
                    rateLimiter.refund()
                    // Deliberately does NOT bump consecutiveSendFailures:
                    // these frames are best-effort deltas, and a dead socket
                    // fails data sends (which do bump it) and fires onFailure
                    // anyway. Keeping the counter data-plane-only avoids a
                    // teardown decision driven by best-effort traffic.
                    emitDiagnostic("warning", "Relay control frame dropped by socket", mapOf(
                        "controlOp" to pending.controlOp,
                        "frameType" to frame.optString("type")
                    ))
                    pendingControlFrames.clear()
                    return
                }
                pending.frames.removeFirst()
            }
            pending.commit?.invoke()
            pendingControlFrames.removeFirst()
        }
    }

    // MARK: - Presence Watch

    /**
     * Fail-fast handler for the relay's recipient-keyed offline signal
     * (DeliveryError). Fails every live in-flight message to the recipient
     * with the recipient_unreachable reason (the core classifies it as
     * per-peer no-carrier and parks welcomes without burning budget),
     * ingests an authoritative offline presence, and adds the recipient to
     * the presence watch set.
     */
    private fun handleRecipientUnreachable(
        recipient: String,
        reason: String,
        source: String
    ) {
        if (recipient.isEmpty()) {
            emitDiagnostic("warning", "Recipient-unreachable signal without recipient", mapOf(
                "source" to source,
                "reason" to reason
            ))
            return
        }
        val now = monotonicNowMs()
        val failedIds = inFlightTracker.drainRecipient(recipient, now)
        for (id in failedIds) {
            // Sentinel entries track app-authored raw SendMessage frames
            // only to keep the per-recipient FIFO honest for MessageSent
            // resolution; their outcomes belong to the app, not the core.
            if (id.startsWith(RAW_SEND_SENTINEL_PREFIX)) continue
            try {
                protocol.internetSendFailedWithReason(id, "recipient_unreachable: $reason")
            } catch (e: Exception) {
                Log.e(TAG, "Failed to fail-fast in-flight message $id", e)
            }
        }
        // Never watch self, and never feed "self is offline" into the core:
        // a malformed self-addressed frame's DeliveryError would otherwise
        // occupy a rotation slot until the idle TTL and could surface a
        // presence_updated(self, offline) to the app. (The core drops self
        // presence too — this just keeps the bridge honest at the source.)
        if (!isSelfPeer(recipient)) {
            presenceWatch.watch(recipient, now)
            try {
                protocol.internetPeerPresence(recipient, false, null)
            } catch (e: Exception) {
                Log.e(TAG, "Failed to ingest offline presence for $recipient", e)
            }
        }
        emitDiagnostic("warning", "Recipient unreachable", mapOf(
            "recipient" to recipient,
            "reason" to reason,
            "source" to source,
            "failedInFlight" to failedIds.size
        ))
    }

    /**
     * True when [peerId] names this device in either namespace — the profile
     * (relay username by convention) or the derived address the core stamps
     * on its own frames. Peer ids reaching the presence plane come from both
     * sides, and a profile-only test lets self through on every core-fed one.
     */
    private fun isSelfPeer(peerId: String): Boolean {
        if (peerId.isEmpty()) return false
        if (peerId == deviceId) return true
        return try {
            peerId == protocol.localAddress()
        } catch (e: Exception) {
            false
        }
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
            val now = monotonicNowMs()
            // Self is filtered BEFORE the merge so it can never enter the
            // watch set and pin a rotation slot until the idle TTL.
            // Resolved once per tick rather than per entry: the watchlist
            // comes from the core in address space, so a profile-only filter
            // lets self into the watch set.
            // Empty folds to null so an absent address never matches an
            // empty watchlist entry.
            val selfAddress = try {
                protocol.localAddress()?.takeIf { it.isNotEmpty() }
            } catch (e: Exception) {
                null
            }
            val peers = presenceWatch.peersToQuery(
                coreWatchlist.filter { it != deviceId && it != selfAddress },
                now
            )
            var queried = 0
            for (peer in peers) {
                // Presence queries yield to data traffic under rate
                // pressure; skipped peers come around on a later rotation.
                if (!rateLimiter.tryAcquire(monotonicNowMs())) break
                val checkMessage = org.json.JSONObject().apply {
                    put("type", "CheckPresence")
                    put("username", peer)
                }
                if (!ws.send(checkMessage.toString())) {
                    rateLimiter.refund()
                    break
                }
                queried++
            }
            if (queried > 0) {
                emitDiagnostic("debug", "Presence watch tick", mapOf(
                    "queried" to queried,
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
     * One-shot CheckPresence. Non-forced calls fail fast (`false`) when the
     * socket isn't authenticated+connected or the token bucket is
     * momentarily empty. `force` exists for the chat-open/focus window (the
     * socket is often still resuming exactly when the app wants a fresh
     * header): the query is parked and retried until authenticated and
     * rate-admitted, up to [FORCED_CHECK_DEADLINE_MS], then resolves false —
     * except on a stopping/stopped transport, where no reconnect is coming
     * and even forced calls fail fast. Forced checks stay one-shot — they
     * never join the watch set. Mirrors the iOS bridge's
     * checkPresence(userId:force:completion:) — keep in sync.
     */
    fun checkPresence(userId: String, force: Boolean, callback: (Boolean) -> Unit) {
        if (userId.isEmpty()) {
            callback(false)
            return
        }
        if (!force) {
            callback(sendPresenceCheckNow(userId))
            return
        }
        val deadlineMs = monotonicNowMs() + FORCED_CHECK_DEADLINE_MS
        mainHandler.post {
            attemptForcedCheck(ForcedPresenceCheckQueue.Entry(userId, deadlineMs, callback))
        }
    }

    /**
     * Admits and writes one CheckPresence frame. Returns true only when the
     * frame was admitted and OkHttp accepted the enqueue.
     */
    private fun sendPresenceCheckNow(userId: String): Boolean {
        if (!isConnected.get() || !isAuthenticated.get()) return false
        val ws = webSocket ?: return false
        if (!rateLimiter.tryAcquire(monotonicNowMs())) return false
        val checkMessage = org.json.JSONObject().apply {
            put("type", "CheckPresence")
            put("username", userId)
        }
        val sent = ws.send(checkMessage.toString())
        if (!sent) rateLimiter.refund()
        return sent
    }

    /**
     * Main-confined. Sends the forced check if currently admissible;
     * otherwise the queue policy parks it, expires it, fails it fast on a
     * stopping/stopped transport (no reconnect coming), or rejects it at
     * capacity.
     */
    private fun attemptForcedCheck(check: ForcedPresenceCheckQueue.Entry) {
        if (sendPresenceCheckNow(check.userId)) {
            check.callback(true)
            return
        }
        val stopped = state == TransportState.STOPPING || state == TransportState.STOPPED
        if (forcedChecks.parkOrExpire(check, stopped, monotonicNowMs())) {
            scheduleForcedCheckRetry()
        }
    }

    /**
     * Main-confined. One retry tick services the whole queue; the scheduled
     * flag keeps ticks from stacking.
     */
    private fun scheduleForcedCheckRetry() {
        if (forcedChecks.isEmpty || forcedCheckRetryScheduled) return
        forcedCheckRetryScheduled = true
        mainHandler.postDelayed(forcedCheckRetryRunnable, FORCED_CHECK_RETRY_INTERVAL_MS)
    }

    /**
     * Main-confined. Re-attempts every parked forced check;
     * attemptForcedCheck re-parks the still-unsendable ones.
     */
    private fun serviceForcedChecks() {
        for (check in forcedChecks.takeAll()) {
            attemptForcedCheck(check)
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
