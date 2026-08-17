package com.offlineprotocol

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
import java.util.concurrent.ConcurrentHashMap
import java.util.concurrent.TimeUnit
import java.util.concurrent.atomic.AtomicBoolean
import java.util.concurrent.atomic.AtomicInteger
import java.util.concurrent.atomic.AtomicLong

/**
 * Nostr Manager implementing TransportManager for Nostr relay communication.
 * Connects to Nostr relays via WebSocket and uses NIP-04 (kind 4) direct messages
 * for protocol message routing.
 */
/**
 * The profile is deliberately not a constructor parameter.
 *
 * This manager is built at configure time, before the protocol has an
 * identity, so any id passed in could only be the app-chosen profile — and
 * the one thing this transport must never carry is a value a username can be
 * recomputed from. The address is read from the protocol at [start], which is
 * the first moment it is both known and needed.
 */
class NostrManager(
    private val context: android.content.Context,
    private val protocol: OfflineProtocol,
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

        // How long a query waits for stragglers before it is finished anyway.
        // End-of-stored-events is the only completion signal a Nostr query has
        // and a relay is free never to send one. Bounded well below the
        // engine's own resolution sweep, so the ordinary answer still comes
        // from here. Mirrors the iOS manager's QUERY_COMPLETION_TIMEOUT.
        private const val QUERY_COMPLETION_TIMEOUT_MS = 10_000L
        // Name of the private looper this manager confines itself to. Shared
        // process-wide under this key, so a manager rebuilt after stop()
        // inherits the same ordered queue.
        private const val CONFINEMENT_THREAD = "offline-nostr"
    }

    // MARK: - Properties

    // Written by configure() on the RN bridge thread, read on the transport
    // thread (connectToRelay, scheduleReconnect) and on OkHttp's reader
    // threads. The first start() gets happens-before from runConfinedSync's
    // post, but a mid-session reconfigure has no such edge — volatile so a
    // reconnect cannot dial a retired relay set or apply a retired retry
    // policy. InternetManager annotates serverUrl/authToken for exactly this
    // reason; these mirrors were missed when it did.
    @Volatile private var relayUrls: List<String> = emptyList()
    @Volatile private var autoReconnect = true
    @Volatile private var maxReconnectAttempts = 0 // 0 = infinite

    // The one thread this manager runs on.
    //
    // This used to be two: a per-session "NostrIO" HandlerThread for polling,
    // and the app's main looper for lifecycle and the connect/disconnect
    // status flips. The poll was already off main, but `nostrStatusChanged`
    // and the `nostrQueryCompleted` release loop are UniFFI calls and they ran
    // on main at every relay transition — which, for a flapping relay set, is
    // the reconnect backoff cadence. See [TransportConfinement].
    private val confinement = TransportConfinement.shared(CONFINEMENT_THREAD)
    private val transportHandler = confinement.handler

    // Message polling runnable
    private val messagePollingRunnable = object : Runnable {
        override fun run() {
            // Returning here also stops the repost, so a paused transport's
            // poll chain terminates itself. Belt to `removeCallbacks`' braces:
            // pause and this runnable share the transport thread, so the
            // removal is already exact — but the reconnect edge can start a
            // fresh chain, and this is what stops that one too.
            if (isPaused) return
            pollAndSendMessages()
            pollAndSendQueries()
            expireStaleQueries()
            if (state == TransportState.RUNNING && isConnected.get()) {
                transportHandler.postDelayed(this, MESSAGE_POLL_INTERVAL_MS)
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

    // Nostr identity (obtained from Rust core). Unlike the configuration
    // above this has a second writer, and it is the one that earns the
    // annotation: configure() seeds it on the RN bridge thread, but
    // sendSubscription() re-reads it from the core on OkHttp's reader thread
    // at every relay (re)connect — deliberately, because the signing key
    // rotates when MLS initialization installs the persisted secret and the
    // fresh value has to be in place before any event can arrive on the
    // subscription being opened. Read on the transport thread and, in
    // processNostrMessage, on the reader thread again for the self-event
    // filter. Volatile so that reader-thread write is visible to all of them.
    @Volatile private var publicKeyHex: String = ""

    // Configuration state
    private val isConfigured = AtomicBoolean(false)

    // Connection state
    private val isConnected = AtomicBoolean(false)

    // True between pause() and resume(). Mirrors InternetManager's flag of the
    // same name, and exists for the same reason: stopping the poll timer is not
    // the same as pausing the transport.
    //
    // Two paths re-arm the send loop behind a pause without it. The reconnect
    // edge is the durable one — a relay that drops and reconnects while the app
    // is backgrounded runs [updateConnectionStatus]'s connected branch, which
    // restarted a 100ms poll for the whole background stay. The other is
    // [onMessagesAvailable], the *primary* send path: the timer this manager's
    // pause stops is only the fallback, so a core callback still drained a
    // batch of ten straight through a paused transport.
    //
    // Volatile because [onMessagesAvailable] arrives on whichever thread the
    // core calls it from, while pause/resume write on the transport thread.
    @Volatile private var isPaused = false
    // Concurrent, not plain: handleRelayConnected reads these on OkHttp's
    // reader thread while scheduleReconnect's getOrPut structurally modifies
    // them on the transport thread. A plain HashMap read racing a resize is
    // the classic corruption/endless-probe case — the atomics inside the
    // values only cover the counter, not the map that holds it.
    private val reconnectAttempts = ConcurrentHashMap<String, AtomicInteger>()
    private val currentReconnectDelay = ConcurrentHashMap<String, AtomicLong>()
    private val reconnectRunnables = ConcurrentHashMap<String, Runnable>()

    // Pending relay confirmations: Nostr event_id → protocol message_id.
    // Populated when a WebSocket send succeeds; removed on relay ["OK", ...].
    private val pendingEventConfirmations = mutableMapOf<String, String>()

    /**
     * A resolution query the platform is running, and which relays still owe it
     * an end-of-stored-events.
     *
     * A query is broadcast, so every connected relay answers it under the same
     * subscription id and each sends its own EOSE. Ending the query on the
     * *first* one makes the answer whatever the fastest relay happened to hold,
     * and for a username resolution that answer is the entire result: a relay
     * holding nothing, or holding only a squatter's claim, would decide what
     * the user sees while every other relay served the honest claimants. A
     * claim is supposed to need only one honest relay to survive.
     *
     * Tracking who is still owed is what lets each relay's subscription close
     * as soon as *that* relay is done, which keeps the "no standing filter on a
     * routing tag" property, without ending the query for the others.
     */
    private data class QueryProgress(
        /** Relays that have not yet sent end-of-stored-events. */
        val awaiting: MutableSet<String>,
        /** When the query was issued, bounding how long a silent relay holds it. */
        val issuedAtMs: Long
    )

    // In-flight resolution queries, as opposed to the standing message
    // subscription. Events arriving under one of these are records fetched on
    // the transport's behalf, not inbound messages, and go to a different entry
    // point. Guarded by relayLock.
    private val activeQueries = mutableMapOf<String, QueryProgress>()
    private val pendingEventLock = Object()

    // Failure tracking for DORS
    private val consecutiveSendFailures = AtomicInteger(0)

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
        runConfinedSync {
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

        // Refused without an identity, and refused *here* rather than at the
        // one call site that enables the transport: start() is the single
        // point that opens a socket, and the first thing a socket does is
        // publish this device's routing tag to a third-party relay.
        //
        // That tag is derived from the address. With no identity there is no
        // address — the core installs no Nostr transport at all — so starting
        // anyway would open relay connections that can never subscribe, which
        // is indistinguishable from "nobody is talking to us".
        val address = protocol.localAddress()
        if (address.isNullOrEmpty()) {
            throw TransportException.NotAvailable(
                "Nostr requires the protocol identity. Initialize MLS (or leave encryption enabled) before enabling Nostr."
            )
        }

        // An identity is necessary but not sufficient. The core registers the
        // Nostr transport during the identity rebuild and only when Nostr was
        // enabled in the config create() received, so enableTransport("nostr")
        // against a config that had it off arrives here with an address and no
        // transport behind it.
        //
        // Probed with the subscription filter because that is the exact call the
        // socket-open callback makes, and a null answer *there* is not a safe
        // failure: it is logged and returned from with no retry, so the socket
        // stays connected to a relay it never subscribes on for the rest of its
        // life. Refusing before any socket exists is the recoverable shape.
        if (protocol.nostrGetSubscriptionFilter("startup-probe") == null) {
            throw TransportException.NotAvailable(
                "No Nostr transport is registered. Enable Nostr in the protocol configuration before starting it."
            )
        }

        // An explicit start() means "run": a pause() from a previous session
        // must not leave this fresh transport connected-but-mute. Mirrors
        // InternetManager.startUnsafe.
        isPaused = false

        Log.i(TAG, "Starting Nostr transport for address: $address")
        emitDiagnostic("info", "Starting Nostr transport", mapOf(
            "address" to address,
            "relayCount" to relayUrls.size,
            "publicKey" to publicKeyHex
        ))

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
        runConfinedSync {
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
            transportHandler.removeCallbacks(runnable)
        }
        reconnectRunnables.clear()

        // Stop timers
        stopMessagePolling()

        // Close all WebSocket connections
        disconnectAll()

        // Shut down OkHttp's dispatcher like InternetManager does — start()
        // builds a fresh client, so a stop that leaves the executor alive
        // leaks its threads once per stop()/start() cycle.
        okHttpClient?.dispatcher?.executorService?.shutdown()
        okHttpClient = null

        // The transport thread is process-wide and outlives this stop() — see
        // [TransportConfinement]. Quitting it here is what the per-session
        // thread used to do, and it is exactly what must not happen now: a
        // stop() is followed by start() often enough (enableTransport, a
        // foreground heal) that a dead looper would silently swallow every
        // post the next session makes.

        // Notify protocol
        try {
            protocol.nostrStatusChanged(false)
        } catch (e: Exception) {
            Log.e(TAG, "Error notifying protocol of disconnect", e)
        }

        updateState(TransportState.STOPPED)
        emitDiagnostic("info", "Nostr transport stopped")
    }

    /**
     * Synchronous like the other lifecycle entry points — see
     * [ReticulumManager.pause] for the argument. In short: the module pauses
     * the transports and then the core, and a posted pause returns before it
     * has stopped anything, so the poll could re-enter UniFFI after the core
     * was already paused.
     */
    override fun pause() {
        runConfinedSync {
            // Set before the timer is stopped, and read by both paths that
            // would otherwise re-arm the send loop behind this call — see
            // [isPaused]. Stopping the timer alone paused the fallback and
            // left the primary path running.
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
     *
     * This is the *primary* send path — the timer [pause] stops is the 100ms
     * fallback — so it carries the pause check itself. Without it a paused
     * transport still drained a batch of ten per callback, each one taking the
     * core's global protocol mutex, for as long as the core kept announcing.
     * The messages are not lost: they stay queued in the core and [resume]
     * drains them.
     */
    fun onMessagesAvailable() {
        if (isPaused) return
        transportHandler.post {
            // Re-read on the transport thread: pause() writes there, so a
            // callback that passed the check above can still be overtaken by
            // the pause it raced.
            if (isPaused) return@post
            pollAndSendMessages()
        }
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
                processNostrMessage(text, relayUrl)
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

        // This relay owes no more end-of-stored-events, so stop waiting on it.
        // A query whose every other relay has already answered finishes here
        // rather than waiting out the timeout.
        dropRelayFromQueries(relayUrl)

        emitDiagnostic("warning", "Nostr relay disconnected", mapOf(
            "relayUrl" to relayUrl,
            "reason" to (reason ?: "none"),
            "wasConnected" to wasConnected
        ))

        // Update overall connection status
        updateConnectionStatus()

        // Attempt reconnection if enabled
        if (autoReconnect && state != TransportState.STOPPING && state != TransportState.STOPPED) {
            transportHandler.post { scheduleReconnect(relayUrl) }
        }
    }

    private fun updateConnectionStatus() {
        // Sampled and published as one step under the lock that owns the map.
        //
        // Read and swapped separately — as these were — two relays
        // transitioning at once can interleave so that the *later* swap
        // publishes the *earlier* reader's answer. A relay connects and this
        // reads `anyConnected = true`, then is descheduled; the same relay
        // drops, and that call reads false, swaps false→false and sees
        // `wasConnected = false`, so neither edge fires; the first thread
        // resumes and swaps false→true with `wasConnected = false`, firing the
        // connected edge against a relay set that is empty. The core is told
        // the transport is up, polling starts, and every message it drains hits
        // the no-connected-relays branch of [publishMessage] and comes back as
        // nostrSendFailed until a relay genuinely reconnects.
        //
        // Under one lock the second caller always samples the final map, and
        // the two edges are enqueued in the order they were swapped.
        val anyConnected: Boolean
        val wasConnected: Boolean
        synchronized(relayLock) {
            anyConnected = relayConnected.values.any { it }
            wasConnected = isConnected.getAndSet(anyConnected)
        }

        if (anyConnected && !wasConnected) {
            // Became connected
            consecutiveSendFailures.set(0)
            transportHandler.post {
                // A stop() that landed while this relay's handshake was still
                // in flight has already told the protocol we are down and
                // moved to STOPPED. Announcing the connection now would put
                // the state back to RUNNING and the protocol back to
                // connected, against a transport nothing will ever tear down
                // again — and the next start() would throw AlreadyRunning off
                // it. The relay socket is stray, so close it here, on the
                // thread that owns disconnectAll().
                if (state == TransportState.STOPPING || state == TransportState.STOPPED) {
                    disconnectAll()
                    return@post
                }

                updateState(TransportState.RUNNING)
                try {
                    protocol.nostrStatusChanged(true)
                } catch (e: Exception) {
                    Log.e(TAG, "Error notifying protocol of connect", e)
                }
                // The status flip stands even while paused — the transport
                // really is up, and DORS needs to know — but the timers do
                // not. This is the durable half of what [isPaused] closes: a
                // relay that drops and reconnects during a background stay
                // reaches here, and restarting the 100ms poll from it re-armed
                // the loop the app paused, for the rest of the stay. Mirrors
                // InternetManager.handleAuthenticated's `if (!isPaused)`.
                if (!isPaused) {
                    startMessagePolling()
                    transportHandler.post { pollAndSendMessages() }
                }
            }
        } else if (!anyConnected && wasConnected) {
            // Lost all connections
            transportHandler.post {
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

        reconnectRunnables[relayUrl]?.let { transportHandler.removeCallbacks(it) }
        val runnable = Runnable { connectToRelay(relayUrl) }
        reconnectRunnables[relayUrl] = runnable
        transportHandler.postDelayed(runnable, delay)
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

    /**
     * Handles one relay frame.
     *
     * Takes the relay it arrived from, because an EOSE is a statement by *that
     * relay* about a broadcast query rather than about the query as a whole.
     */
    private fun processNostrMessage(text: String, relayUrl: String) {
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
                // means *this relay* has given us everything it holds. The
                // query is done when every relay it was sent to has said so,
                // not when the first one has.
                val eoseSubId = if (json.length() >= 2) json.optString(1, "") else ""
                if (eoseSubId.isNotEmpty() && isActiveQuery(eoseSubId)) {
                    noteEndOfStoredEvents(eoseSubId, relayUrl)
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
        synchronized(relayLock) { activeQueries.containsKey(subscriptionId) }

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

                // Recorded against the relays this REQ actually goes to, so a
                // relay that connects later is not waited on for an answer it
                // was never asked for.
                synchronized(relayLock) {
                    activeQueries[query.queryId] = QueryProgress(
                        awaiting = connectedRelays.keys.toMutableSet(),
                        issuedAtMs = monotonicNowMs()
                    )
                }

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

    /** Monotonic clock, unaffected by wall-clock jumps. */
    private fun monotonicNowMs(): Long = android.os.SystemClock.elapsedRealtime()

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
            val ids = activeQueries.keys.toList()
            activeQueries.clear()
            ids
        }
        if (queryIds.isEmpty()) return

        for (queryId in queryIds) {
            releaseQuery(queryId)
        }

        emitDiagnostic("debug", "Released in-flight Nostr resolution queries", mapOf(
            "count" to queryIds.size
        ))
    }

    /**
     * Records one relay's end-of-stored-events for a query.
     *
     * Closes *that relay's* subscription immediately, because it has nothing
     * further to send and a filter left open on a routing tag is the standing
     * signal this design avoids. The query itself finishes only once every
     * relay it was sent to has answered: ending it on the first EOSE would
     * discard the records the slower relays are still sending, and for a
     * username resolution those records are the answer.
     */
    private fun noteEndOfStoredEvents(subscriptionId: String, relayUrl: String) {
        val closeMessage = JSONArray().put("CLOSE").put(subscriptionId).toString()
        synchronized(relayLock) { relayWebSockets[relayUrl] }?.send(closeMessage)

        val finished = synchronized(relayLock) {
            val progress = activeQueries[subscriptionId]
            when {
                progress == null -> false
                else -> {
                    progress.awaiting.remove(relayUrl)
                    if (progress.awaiting.isEmpty()) {
                        activeQueries.remove(subscriptionId)
                        true
                    } else {
                        false
                    }
                }
            }
        }

        if (finished) releaseQuery(subscriptionId)
    }

    /**
     * Stops waiting on a relay that went away, for every query it owed.
     *
     * A disconnected relay will never send its EOSE, so without this the last
     * query it was asked would wait out the timeout instead of finishing as
     * soon as the relays that *can* answer have.
     */
    private fun dropRelayFromQueries(relayUrl: String) {
        val finished = synchronized(relayLock) {
            val done = mutableListOf<String>()
            // Over a snapshot, so the removals below cannot interact with the
            // walk.
            for ((subscriptionId, progress) in activeQueries.entries.toList()) {
                if (!progress.awaiting.remove(relayUrl)) continue
                if (progress.awaiting.isEmpty()) {
                    activeQueries.remove(subscriptionId)
                    done.add(subscriptionId)
                }
            }
            done
        }

        for (subscriptionId in finished) releaseQuery(subscriptionId)
    }

    /**
     * Finishes queries whose relays never sent end-of-stored-events.
     *
     * Runs on the poll loop. A relay is free never to send EOSE, and without a
     * deadline such a query holds its subscription for the life of the
     * connection while its caller waits on the engine's much later sweep.
     */
    private fun expireStaleQueries() {
        val cutoff = monotonicNowMs() - QUERY_COMPLETION_TIMEOUT_MS
        val stale = synchronized(relayLock) {
            activeQueries.filterValues { it.issuedAtMs < cutoff }.keys.toList()
        }

        for (subscriptionId in stale) {
            emitDiagnostic("debug", "Nostr query timed out waiting for end-of-stored-events", mapOf(
                "queryId" to subscriptionId
            ))
            finishQuery(subscriptionId)
        }
    }

    /** Ends a query now, whatever the relays have or have not sent. */
    private fun finishQuery(subscriptionId: String) {
        val progress = synchronized(relayLock) { activeQueries.remove(subscriptionId) } ?: return

        val closeMessage = JSONArray().put("CLOSE").put(subscriptionId).toString()
        val sockets = synchronized(relayLock) {
            progress.awaiting.mapNotNull { relayWebSockets[it] }
        }
        for (socket in sockets) {
            socket.send(closeMessage)
        }

        releaseQuery(subscriptionId)
    }

    /** Hands a finished query back to the transport. */
    private fun releaseQuery(subscriptionId: String) {
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
