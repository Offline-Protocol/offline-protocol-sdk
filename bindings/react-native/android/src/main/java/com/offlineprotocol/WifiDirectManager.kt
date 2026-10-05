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
import android.os.SystemClock
import android.util.Log
import android.net.wifi.p2p.nsd.WifiP2pDnsSdServiceInfo
import android.net.wifi.p2p.nsd.WifiP2pDnsSdServiceRequest
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
 * Forming the group is opt-in. With [formGroups] (`wifiDirect.autoAccept`)
 * on Android 10 and later, the manager finds devices of the same application
 * over Wi-Fi P2P service discovery and forms or joins one group by derived
 * credentials, with no system dialog on either phone; the rules are in
 * [WifiDirectGroupFormation]. Without it the manager never calls
 * `WifiP2pManager.connect` or `createGroup`: a group comes from the system's
 * Wi-Fi Direct settings or from another app. Either way the socket layer joins
 * when `WIFI_P2P_CONNECTION_CHANGED_ACTION` says a group exists, or at start
 * when one already does (see [adoptExistingGroup]).
 */
class WifiDirectManager(
    private val context: Context,
    private val protocol: OfflineProtocol,
    private val deviceId: String,
    // The application id, from which the group passphrase is derived; see
    // [WifiDirectGroupFormation].
    private val appId: String = "",
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
        // Group formation (opt-in, [formGroups]). A step runs on this period;
        // after it acts it waits at least the retry delay, doubled on each
        // failure up to the ceiling, so a forged or stale record cannot keep
        // the radio busy. A record not heard again within the TTL is dropped.
        private const val FORMATION_TICK_MS = 10_000L
        private const val FORMATION_RETRY_MS = 20_000L
        private const val FORMATION_MAX_BACKOFF_MS = 120_000L
        private const val ADVERT_TTL_MS = 180_000L
        // How often service discovery is asked again while there is anything
        // to find. Each call cancels and re-queues the supplicant's query and
        // restarts its search, so it is not asked on every step.
        private const val SERVICE_DISCOVERY_PERIOD_MS = 15_000L
        // How long a group this manager formed may stay without clients. An
        // owner answers no service discovery query, so an empty group is
        // invisible to the peer it was made for (after either side restarts,
        // say); dissolving it makes the owner discoverable again, and the next
        // round forms the group with whoever is there.
        private const val OWNER_IDLE_MS = 45_000L
    }

    /**
     * Whether this manager forms and joins groups itself (Android 10 and
     * later), from `wifiDirect.autoAccept`. Off by default: then a group comes
     * from the system settings, as before. Set before [start].
     */
    @Volatile var formGroups: Boolean = false

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

    // Group formation state. Confined to the transport thread: the framework
    // delivers every listener on the channel's looper, which is this one.
    private val adverts = HashMap<String, WifiDirectGroupFormation.Advert>()
    private var inGroup = false
    private var ownerClients = 0
    private var createdNetwork: String? = null
    private var nextFormationAtMs = 0L
    private var formationBackoffMs = FORMATION_RETRY_MS
    private var cachedLocalAddress: String? = null
    private var advertPublished = false
    private var nextServiceDiscoveryAtMs = 0L
    // True while this device owns a group under the name it would derive,
    // which is the only kind of group formation ever dissolves.
    private var ownsFormationGroup = false
    private var ownerIdleSinceMs = 0L
    private var advertNetwork: String? = null
    private val appTag: String by lazy { WifiDirectGroupFormation.appTag(appId) }
    
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

        adoptExistingGroup()
        startGroupFormation()

        emitDiagnostic("info", "WiFi Direct transport started")
    }

    /**
     * Joins the socket layer to a group that formed before this start.
     *
     * Since Android 10 `WIFI_P2P_CONNECTION_CHANGED_ACTION` is not sticky, so
     * the receiver registered above hears nothing about a group that already
     * exists. Without this, an app restarted while its device was still
     * grouped (the group belongs to the system, not the process) neither
     * listened for its peer as owner nor dialled its owner as client, and
     * stayed unreachable until someone tore the group down by hand. The
     * answer arrives on the transport looper, like every other P2P callback.
     */
    @SuppressLint("MissingPermission")
    private fun adoptExistingGroup() {
        wifiP2pManager?.requestConnectionInfo(channel) { info ->
            if (info?.groupFormed == true && state == TransportState.RUNNING) {
                emitDiagnostic("info", "Joining a group formed before start")
                handleConnectionChanged(true)
            }
        }
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
        stopGroupFormation()
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
        // On 13+ the module's manifest declares NEARBY_WIFI_DEVICES with
        // neverForLocation, which is what lets every P2P call run without a
        // location grant there; demanding one anyway made an app ask for
        // location it does not use, or lose the transport without it.
        val permissions = if (Build.VERSION.SDK_INT >= Build.VERSION_CODES.TIRAMISU) {
            listOf(Manifest.permission.NEARBY_WIFI_DEVICES)
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
        inGroup = connected
        if (connected) formationBackoffMs = FORMATION_RETRY_MS

        if (connected && hasRequiredPermissions()) {
            wifiP2pManager?.requestConnectionInfo(channel) { info ->
                info?.let {
                    val previousOwner = groupOwnerAddress
                    isGroupOwner = it.isGroupOwner
                    groupOwnerAddress = it.groupOwnerAddress?.hostAddress
                    // A new connection starts its redial ladder from the
                    // bottom. The delay doubles on each failed dial and was
                    // reset only by a proved stream or stop(), so a new group's
                    // first failed dial waited out the old group's backoff.
                    // Group owners on Android nearly always take the same
                    // address, so this compares against the null the
                    // disconnect branch below leaves, not one owner with
                    // another: it resets on every connection after a
                    // disconnect, the same group rejoined included.
                    if (groupOwnerAddress != previousOwner) {
                        reconnectDelayMs.set(RECONNECT_INITIAL_DELAY_MS)
                    }

                    emitDiagnostic("info", "Connection info", mapOf(
                        "isGroupOwner" to isGroupOwner,
                        "groupOwnerAddress" to (groupOwnerAddress ?: "null")
                    ))

                    if (!isGroupOwner && groupOwnerAddress != null) {
                        // Connect to group owner
                        connectToGroupOwner(groupOwnerAddress!!)
                    }
                    refreshGroupForFormation()
                }
            }
        } else {
            isGroupOwner = false
            groupOwnerAddress = null
            ownerClients = 0
            createdNetwork = null
            ownsFormationGroup = false
            ownerIdleSinceMs = 0L
            if (formGroups) publishAdvert(null)
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

    // MARK: - Group formation (opt-in; see WifiDirectGroupFormation)

    private val formationSupported: Boolean
        get() = Build.VERSION.SDK_INT >= Build.VERSION_CODES.Q

    private val formationTick = object : Runnable {
        override fun run() {
            if (state != TransportState.RUNNING || !formGroups) return
            if (!isPaused) formationStep()
            transportHandler.postDelayed(this, FORMATION_TICK_MS)
        }
    }

    @SuppressLint("MissingPermission")
    private fun startGroupFormation() {
        if (!formGroups) return
        if (!formationSupported) {
            emitDiagnostic("info", "Wi-Fi Direct group formation needs Android 10; join a group from the system settings")
            return
        }
        val p2p = wifiP2pManager ?: return
        p2p.setDnsSdResponseListeners(
            channel,
            { _, _, _ -> },
            { domain, txt, device ->
                if (!domain.contains(WifiDirectGroupFormation.SERVICE_TYPE)) return@setDnsSdResponseListeners
                val advert = WifiDirectGroupFormation.parse(
                    device.deviceAddress, txt, SystemClock.elapsedRealtime()
                ) ?: return@setDnsSdResponseListeners
                val previous = adverts.put(device.deviceAddress, advert)
                if (previous == null || previous.ownerNetwork != advert.ownerNetwork) {
                    emitDiagnostic("debug", "Wi-Fi Direct peer record", mapOf(
                        "address" to advert.address,
                        "owner" to (advert.ownerNetwork != null),
                    ))
                }
            },
        )
        // A TXT query for the one instance name every device of this protocol
        // publishes. A query by service type alone asks only for the PTR
        // record, whose answer reaches the service listener above but never
        // the TXT listener, and the TXT record is where the address and the
        // group name are.
        p2p.addServiceRequest(
            channel,
            WifiP2pDnsSdServiceRequest.newInstance(
                WifiDirectGroupFormation.INSTANCE_NAME,
                WifiDirectGroupFormation.SERVICE_TYPE,
            ),
            quietListener("add service request"),
        )
        publishAdvert(null)
        transportHandler.removeCallbacks(formationTick)
        transportHandler.post(formationTick)
        emitDiagnostic("info", "Wi-Fi Direct group formation started")
    }

    @SuppressLint("MissingPermission")
    private fun stopGroupFormation() {
        transportHandler.removeCallbacks(formationTick)
        if (!formGroups || !formationSupported) return
        val p2p = wifiP2pManager ?: return
        p2p.clearServiceRequests(channel, quietListener("clear service requests"))
        p2p.clearLocalServices(channel, quietListener("clear local services"))
        // A group this manager made goes with it. One the system formed, or one
        // adopted at start, is not this run's to remove.
        if (createdNetwork != null) p2p.removeGroup(channel, quietListener("remove group"))
        createdNetwork = null
        adverts.clear()
        inGroup = false
        ownerClients = 0
        advertPublished = false
        advertNetwork = null
    }

    /** One step: refresh what is known, and act on it if the backoff allows. */
    @SuppressLint("MissingPermission")
    private fun formationStep() {
        val p2p = wifiP2pManager ?: return
        val now = SystemClock.elapsedRealtime()
        adverts.values.removeAll { now - it.seenAtMs > ADVERT_TTL_MS }
        val address = localAddressOrNull() ?: return

        if (!advertPublished) publishAdvert(advertNetwork)

        if (inGroup && isGroupOwner && ownsFormationGroup && ownerClients == 0) {
            if (ownerIdleSinceMs == 0L) {
                ownerIdleSinceMs = now
            } else if (now - ownerIdleSinceMs > OWNER_IDLE_MS) {
                ownerIdleSinceMs = 0L
                emitDiagnostic("info", "Dissolving an empty Wi-Fi Direct group so peers can find this device")
                p2p.removeGroup(channel, quietListener("remove empty group"))
                return
            }
        } else {
            ownerIdleSinceMs = 0L
        }

        // Service discovery is one-shot on Android, so it is asked again while
        // there is anything to find. Only it: it runs its own peer search, and
        // a discoverPeers issued beside it is refused by the supplicant as a
        // second pending scan.
        val settled = inGroup && (!isGroupOwner || ownerClients > 0)
        if (!settled && now >= nextServiceDiscoveryAtMs) {
            nextServiceDiscoveryAtMs = now + SERVICE_DISCOVERY_PERIOD_MS
            p2p.discoverServices(channel, quietListener("discover services"))
        }
        if (now < nextFormationAtMs) return
        val action = WifiDirectGroupFormation.decide(
            WifiDirectGroupFormation.Local(address, appTag, inGroup, isGroupOwner, ownerClients),
            adverts.values,
        )
        when (action) {
            WifiDirectGroupFormation.Action.Wait -> return
            WifiDirectGroupFormation.Action.Create -> createGroup(WifiDirectGroupFormation.networkName(address))
            is WifiDirectGroupFormation.Action.Join -> joinGroup(action.network)
            is WifiDirectGroupFormation.Action.Move -> {
                emitDiagnostic("info", "Leaving an empty Wi-Fi Direct group for a lower owner's")
                p2p.removeGroup(channel, object : WifiP2pManager.ActionListener {
                    override fun onSuccess() {
                        createdNetwork = null
                        joinGroup(action.network)
                    }
                    override fun onFailure(reason: Int) = formationFailed("remove group", reason)
                })
            }
        }
        nextFormationAtMs = now + formationBackoffMs
    }

    @SuppressLint("MissingPermission")
    private fun createGroup(network: String) {
        // Here and not only in the caller, so lint's NewApi check sees it.
        if (Build.VERSION.SDK_INT < Build.VERSION_CODES.Q) return
        val config = WifiP2pConfig.Builder()
            .setNetworkName(network)
            .setPassphrase(WifiDirectGroupFormation.passphrase(appId, network))
            .enablePersistentMode(false)
            .build()
        val ch = channel ?: return
        emitDiagnostic("info", "Creating a Wi-Fi Direct group", mapOf("network" to network))
        wifiP2pManager?.createGroup(ch, config, object : WifiP2pManager.ActionListener {
            override fun onSuccess() {
                createdNetwork = network
            }
            override fun onFailure(reason: Int) = formationFailed("create group", reason)
        })
    }

    @SuppressLint("MissingPermission")
    private fun joinGroup(network: String) {
        if (Build.VERSION.SDK_INT < Build.VERSION_CODES.Q) return
        val config = WifiP2pConfig.Builder()
            .setNetworkName(network)
            .setPassphrase(WifiDirectGroupFormation.passphrase(appId, network))
            .enablePersistentMode(false)
            .build()
        val ch = channel ?: return
        emitDiagnostic("info", "Joining a Wi-Fi Direct group", mapOf("network" to network))
        wifiP2pManager?.connect(ch, config, object : WifiP2pManager.ActionListener {
            override fun onSuccess() {}
            override fun onFailure(reason: Int) = formationFailed("join group", reason)
        })
    }

    private fun formationFailed(what: String, reason: Int) {
        formationBackoffMs = minOf(formationBackoffMs * 2, FORMATION_MAX_BACKOFF_MS)
        nextFormationAtMs = SystemClock.elapsedRealtime() + formationBackoffMs
        emitDiagnostic("warning", "Wi-Fi Direct group formation: $what failed", mapOf(
            "reason" to reasonToString(reason),
            "retryInMs" to formationBackoffMs,
        ))
    }

    /**
     * After any group change: count this owner's clients and say in the record
     * whether this device owns a group others can join. Only a group whose name
     * this device would derive is offered; a group formed from the system
     * settings has a passphrase no joiner can compute.
     */
    @SuppressLint("MissingPermission")
    private fun refreshGroupForFormation() {
        if (!formGroups || !formationSupported) return
        if (!isGroupOwner) {
            ownerClients = 0
            ownsFormationGroup = false
            publishAdvert(null)
            return
        }
        wifiP2pManager?.requestGroupInfo(channel) { group ->
            ownerClients = group?.clientList?.size ?: 0
            val address = localAddressOrNull()
            val ours = address?.let { WifiDirectGroupFormation.networkName(it) }
            ownsFormationGroup = group?.networkName != null && group.networkName == ours
            publishAdvert(group?.networkName?.takeIf { it == ours })
        }
    }

    @SuppressLint("MissingPermission")
    private fun publishAdvert(ownerNetwork: String?) {
        if (!formGroups || !formationSupported) return
        val p2p = wifiP2pManager ?: return
        val address = localAddressOrNull() ?: return
        val info = WifiP2pDnsSdServiceInfo.newInstance(
            WifiDirectGroupFormation.INSTANCE_NAME,
            WifiDirectGroupFormation.SERVICE_TYPE,
            WifiDirectGroupFormation.txtRecord(address, appTag, ownerNetwork),
        )
        advertNetwork = ownerNetwork
        val added = object : WifiP2pManager.ActionListener {
            override fun onSuccess() {
                advertPublished = true
                emitDiagnostic("debug", "Wi-Fi Direct record published", mapOf("owner" to (ownerNetwork != null)))
            }
            override fun onFailure(reason: Int) {
                advertPublished = false
                emitDiagnostic("debug", "Wi-Fi Direct add local service failed", mapOf("reason" to reasonToString(reason)))
            }
        }
        // Replaced whole: a record is a set of TXT entries, not a field to edit.
        p2p.clearLocalServices(channel, object : WifiP2pManager.ActionListener {
            override fun onSuccess() = p2p.addLocalService(channel, info, added)
            override fun onFailure(reason: Int) = p2p.addLocalService(channel, info, added)
        })
    }

    private fun localAddressOrNull(): String? {
        cachedLocalAddress?.let { return it }
        return try {
            protocol.localAddress()?.also { cachedLocalAddress = it }
        } catch (_: Exception) {
            null
        }
    }

    private fun quietListener(what: String) = object : WifiP2pManager.ActionListener {
        override fun onSuccess() {}
        override fun onFailure(reason: Int) {
            emitDiagnostic("debug", "Wi-Fi Direct $what failed", mapOf("reason" to reasonToString(reason)))
        }
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
     * Dials the group owner again after this client's stream to [ended]
     * closed; see [GroupOwnerRedial] for the decision.
     *
     * The owner dialled is the one the group has now, not [ended]. On a group
     * switch the new owner's [connectToGroupOwner] runs while the old stream
     * is still winding down, loses [outboundOpen] to it, and returns; this is
     * then the only dial left, so a check that the owner is still [ended]
     * would leave the client with no stream for the whole of the new group.
     */
    private fun scheduleReconnect(ended: String, proved: Boolean) {
        val plan = GroupOwnerRedial.next(
            ended = ended,
            proved = proved,
            running = state == TransportState.RUNNING,
            isGroupOwner = isGroupOwner,
            owner = groupOwnerAddress,
            currentDelayMs = reconnectDelayMs.get(),
            initialDelayMs = RECONNECT_INITIAL_DELAY_MS,
            maxDelayMs = RECONNECT_MAX_DELAY_MS,
        )
        if (plan == null) {
            if (proved) reconnectDelayMs.set(RECONNECT_INITIAL_DELAY_MS)
            return
        }
        reconnectDelayMs.set(plan.nextDelayMs)
        transportHandler.postDelayed({
            // Not gated on pause: a paused transport keeps its open streams,
            // so it keeps reconnecting one that dropped. Re-read, because the
            // group can change again inside the delay.
            val owner = groupOwnerAddress
            if (state == TransportState.RUNNING && !isGroupOwner && owner != null) {
                connectToGroupOwner(owner)
            }
        }, plan.delayMs)
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

/**
 * When a group client dials its owner again after its stream ended, and after
 * how long. Pure, so [GroupOwnerRedialTest] pins it without a group.
 *
 * CONNECTION_CHANGED fires only when the group changes, so without a redial a
 * client whose stream dropped (the owner's app restarted, a write deadline
 * fired) stays unreachable for as long as the group lasts. The delay doubles
 * after an attempt toward the same owner that proved no peer, so an owner
 * that refuses us is not dialled in a loop. It starts over after an attempt
 * that proved one, and for an owner other than the one that ended: a group
 * switch is a new owner, not a retry.
 */
internal object GroupOwnerRedial {
    data class Plan(val owner: String, val delayMs: Long, val nextDelayMs: Long)

    fun next(
        ended: String,
        proved: Boolean,
        running: Boolean,
        isGroupOwner: Boolean,
        owner: String?,
        currentDelayMs: Long,
        initialDelayMs: Long,
        maxDelayMs: Long,
    ): Plan? {
        if (!running || isGroupOwner || owner == null) return null
        val delay = if (proved || owner != ended) initialDelayMs else currentDelayMs
        return Plan(owner, delay, minOf(delay * 2, maxDelayMs))
    }
}
