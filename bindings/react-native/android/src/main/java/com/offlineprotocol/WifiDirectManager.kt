package com.offlineprotocol

import android.Manifest
import android.annotation.SuppressLint
import android.content.BroadcastReceiver
import android.content.Context
import android.content.Intent
import android.content.IntentFilter
import android.content.pm.PackageManager
import android.net.wifi.WifiManager
import android.net.wifi.p2p.*
import android.os.Build
import android.os.Handler
import android.util.Log
import androidx.core.content.ContextCompat
import uniffi.offline_protocol.OfflineProtocol
import uniffi.offline_protocol.verifyIdentityAssertion
import java.io.*
import java.net.InetSocketAddress
import java.net.ServerSocket
import java.net.Socket
import java.util.concurrent.*
import java.util.concurrent.atomic.AtomicBoolean
import java.util.concurrent.atomic.AtomicLong

/**
 * WiFi Direct Manager implementing TransportManager for WiFi P2P communication
 *
 * ## Every socket proves its peer before it carries anything
 *
 * The engine registers this transport as its peer-stream slot, and
 * `docs/spec/stream-framing.md` is the contract. Each socket, accepted by the
 * group owner or opened by a client toward it, sends this device's identity
 * assertion as its first frame and reads the peer's. The peer's assertion is
 * checked by `verifyIdentityAssertion`, the one verifier every platform uses,
 * and the address it derives is the only id this manager ever hands the core:
 * to `wifiDirectPeerConnected`, as the `senderId` of each body, and to
 * `wifiDirectPeerDisconnected`. A socket endpoint, a device name or a MAC
 * never reaches the core. They did once, and a TCP endpoint in `known_peers`
 * evicted real neighbours and started key exchanges toward a string.
 *
 * What the chapter asks of a receiver lives in [PeerStreamSockets], which
 * this manager hands each socket and each outbound body: the preamble under
 * one deadline, the frame bounds (the ceiling is inclusive, and a refused
 * length closes the socket rather than reading on; the reader this replaced
 * got both wrong), one announced stream per address with the newer
 * superseding, a per-stream writer, and the ordering that keeps a delivery
 * from landing after a loss report. It is framework-free and tested on real
 * loopback sockets. What stays here is the group: the listener, the one
 * outbound socket a client opens to its owner, and reconnecting that socket
 * while the group lasts.
 *
 * What this manager does not do: form a group. Nothing here calls
 * `WifiP2pManager.connect`, so a group comes from the system's Wi-Fi Direct
 * settings or from another app, and this manager joins the socket layer when
 * `WIFI_P2P_CONNECTION_CHANGED_ACTION` says one exists.
 */
class WifiDirectManager(
    private val context: Context,
    private val protocol: OfflineProtocol,
    private val deviceId: String,
    private val diagnosticEmitter: ((String, String, Map<String, Any?>) -> Unit)? = null
) : TransportManager {

    // MARK: - TransportManager Implementation
    
    override val transportId = "wifi_direct"
    override val transportName = "WiFi Direct (P2P)"
    // Volatile: written on the transport thread (updateState) but read from
    // RN threads and the socket executor. The other three managers already
    // declared this; this one did not, and the drain and poll both branch on
    // it from a different thread than the lifecycle that writes it.
    @Volatile override var state: TransportState = TransportState.UNAVAILABLE
        private set
    override var listener: TransportManagerListener? = null

    companion object {
        private const val TAG = "WifiDirectManager"
        
        // Fallback interval for message polling. Primary send path is event-driven
        // via onMessagesAvailable(); this timer only catches edge cases.
        private const val MESSAGE_POLL_INTERVAL_MS = 2000L
        private const val CONNECTION_TIMEOUT_MS = 30000L
        private const val SERVER_PORT = 8988
        // Name of the private looper this manager confines itself to. Shared
        // process-wide under this key, so a manager rebuilt after stop()
        // inherits the same ordered queue.
        private const val CONFINEMENT_THREAD = "offline-wifidirect"
        // Messages one drain pass sends before yielding the transport looper
        // back to the framework callbacks that share it — see
        // [drainAndSendMessages]. A fairness budget, not a limit: the pass
        // reposts itself when it spends the budget with the queue non-empty.
        private const val MAX_DRAIN_BATCH = 32
        private const val SOCKET_TIMEOUT_MS = 5000
        // Reconnect delays for a client whose stream to the group owner ended
        // while the group is still up. Doubled after an attempt that proved
        // no peer, reset by one that did.
        private const val RECONNECT_INITIAL_DELAY_MS = 1_000L
        private const val RECONNECT_MAX_DELAY_MS = 60_000L
    }

    // MARK: - Properties
    
    private val wifiP2pManager: WifiP2pManager? = 
        context.getSystemService(Context.WIFI_P2P_SERVICE) as? WifiP2pManager
    private var channel: WifiP2pManager.Channel? = null
    
    // The one thread this manager runs on.
    //
    // This used to be the app's main looper, which put two UniFFI paths on it:
    // the 2s fallback poll, and the event-driven drain, whose `while (true)`
    // takes the global protocol mutex once per queued message with no batch
    // bound. The Wi-Fi P2P framework callbacks landed there too, since the
    // channel was initialized with the main looper and the broadcast receiver
    // registered without a scheduler — so a WIFI_P2P_STATE_CHANGED broadcast
    // called into the protocol on the main thread as well. All of it moves
    // here; see [TransportConfinement] for why a private looper is the same
    // ordering primitive without the ANR exposure.
    private val confinement = TransportConfinement.shared(CONFINEMENT_THREAD)
    private val transportHandler = confinement.handler
    
    // Executor for socket operations
    private val socketExecutor = Executors.newCachedThreadPool()
    
    // Server socket for receiving connections. Volatile because it crosses
    // two threads with no lock: the socket executor binds and publishes it,
    // and stopServerSocket() closes and nulls it from the transport thread —
    // the close is what unblocks a parked accept(), so a stale read there
    // skips the one call that ends the loop early.
    @Volatile private var serverSocket: ServerSocket? = null
    private var serverRunning = AtomicBoolean(false)
    
    // Every socket from its first byte to its end: the preamble, the framing,
    // one announced stream per address, the writers. This manager owns the
    // group and the connect; see [PeerStreamSockets] for the rest.
    private val sockets = PeerStreamSockets(object : PeerStreamSockets.Host {
        override fun identityAssertion(): ByteArray =
            protocol.identityAssertion(emptyList()).map { it.toByte() }.toByteArray()

        override fun localAddress(): String? = protocol.localAddress()

        override fun verify(assertion: ByteArray): String =
            verifyIdentityAssertion(assertion.map { it.toUByte() })

        override fun peerConnected(address: String) =
            protocol.wifiDirectPeerConnected(address)

        override fun messageReceived(address: String, body: ByteArray) =
            protocol.wifiDirectMessageReceived(address, body.map { it.toUByte() })

        override fun peerDisconnected(address: String) =
            protocol.wifiDirectPeerDisconnected(address)

        override fun diagnostic(level: String, message: String, context: Map<String, Any?>) =
            emitDiagnostic(level, message, context)
    })
    // True while this device's one outbound socket (a client's, toward the
    // group owner) is connecting or open, so a repeated CONNECTION_CHANGED
    // does not open a second one to the same owner.
    private val outboundOpen = AtomicBoolean(false)
    private val discoveredPeers = ConcurrentHashMap<String, WifiP2pDevice>()
    
    // State tracking. Volatile: written on the transport thread, read by the
    // socket thread that decides whether a client reconnects.
    @Volatile private var isGroupOwner = false
    @Volatile private var groupOwnerAddress: String? = null
    // The client's next reconnect delay; see [RECONNECT_INITIAL_DELAY_MS].
    private val reconnectDelayMs = AtomicLong(RECONNECT_INITIAL_DELAY_MS)

    // True between pause() and resume(). Mirrors InternetManager's flag of the
    // same name, and exists for the same reason: stopping the poll timer is not
    // the same as pausing the transport.
    //
    // The timer [pause] removes is the 2s *fallback*. The primary send path is
    // [onMessagesAvailable] → [drainAndSendMessages], which gated only on
    // `state` — and pause does not change `state` — so a paused transport went
    // on draining batches of [MAX_DRAIN_BATCH], each message a global-mutex
    // acquisition. Worse, a budget-spent pass reposts itself as an anonymous
    // lambda, which `removeCallbacks` cannot target: nothing but this flag can
    // stop a continuation already in flight.
    //
    // Volatile although every current access is confined to the transport
    // thread: writes come from `runSync` blocks and reads from posted blocks
    // — this manager's [onMessagesAvailable] posts unconditionally and never
    // reads the flag itself. Kept volatile because this is exactly the kind
    // of flag a future callback will read from the core's thread, as Nostr's
    // and Reticulum's already do.
    @Volatile private var isPaused = false

    // True while a budget-spent drain pass has queued its own continuation.
    // Confined to the transport thread — [drainAndSendMessages] and the block
    // in [onMessagesAvailable] are its only reader and writer, and both run
    // there — so a plain Boolean is enough; @Volatile would advertise a
    // cross-thread access that must not exist.
    private var drainContinuationQueued = false

    // Message polling
    private val messagePollingRunnable = object : Runnable {
        override fun run() {
            // Returning here also stops the repost, so a paused transport's
            // poll chain terminates itself rather than relying on the
            // `removeCallbacks` in [pause] having landed first.
            if (isPaused) return
            pollAndSendMessages()
            if (state == TransportState.RUNNING) {
                transportHandler.postDelayed(this, MESSAGE_POLL_INTERVAL_MS)
            }
        }
    }
    
    // Broadcast receiver for WiFi P2P events
    private val p2pReceiver = object : BroadcastReceiver() {
        override fun onReceive(context: Context, intent: Intent) {
            when (intent.action) {
                WifiP2pManager.WIFI_P2P_STATE_CHANGED_ACTION -> {
                    val state = intent.getIntExtra(WifiP2pManager.EXTRA_WIFI_STATE, -1)
                    handleWifiP2pStateChanged(state == WifiP2pManager.WIFI_P2P_STATE_ENABLED)
                }
                WifiP2pManager.WIFI_P2P_PEERS_CHANGED_ACTION -> {
                    handlePeersChanged()
                }
                WifiP2pManager.WIFI_P2P_CONNECTION_CHANGED_ACTION -> {
                    val networkInfo = if (Build.VERSION.SDK_INT >= Build.VERSION_CODES.TIRAMISU) {
                        intent.getParcelableExtra(WifiP2pManager.EXTRA_NETWORK_INFO, android.net.NetworkInfo::class.java)
                    } else {
                        @Suppress("DEPRECATION")
                        intent.getParcelableExtra(WifiP2pManager.EXTRA_NETWORK_INFO)
                    }
                    handleConnectionChanged(networkInfo?.isConnected == true)
                }
                WifiP2pManager.WIFI_P2P_THIS_DEVICE_CHANGED_ACTION -> {
                    val device = if (Build.VERSION.SDK_INT >= Build.VERSION_CODES.TIRAMISU) {
                        intent.getParcelableExtra(WifiP2pManager.EXTRA_WIFI_P2P_DEVICE, WifiP2pDevice::class.java)
                    } else {
                        @Suppress("DEPRECATION")
                        intent.getParcelableExtra(WifiP2pManager.EXTRA_WIFI_P2P_DEVICE)
                    }
                    device?.let { handleThisDeviceChanged(it) }
                }
            }
        }
    }

    // MARK: - TransportManager Implementation

    override fun isAvailable(): Boolean {
        return wifiP2pManager != null && hasRequiredPermissions()
    }

    @SuppressLint("MissingPermission")
    override fun start() {
        confinement.runSync { startUnsafe() }
    }

    @SuppressLint("MissingPermission")
    private fun startUnsafe() {
        if (state == TransportState.RUNNING) {
            throw TransportException.AlreadyRunning()
        }

        if (!isAvailable()) {
            throw TransportException.NotAvailable("WiFi P2P is not available on this device")
        }

        Log.i(TAG, "Starting WiFi Direct transport for device: $deviceId")
        emitDiagnostic("info", "Starting WiFi Direct transport", mapOf(
            "deviceId" to deviceId
        ))

        updateState(TransportState.STARTING)

        // An explicit start() means "run": a pause() from a previous session
        // must not leave this fresh transport connected-but-mute. Mirrors
        // InternetManager.startUnsafe.
        isPaused = false

        // Initialize the channel on the transport looper: `srcLooper` is the
        // thread every ActionListener / PeerListListener / ConnectionInfoListener
        // callback is delivered on, and those callbacks share this manager's
        // state with the drain and the poll. Passing the main looper here was
        // what put the framework's half of this transport on the UI thread.
        channel = wifiP2pManager?.initialize(context, confinement.looper) {
            emitDiagnostic("warning", "WiFi P2P channel disconnected")
        }

        // Register broadcast receiver. The scheduler overload delivers
        // onReceive on the transport thread instead of main — without it a
        // WIFI_P2P_STATE_CHANGED broadcast would still call
        // wifiDirectStatusChanged (a UniFFI call, so the global protocol
        // mutex) on the UI thread, which is the defect this manager is being
        // moved off main to fix.
        val intentFilter = IntentFilter().apply {
            addAction(WifiP2pManager.WIFI_P2P_STATE_CHANGED_ACTION)
            addAction(WifiP2pManager.WIFI_P2P_PEERS_CHANGED_ACTION)
            addAction(WifiP2pManager.WIFI_P2P_CONNECTION_CHANGED_ACTION)
            addAction(WifiP2pManager.WIFI_P2P_THIS_DEVICE_CHANGED_ACTION)
        }

        if (Build.VERSION.SDK_INT >= Build.VERSION_CODES.TIRAMISU) {
            context.registerReceiver(
                p2pReceiver,
                intentFilter,
                null,
                transportHandler,
                Context.RECEIVER_NOT_EXPORTED,
            )
        } else {
            context.registerReceiver(p2pReceiver, intentFilter, null, transportHandler)
        }

        // Start peer discovery
        startPeerDiscovery()

        // Start server socket
        startServerSocket()

        updateState(TransportState.RUNNING)

        // Notify protocol
        try {
            protocol.wifiDirectStatusChanged(true)
        } catch (e: Exception) {
            Log.e(TAG, "Error notifying protocol of start", e)
        }

        // Start message polling
        transportHandler.post(messagePollingRunnable)

        emitDiagnostic("info", "WiFi Direct transport started")
    }

    override fun stop() {
        confinement.runSync { stopUnsafe() }
    }

    private fun stopUnsafe() {
        if (state != TransportState.RUNNING && state != TransportState.STARTING) {
            return
        }

        updateState(TransportState.STOPPING)

        // Stop message polling
        transportHandler.removeCallbacks(messagePollingRunnable)

        // Stop peer discovery
        stopPeerDiscovery()

        // Stop server socket
        stopServerSocket()

        // Close all connections
        closeAllConnections()
        // Forget the group, so a reconnect already posted finds nothing to
        // dial and a later start() waits for its own CONNECTION_CHANGED.
        isGroupOwner = false
        groupOwnerAddress = null
        reconnectDelayMs.set(RECONNECT_INITIAL_DELAY_MS)

        // Unregister receiver
        try {
            context.unregisterReceiver(p2pReceiver)
        } catch (e: Exception) {
            // Ignore - might not be registered
        }

        // Release the framework channel. start() creates a fresh one every
        // time, so an unclosed channel leaks its binder connection once per
        // start()/stop() cycle. close() exists since API 27; below that the
        // framework offers no release and the leak is unavoidable.
        if (Build.VERSION.SDK_INT >= Build.VERSION_CODES.O_MR1) {
            try { channel?.close() } catch (_: Exception) {}
        }
        channel = null

        // Notify protocol
        try {
            protocol.wifiDirectStatusChanged(false)
        } catch (e: Exception) {
            Log.e(TAG, "Error notifying protocol of stop", e)
        }

        updateState(TransportState.STOPPED)
        emitDiagnostic("info", "WiFi Direct transport stopped")
    }

    override fun pause() {
        confinement.runSync {
            // Set before the removal, and the only thing that stops a drain
            // continuation already queued — those are posted as anonymous
            // lambdas, which removeCallbacks cannot target. See [isPaused].
            isPaused = true
            transportHandler.removeCallbacks(messagePollingRunnable)
            stopPeerDiscovery()
        }
    }

    override fun resume() {
        confinement.runSync {
            isPaused = false
            if (state == TransportState.RUNNING) {
                startPeerDiscovery()
                // Drain what queued during the pause. Unlike the other three
                // managers, restarting the timer is not enough here: a poll
                // tick sends one message every 2s, and the core does not
                // re-issue [onMessagesAvailable] for messages it already
                // announced, so a backlog would trickle out at that rate.
                //
                // Posted, not inline. This block already runs on the
                // transport thread, so an inline call would be correct — but
                // the `runSync` wrapping it holds the caller until the whole
                // block finishes, and the caller is the RN native-modules
                // thread ([OfflineProtocolModule.resume]). Inline, that
                // thread waits through up to [MAX_DRAIN_BATCH] global-mutex
                // acquisitions; posted, it is released after the cheap
                // lifecycle work and the drain runs on this same thread a
                // moment later. Ordering is preserved: this post is enqueued
                // ahead of the polling runnable below, so the backlog still
                // goes out before the first fallback tick.
                //
                // Carries [onMessagesAvailable]'s continuation check for the
                // same reason it does, and skipping it here would not have
                // been harmless. The reachable ordering is this one: a
                // continuation queued before the pause is still ahead of us
                // on the looper when `resume()` posts this lambda — it does
                // *not* self-cancel, because by the time it runs the flag is
                // already false again — so it drains, spends its budget, and
                // reposts. Without the check this lambda then starts a second
                // self-reposting chain alongside it, two [MAX_DRAIN_BATCH]
                // budgets deep, which is exactly the state
                // [drainContinuationQueued] exists to make impossible. That
                // continuation drains this backlog anyway, so returning here
                // loses nothing.
                transportHandler.post {
                    if (drainContinuationQueued) return@post
                    drainAndSendMessages()
                }
                // Removed before posting, which is the idiom the other three
                // managers keep in their startMessagePolling helpers. This
                // runnable reposts itself, and [startUnsafe] already posted one
                // — so a resume() that does not follow a pause() (two in a row,
                // or one against a transport that never paused) leaves two
                // self-reposting chains running and doubles the FFI rate until
                // the next stop(). Same-thread, so removeCallbacks is exact
                // here rather than racing an in-flight repost.
                transportHandler.removeCallbacks(messagePollingRunnable)
                transportHandler.post(messagePollingRunnable)
            }
        }
    }

    // MARK: - Permission Helpers

    private fun hasRequiredPermissions(): Boolean {
        val permissions = if (Build.VERSION.SDK_INT >= Build.VERSION_CODES.TIRAMISU) {
            listOf(
                Manifest.permission.NEARBY_WIFI_DEVICES,
                Manifest.permission.ACCESS_FINE_LOCATION
            )
        } else {
            listOf(
                Manifest.permission.ACCESS_FINE_LOCATION,
                Manifest.permission.ACCESS_WIFI_STATE,
                Manifest.permission.CHANGE_WIFI_STATE
            )
        }

        return permissions.all {
            ContextCompat.checkSelfPermission(context, it) == PackageManager.PERMISSION_GRANTED
        }
    }

    // MARK: - P2P Discovery

    @SuppressLint("MissingPermission")
    private fun startPeerDiscovery() {
        if (!hasRequiredPermissions()) {
            emitDiagnostic("warning", "Missing permissions for peer discovery")
            return
        }

        wifiP2pManager?.discoverPeers(channel, object : WifiP2pManager.ActionListener {
            override fun onSuccess() {
                emitDiagnostic("info", "Peer discovery started")
            }

            override fun onFailure(reason: Int) {
                emitDiagnostic("error", "Failed to start peer discovery", mapOf(
                    "reason" to reasonToString(reason)
                ))
            }
        })
    }

    private fun stopPeerDiscovery() {
        wifiP2pManager?.stopPeerDiscovery(channel, object : WifiP2pManager.ActionListener {
            override fun onSuccess() {
                emitDiagnostic("info", "Peer discovery stopped")
            }

            override fun onFailure(reason: Int) {
                emitDiagnostic("warning", "Failed to stop peer discovery", mapOf(
                    "reason" to reasonToString(reason)
                ))
            }
        })
    }

    // MARK: - P2P Event Handlers

    private fun handleWifiP2pStateChanged(enabled: Boolean) {
        emitDiagnostic("info", "WiFi P2P state changed", mapOf(
            "enabled" to enabled
        ))

        if (!enabled && state == TransportState.RUNNING) {
            // WiFi P2P was disabled.
            //
            // Posted rather than called inline, even though this already runs
            // on the transport thread. This body *is* the broadcast receiver's
            // onReceive — [startUnsafe] registers with this handler as the
            // scheduler — and wifiDirectStatusChanged is a UniFFI call that
            // waits on the core's global protocol mutex, seconds of it under
            // contention. Holding a receiver's dispatch window open for that is
            // the hazard [drainAndSendMessages] spends a batch budget to avoid,
            // reached through the framework's door instead of ours. Posting
            // lets onReceive return and runs the call as the next item on the
            // same queue, so the ordering is unchanged.
            //
            // Re-read inside, because a stop() fits in the gap the post opens.
            transportHandler.post {
                if (state != TransportState.RUNNING) return@post
                // Streams first, so each announced peer is reported lost
                // while the core still holds its link. A stream left open
                // across the flip would keep delivering, and every body
                // re-adds a neighbour the flip just cleared.
                closeAllConnections()
                try {
                    protocol.wifiDirectStatusChanged(false)
                } catch (e: Exception) {
                    Log.e(TAG, "Error notifying protocol", e)
                }
            }
        }
    }

    @SuppressLint("MissingPermission")
    private fun handlePeersChanged() {
        if (!hasRequiredPermissions()) return

        wifiP2pManager?.requestPeers(channel) { peers ->
            val deviceList = peers?.deviceList ?: return@requestPeers

            // Update discovered peers
            val currentPeers = mutableSetOf<String>()
            for (device in deviceList) {
                currentPeers.add(device.deviceAddress)
                if (!discoveredPeers.containsKey(device.deviceAddress)) {
                    discoveredPeers[device.deviceAddress] = device
                    emitDiagnostic("info", "Discovered peer", mapOf(
                        "name" to device.deviceName,
                        "address" to device.deviceAddress
                    ))
                }
            }

            // Remove lost peers
            val lostPeers = discoveredPeers.keys - currentPeers
            for (address in lostPeers) {
                discoveredPeers.remove(address)
                emitDiagnostic("info", "Lost peer", mapOf(
                    "address" to address
                ))
            }
        }
    }

    @SuppressLint("MissingPermission")
    private fun handleConnectionChanged(connected: Boolean) {
        emitDiagnostic("info", "Connection changed", mapOf(
            "connected" to connected
        ))

        if (connected && hasRequiredPermissions()) {
            wifiP2pManager?.requestConnectionInfo(channel) { info ->
                info?.let {
                    isGroupOwner = it.isGroupOwner
                    groupOwnerAddress = it.groupOwnerAddress?.hostAddress

                    emitDiagnostic("info", "Connection info", mapOf(
                        "isGroupOwner" to isGroupOwner,
                        "groupOwnerAddress" to (groupOwnerAddress ?: "null")
                    ))

                    if (!isGroupOwner && groupOwnerAddress != null) {
                        // Connect to group owner
                        connectToGroupOwner(groupOwnerAddress!!)
                    }
                }
            }
        } else {
            isGroupOwner = false
            groupOwnerAddress = null
            // Posted: this is the broadcast receiver's onReceive, and ending
            // an announced stream is an FFI call. Same reasoning as the post
            // in [handleWifiP2pStateChanged].
            transportHandler.post { closeAllConnections() }
        }
    }

    private fun handleThisDeviceChanged(device: WifiP2pDevice) {
        emitDiagnostic("debug", "This device changed", mapOf(
            "name" to device.deviceName,
            "status" to device.status
        ))
    }

    // MARK: - Socket Operations

    private fun startServerSocket() {
        serverRunning.set(true)
        
        socketExecutor.execute {
            // Held locally as well as published, so the finally closes exactly
            // the socket this task bound. A stop() can land before the bind
            // publishes: it clears serverRunning, closes a still-null field
            // and nulls it — then the bind completes, the loop guard reads
            // false and exits immediately, and without the finally the
            // freshly bound socket would leak with port 8988 still held,
            // failing the next start() with BindException.
            var server: ServerSocket? = null
            try {
                server = ServerSocket(SERVER_PORT)
                server.soTimeout = SOCKET_TIMEOUT_MS
                serverSocket = server
                
                emitDiagnostic("info", "Server socket started on port $SERVER_PORT")
                
                // `serverRunning` alone is the wrong question, because it is
                // process state and this loop is asking about *its* socket. A
                // stop() clears the flag and closes the socket; a start()
                // landing before this thread is next scheduled sets the flag
                // back — and there is room for one, since stopUnsafe still has
                // closeAllConnections, unregisterReceiver, channel.close() and
                // a status FFI under the global mutex to get through. The
                // accept then throws on the closed socket, the catch reads a
                // flag that is true again, and this spins at full tilt emitting
                // a diagnostic per turn, never reaching the finally.
                while (serverRunning.get() && !server.isClosed) {
                    try {
                        handleClientConnection(server.accept())
                    } catch (e: java.net.SocketTimeoutException) {
                        // Expected timeout, continue
                    } catch (e: Exception) {
                        if (serverRunning.get()) {
                            emitDiagnostic("error", "Error accepting connection", mapOf(
                                "error" to (e.message ?: "unknown")
                            ))
                        }
                    }
                }
            } catch (e: Exception) {
                emitDiagnostic("error", "Failed to start server socket", mapOf(
                    "error" to (e.message ?: "unknown")
                ))
            } finally {
                try { server?.close() } catch (_: Exception) {}
            }
        }
    }

    private fun stopServerSocket() {
        serverRunning.set(false)
        try {
            serverSocket?.close()
        } catch (e: Exception) {
            // Ignore
        }
        serverSocket = null
    }

    private fun handleClientConnection(socket: Socket) {
        socketExecutor.execute {
            sockets.run(socket, outbound = false) { state == TransportState.RUNNING }
        }
    }

    private fun connectToGroupOwner(address: String) {
        // One outbound socket at a time. CONNECTION_CHANGED fires for every
        // change to the group, not only for our joining it, and each used to
        // open another socket to the same owner.
        if (!outboundOpen.compareAndSet(false, true)) return
        socketExecutor.execute {
            var proved = false
            try {
                val socket = Socket()
                try {
                    socket.connect(InetSocketAddress(address, SERVER_PORT), CONNECTION_TIMEOUT_MS.toInt())
                } catch (e: Exception) {
                    try { socket.close() } catch (_: Exception) {}
                    emitDiagnostic("error", "Failed to connect to group owner", mapOf(
                        "address" to address,
                        "error" to (e.message ?: "unknown")
                    ))
                    return@execute
                }
                emitDiagnostic("info", "Connected to group owner", mapOf(
                    "address" to address
                ))
                // No claim to compare against: the owner's IP says nothing
                // about who it is, so the address its preamble derives is its
                // id (stream-framing.md, "The preamble").
                proved = sockets.run(socket, outbound = true) { state == TransportState.RUNNING } != null
            } finally {
                outboundOpen.set(false)
                scheduleReconnect(address, proved)
            }
        }
    }

    /**
     * Reconnects a client whose stream to the group owner ended while the
     * group is still up. CONNECTION_CHANGED fires only when the group changes,
     * so without this a client whose socket dropped (the owner's app
     * restarted, a write deadline fired) stays unreachable for as long as the
     * group lasts. The delay doubles after an attempt that proved no peer, so
     * an owner that refuses us is not dialled in a loop, and resets after one
     * that did.
     */
    private fun scheduleReconnect(address: String, proved: Boolean) {
        if (proved) reconnectDelayMs.set(RECONNECT_INITIAL_DELAY_MS)
        if (state != TransportState.RUNNING || isGroupOwner || groupOwnerAddress != address) return
        val delay = reconnectDelayMs.get()
        reconnectDelayMs.set(minOf(delay * 2, RECONNECT_MAX_DELAY_MS))
        transportHandler.postDelayed({
            // Not gated on pause: a paused transport keeps its open streams,
            // so it keeps reconnecting one that dropped.
            if (state == TransportState.RUNNING && !isGroupOwner && groupOwnerAddress == address) {
                connectToGroupOwner(address)
            }
        }, delay)
    }

    /**
     * Ends every stream, reporting each announced one lost. Runs on the
     * transport thread and makes FFI calls, so a broadcast receiver posts it
     * rather than calling it inline (see [handleWifiP2pStateChanged]).
     */
    private fun closeAllConnections() {
        sockets.closeAll()
    }

    // MARK: - Message Handling (Event-Driven)

    /**
     * Called by the Rust transport callback when new outgoing messages are available.
     * This is the primary send path, replacing the 100ms polling loop.
     *
     * The pre-post check mirrors [NostrManager.onMessagesAvailable] and
     * [ReticulumManager.onMessagesAvailable], and earns its keep for a reason
     * specific to this manager: [drainAndSendMessages] already refuses while
     * paused, so without this every core callback during a background stay
     * still posts a runnable onto [transportHandler] just to return. That
     * looper is not this manager's alone — [startUnsafe] hands it to
     * `WifiP2pManager.initialize` and to `registerReceiver`, and a broadcast
     * that misses Android's dispatch budget is an ANR wherever the receiver
     * lives. Not posting at all is strictly better than posting a no-op.
     */
    fun onMessagesAvailable() {
        if (isPaused) return
        transportHandler.post {
            // A pass that spent its budget has already queued a continuation,
            // and that continuation drains whatever this callback is
            // announcing. Without the check, every callback arriving against a
            // deep queue starts its own self-reposting chain of
            // [MAX_DRAIN_BATCH] mutex acquisitions apiece — several budgets
            // running concurrently, which is precisely the looper starvation
            // the budget exists to prevent.
            if (drainContinuationQueued) return@post
            drainAndSendMessages()
        }
    }

    /**
     * Drains the Rust message queue and sends each message over WiFi Direct.
     * Called from onMessagesAvailable() and from the fallback polling timer.
     *
     * Bounded, and reposting rather than looping past the bound, because this
     * looper is not this manager's alone. [startUnsafe] hands it to
     * `WifiP2pManager.initialize` as the callback looper and to
     * `registerReceiver` as the broadcast scheduler, so every P2P framework
     * callback queues behind whatever the drain is doing — and a broadcast
     * that does not reach `onReceive` inside Android's dispatch budget is an
     * ANR wherever the receiver lives, main or not. `stop()` waits on this
     * thread without a bound too ([TransportConfinement.runSync]), so an
     * unbounded drain would put a teardown behind an arbitrary number of
     * mutex acquisitions.
     *
     * Each iteration takes the global protocol mutex once, so the bound is a
     * fairness budget rather than a correctness one: a queue deeper than
     * [MAX_DRAIN_BATCH] finishes on the next posted pass, after everything
     * else sharing this looper has had its turn.
     *
     * There is at most one such chain at a time — [drainContinuationQueued] is
     * what makes the budget mean anything, since N concurrent chains spend N
     * budgets between the framework callbacks they were meant to make room for.
     */
    private fun drainAndSendMessages() {
        // Reached the front of the queue, so whatever continuation was pending
        // is this one. Cleared here rather than at each exit so that every way
        // out leaves it false — the stopped-transport return, the drained
        // return, a throwing send — and only the one path that actually queues
        // a successor sets it again. A chain that ends because the transport
        // stopped must not leave the flag suppressing the callbacks that the
        // next start() delivers.
        drainContinuationQueued = false

        // `isPaused` alongside the state, because pause() does not change the
        // state and this is the *primary* send path — the timer it removes is
        // the 2s fallback. Checked here rather than in [onMessagesAvailable]
        // alone so it also catches the self-posted continuation, which no
        // removeCallbacks can reach. Clearing the flag above first is what
        // keeps a pause from wedging the callback path: the pass exits with
        // drainContinuationQueued false, so resume()'s drain is not suppressed.
        if (isPaused || state != TransportState.RUNNING || sockets.isEmpty()) return

        try {
            var sent = 0
            while (sent < MAX_DRAIN_BATCH) {
                val message = protocol.wifiDirectGetNextMessage() ?: return
                sendMessage(message.recipientId, message.data.map { it.toByte() }.toByteArray())
                sent++
            }
            // Budget spent with the queue still non-empty: yield the looper and
            // come back, rather than starving the callbacks that share it.
            drainContinuationQueued = true
            transportHandler.post { drainAndSendMessages() }
        } catch (e: Exception) {
            emitDiagnostic("error", "Error in drainAndSendMessages", mapOf(
                "error" to (e.message ?: "unknown")
            ))
        }
    }

    private fun pollAndSendMessages() {
        if (state != TransportState.RUNNING || sockets.isEmpty()) return

        try {
            val message = protocol.wifiDirectGetNextMessage()
            if (message != null) {
                sendMessage(message.recipientId, message.data.map { it.toByte() }.toByteArray())
            }
        } catch (e: Exception) {
            emitDiagnostic("error", "Error polling messages", mapOf(
                "error" to (e.message ?: "unknown")
            ))
        }
    }

    /** Queues one body on the stream that proved [recipientId]; see [PeerStreamSockets.send]. */
    private fun sendMessage(recipientId: String, data: ByteArray) {
        val reason = when (sockets.send(recipientId, data)) {
            PeerStreamSockets.SendResult.QUEUED -> return
            PeerStreamSockets.SendResult.NO_STREAM -> "no stream holds the recipient"
            PeerStreamSockets.SendResult.OUT_OF_BOUNDS -> "outside the frame bounds"
            PeerStreamSockets.SendResult.QUEUE_FULL -> "the peer is stalled and its queue is full"
        }
        emitDiagnostic("warning", "Wi-Fi Direct body dropped: $reason", mapOf(
            "to" to recipientId,
            "size" to data.size
        ))
    }

    // MARK: - Utility

    private fun reasonToString(reason: Int): String {
        return when (reason) {
            WifiP2pManager.P2P_UNSUPPORTED -> "P2P_UNSUPPORTED"
            WifiP2pManager.BUSY -> "BUSY"
            WifiP2pManager.ERROR -> "ERROR"
            else -> "UNKNOWN ($reason)"
        }
    }

    private fun updateState(newState: TransportState) {
        state = newState
        listener?.onTransportStateChanged(this, newState)
    }

    private fun emitDiagnostic(level: String, message: String, context: Map<String, Any?> = emptyMap()) {
        diagnosticEmitter?.invoke(level, message, context)
        listener?.onTransportDiagnostic(this, level, message, context)
    }
}

