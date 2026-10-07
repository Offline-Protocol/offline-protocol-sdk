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
import java.net.InetAddress
import java.net.InetSocketAddress
import java.net.ServerSocket
import java.net.Socket
import java.util.concurrent.*
import java.util.concurrent.atomic.AtomicBoolean
import java.util.concurrent.atomic.AtomicInteger
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
 * got both wrong), one announced stream per address with the one the lower
 * address opened kept, a per-stream writer, and the ordering that keeps a delivery
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
        private const val FORMATION_RETRY_MS = 30_000L
        private const val FORMATION_MAX_BACKOFF_MS = 120_000L
        private const val ADVERT_TTL_MS = 180_000L
        // How often service discovery is asked again is
        // WifiDirectGroupFormation.discoveryPeriodMs: each call cancels and
        // re-queues the supplicant's query and restarts its search, so it is
        // not asked on every step, and less often the longer it hears nothing.
        // How long a group this manager formed may stay without clients,
        // randomised up to twice this per group so that two owners that
        // formed at once do not dissolve in step. Dissolved, the owner joins
        // first, so two groups merge; and an owner answers no service
        // discovery query, so an empty group also hides it from new peers.
        private const val OWNER_IDLE_MS = 30_000L
        // How long one join attempt may run before it is cancelled. A device
        // that is joining answers no service discovery query, so a join left
        // running hides the joiner from the very peer that has to hear it
        // before it creates the group: on two phones that kept a pair apart
        // for minutes. Cancelled, the joiner is discoverable again until the
        // next attempt.
        // Fifteen and not less: a join that has no fresh scan result scans for
        // about six seconds before it associates, and the supplicant retries a
        // rejected association; an eight-second window cut that retry off.
        private const val JOIN_WINDOW_MS = 15_000L
    }

    /**
     * Whether this manager forms and joins groups itself (Android 10 and
     * later), from `wifiDirect.autoAccept`. Off by default: then a group comes
     * from the system settings, as before. Set before [start], and only while
     * stopped: [stop] undoes what the run did whatever this reads by then,
     * but [start] reads it once.
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
    // True while this device is in a group under the application's name, as
    // owner or client. Volatile: the socket thread reads it when a client's
    // dials keep failing, to decide whether the group is ours to leave.
    @Volatile private var inAppGroup = false
    // A client's dials in a row toward an owner that proved nothing while the
    // redial ladder sat at its ceiling; see [GroupOwnerRedial.shouldLeave].
    private val unprovedAtCeiling = AtomicInteger(0)
    private var nextPublishAtMs = 0L
    private var ownerIdleSinceMs = 0L
    private var failedJoins = 0
    private var ownerIdleLimitMs = OWNER_IDLE_MS
    private var lastJoinAtMs = 0L
    private var joinFirstRemaining = 0
    // True while this run has a service request and a local service
    // registered, so stop() clears exactly what start registered, whatever
    // [formGroups] says by then.
    private var formationRegistered = false
    // Discovery backoff: rounds in a row that heard no record, and whether a
    // record arrived since the last round was asked.
    private var quietDiscoveryRounds = 0
    private var heardSinceLastDiscovery = true
    // Probe backoff: probes in a row toward an owner nobody vouched for that
    // found no group, and whether the join in flight is one. When each owner
    // was last in the peer list, so only an owner new or long gone starts the
    // backoff over (WifiDirectGroupFormation.ownersNewlySeen).
    private var failedProbes = 0
    private var joinIsProbe = false
    private val ownerLastSeenMs = HashMap<String, Long>()
    // Every Wi-Fi Direct group owner in the peer list, of any application.
    // An owner answers no service discovery query, so this is how a device
    // that heard no record still learns there is a group it might join.
    private var nearbyOwners: Set<String> = emptySet()
    // Owners this device left because they proved nothing, by device address,
    // with when, until WifiDirectGroupFormation.DEAD_OWNER_TTL_MS. Not an
    // owner nearby while remembered, and a join that lands in one's group
    // leaves at once (refreshGroupForFormation): a join cannot be steered
    // away from it. Kept across stop(): it is a fact about the radio, not
    // this run.
    private val deadOwners = HashMap<String, Long>()
    // The device address of the owner of the application's group this device
    // is a client of, so a leave knows whom to avoid.
    private var appGroupOwnerDevice: String? = null
    private val appTag: String by lazy { WifiDirectGroupFormation.appTag(appId) }
    private val appNetwork: String by lazy { WifiDirectGroupFormation.networkName(appId) }
    
    // State tracking. Volatile: written on the transport thread, read by the
    // socket thread that decides whether a client reconnects.
    @Volatile private var isGroupOwner = false
    // Whether the core was last told the stream layer is up: while either
    // carrier is ([reportLayer]). Wi-Fi P2P going off with no LAN reports it
    // down; coming back on has to report it up again, or the core keeps the
    // slot down while streams prove peers over it.
    @Volatile private var layerUp = false
    // Whether Wi-Fi P2P is on, and whether this device is on a Wi-Fi network
    // it can find LAN peers on.
    @Volatile private var p2pUp = false
    @Volatile private var lanUp = false
    // The Wi-Fi network's own addresses: a socket accepted on one of them is
    // a LAN stream, any other a group stream.
    @Volatile private var lanAddresses: Set<InetAddress> = emptySet()
    // The LAN carrier, between start and stop. Transport thread only.
    private var lan: LanPeerDiscovery? = null
    @Volatile private var groupOwnerAddress: String? = null
    // The owner's proved address while another stream holds it and this
    // client's own dial is parked; see [scheduleReconnect].
    @Volatile private var heldOwnerAddress: String? = null
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

    override fun isAvailable(): Boolean = p2pAllowed() || hasWifi()

    /** Wi-Fi P2P is usable. The LAN carrier needs none of its permissions. */
    private fun p2pAllowed(): Boolean = wifiP2pManager != null && hasRequiredPermissions()

    private fun hasWifi(): Boolean =
        context.packageManager.hasSystemFeature(PackageManager.FEATURE_WIFI)

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
            throw TransportException.NotAvailable("Neither Wi-Fi P2P nor Wi-Fi is available on this device")
        }
        val p2p = p2pAllowed()

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
        if (p2p) {
            channel = wifiP2pManager?.initialize(context, confinement.looper) {
                emitDiagnostic("warning", "WiFi P2P channel disconnected")
            }
        } else {
            emitDiagnostic("warning", "Wi-Fi P2P unavailable or not permitted: LAN only")
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

        if (!p2p) {
            // Nothing to hear: no channel to act on what it would say.
        } else if (Build.VERSION.SDK_INT >= Build.VERSION_CODES.TIRAMISU) {
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

        // Notify protocol. Wi-Fi P2P off at start arrives next as the
        // sticky state broadcast, which reports it. With no P2P the layer
        // comes up when the LAN does.
        p2pUp = p2p
        reportLayer()

        lan = LanPeerDiscovery(
            context = context,
            handler = transportHandler,
            executor = socketExecutor,
            sockets = sockets,
            port = SERVER_PORT,
            localAddress = ::localAddressOrNull,
            running = { state == TransportState.RUNNING },
            networkChanged = ::lanNetworkChanged,
            diagnostic = ::emitDiagnostic,
        ).also { it.start() }
        sockets.onLost = { address ->
            transportHandler.post {
                lan?.peerLost(address)
                ownerStreamLost(address)
            }
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
        if (channel == null) return
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

        lan?.stop()
        lan = null

        // Close all connections
        closeAllConnections()
        stopGroupFormation()
        // Forget the group, so a reconnect already posted finds nothing to
        // dial and a later start() waits for its own CONNECTION_CHANGED.
        isGroupOwner = false
        groupOwnerAddress = null
        heldOwnerAddress = null
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
        p2pUp = false
        lanUp = false
        lanAddresses = emptySet()
        layerUp = false
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
            lan?.pause()
        }
    }

    override fun resume() {
        confinement.runSync {
            isPaused = false
            if (state == TransportState.RUNNING) {
                lan?.resume()
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
        // No channel: Wi-Fi P2P was unusable at start, and every call would throw.
        if (channel == null) return
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
        if (channel == null) return
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
                // re-adds a neighbour the flip just cleared. LAN streams do
                // not ride P2P and stay.
                sockets.closeCarrier(PeerStreamSockets.Carrier.P2P)
                p2pUp = false
                reportLayer()
            }
        } else if (enabled && state == TransportState.RUNNING) {
            // Wi-Fi P2P came back (Wi-Fi turned on, or the app started with it
            // off). Nothing else undoes the branch above: the core kept the
            // slot down, discovery had stopped, and the framework had dropped
            // every local service and service request with the P2P state.
            // Posted for the same reason as above; the flag makes a broadcast
            // that repeats the current state a no-op.
            transportHandler.post {
                if (state != TransportState.RUNNING || p2pUp) return@post
                p2pUp = true
                reportLayer()
                startPeerDiscovery()
                resumeGroupFormationAfterP2pReturned()
            }
        }
    }

    @SuppressLint("MissingPermission")
    private fun handlePeersChanged() {
        if (!hasRequiredPermissions()) return

        wifiP2pManager?.requestPeers(channel) { peers ->
            val deviceList = peers?.deviceList ?: return@requestPeers

            val owners = deviceList.filter { it.isGroupOwner }.map { it.deviceAddress }.toSet()
            // A new owner may be this application's group: probe it at once.
            // Not one that only flickered out of the list and back.
            val now = SystemClock.elapsedRealtime()
            if (WifiDirectGroupFormation.ownersNewlySeen(owners - deadOwners.keys, ownerLastSeenMs, now).isNotEmpty()) {
                failedProbes = 0
            }
            owners.forEach { ownerLastSeenMs[it] = now }
            ownerLastSeenMs.values.removeAll { now - it >= WifiDirectGroupFormation.OWNER_MEMORY_MS }
            nearbyOwners = owners
            // A client leaving changes the peer list, and may not change the
            // connection: recount, or an owner whose last client left stays
            // "settled" and never dissolves its empty group.
            if (inGroup && isGroupOwner) refreshGroupForFormation()

            // Update discovered peers
            val currentPeers = mutableSetOf<String>()
            for (device in deviceList) {
                currentPeers.add(device.deviceAddress)
                if (!discoveredPeers.containsKey(device.deviceAddress)) {
                    discoveredPeers[device.deviceAddress] = device
                    // Someone new is in range: ask for records now, and at
                    // full rate after.
                    quietDiscoveryRounds = 0
                    nextServiceDiscoveryAtMs = 0L
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
        if (connected) {
            transportHandler.removeCallbacks(joinWindowTimeout)
            formationBackoffMs = FORMATION_RETRY_MS
            // The join counters are settled in refreshGroupForFormation, once
            // the group's owner is known: a group owned by a device this one
            // left is a join that found no group, and resetting here would
            // zero the count every time the dead owner captured the device.
        }

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
            heldOwnerAddress = null
            ownerClients = 0
            createdNetwork = null
            ownsFormationGroup = false
            inAppGroup = false
            appGroupOwnerDevice = null
            unprovedAtCeiling.set(0)
            ownerIdleSinceMs = 0L
            // Posted: this is the broadcast receiver's onReceive, and ending
            // an announced stream is an FFI call. Same reasoning as the post
            // in [handleWifiP2pStateChanged]. Leaving the group ends only the
            // group's streams.
            transportHandler.post { sockets.closeCarrier(PeerStreamSockets.Carrier.P2P) }
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
        if (!formGroups || channel == null) return
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
                heardSinceLastDiscovery = true
                quietDiscoveryRounds = 0
                // A device left for proving nothing whose record is heard
                // again has its application back: its stale group is being
                // dissolved, or carries a listener again.
                if (deadOwners.remove(device.deviceAddress) != null) {
                    emitDiagnostic("debug", "Wi-Fi Direct owner left earlier is back", mapOf("address" to advert.address))
                }
                if (adverts.put(device.deviceAddress, advert) == null) {
                    // A new peer: the next round is due now, not at the end
                    // of a period backed off while nothing was heard.
                    nextServiceDiscoveryAtMs = 0L
                    emitDiagnostic("debug", "Wi-Fi Direct peer record", mapOf("address" to advert.address))
                }
            },
        )
        formationRegistered = true
        heardSinceLastDiscovery = true
        quietDiscoveryRounds = 0
        addServiceRequest()
        publishAdvert()
        // The first step runs at once and the publish above has not answered
        // yet: without this it publishes again, and the two clear-and-add
        // chains interleave.
        nextPublishAtMs = SystemClock.elapsedRealtime() + FORMATION_RETRY_MS
        transportHandler.removeCallbacks(formationTick)
        transportHandler.post(formationTick)
        emitDiagnostic("info", "Wi-Fi Direct group formation started")
    }

    /**
     * A TXT query for the one instance name every device of this protocol
     * publishes. A query by service type alone asks only for the PTR record,
     * whose answer reaches the service listener but never the TXT listener, and
     * the TXT record is where the address and the group name are.
     */
    @SuppressLint("MissingPermission")
    private fun addServiceRequest() {
        wifiP2pManager?.addServiceRequest(
            channel,
            WifiP2pDnsSdServiceRequest.newInstance(
                WifiDirectGroupFormation.INSTANCE_NAME,
                WifiDirectGroupFormation.SERVICE_TYPE,
            ),
            quietListener("add service request"),
        )
    }

    /** The framework drops local services and service requests with P2P. */
    private fun resumeGroupFormationAfterP2pReturned() {
        if (!formGroups || !formationSupported) return
        addServiceRequest()
        advertPublished = false
        nextPublishAtMs = 0L
        nextServiceDiscoveryAtMs = 0L
        quietDiscoveryRounds = 0
        heardSinceLastDiscovery = true
        emitDiagnostic("info", "Wi-Fi Direct group formation resumed")
    }

    /**
     * Undoes what this run did, decided by what it did and never by
     * [formGroups]: the flag can change between [start] and here, and a check
     * on it once left a created group up (and the device hidden behind it)
     * after formation was switched off.
     */
    @SuppressLint("MissingPermission")
    private fun stopGroupFormation() {
        transportHandler.removeCallbacks(formationTick)
        // A join timeout from this run would otherwise land on the next run's
        // channel, count a failure it did not see, and cancel its join.
        transportHandler.removeCallbacks(joinWindowTimeout)
        wifiP2pManager?.let { p2p ->
            if (formationRegistered) {
                p2p.clearServiceRequests(channel, quietListener("clear service requests"))
                p2p.clearLocalServices(channel, quietListener("clear local services"))
            }
            // A group this manager made goes with it. One the system formed, or
            // one adopted at start, is not this run's to remove.
            if (createdNetwork != null) p2p.removeGroup(channel, quietListener("remove group"))
        }
        formationRegistered = false
        createdNetwork = null
        ownsFormationGroup = false
        inAppGroup = false
        appGroupOwnerDevice = null
        unprovedAtCeiling.set(0)
        ownerIdleSinceMs = 0L
        adverts.clear()
        inGroup = false
        ownerClients = 0
        advertPublished = false
        nextPublishAtMs = 0L
        failedJoins = 0
        joinFirstRemaining = 0
        joinIsProbe = false
        failedProbes = 0
        quietDiscoveryRounds = 0
        heardSinceLastDiscovery = true
        nearbyOwners = emptySet()
        ownerLastSeenMs.clear()
    }

    /** One step: refresh what is known, and act on it if the backoff allows. */
    @SuppressLint("MissingPermission")
    private fun formationStep() {
        val p2p = wifiP2pManager ?: return
        // Nothing to form over while Wi-Fi P2P is off: every call would fail
        // BUSY, and a join attempted then is refused with no group to find.
        if (!p2pUp) return
        val now = SystemClock.elapsedRealtime()
        adverts.values.removeAll { now - it.seenAtMs > ADVERT_TTL_MS }
        deadOwners.values.removeAll { now - it >= WifiDirectGroupFormation.DEAD_OWNER_TTL_MS }
        val address = localAddressOrNull() ?: return

        // Paced like a formation retry: a framework that keeps refusing the
        // record is not asked again on every step.
        if (!advertPublished && now >= nextPublishAtMs) {
            nextPublishAtMs = now + FORMATION_RETRY_MS
            publishAdvert()
        }

        if (inGroup && isGroupOwner && ownsFormationGroup && ownerClients == 0) {
            if (ownerIdleSinceMs == 0L) {
                ownerIdleSinceMs = now
                ownerIdleLimitMs = OWNER_IDLE_MS + (Math.random() * OWNER_IDLE_MS).toLong()
            } else if (now - ownerIdleSinceMs > ownerIdleLimitMs) {
                ownerIdleSinceMs = 0L
                emitDiagnostic("info", "Dissolving an empty Wi-Fi Direct group")
                p2p.removeGroup(channel, quietListener("remove empty group"))
                // Join before creating again: another owner's group may be up.
                failedJoins = 0
                joinFirstRemaining = 2
                nextFormationAtMs = now + FORMATION_TICK_MS
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
            // The round that just ended heard nothing: wait longer next time.
            if (!heardSinceLastDiscovery) quietDiscoveryRounds++
            heardSinceLastDiscovery = false
            nextServiceDiscoveryAtMs =
                now + WifiDirectGroupFormation.discoveryPeriodMs(quietDiscoveryRounds)
            p2p.discoverServices(channel, object : WifiP2pManager.ActionListener {
                override fun onSuccess() {}
                override fun onFailure(reason: Int) {
                    // The request was lost (P2P went off and on between our
                    // broadcasts, say): register it again for the next round.
                    if (reason == WifiP2pManager.NO_SERVICE_REQUESTS) addServiceRequest()
                    emitDiagnostic("debug", "Wi-Fi Direct discover services failed", mapOf(
                        "reason" to reasonToString(reason)
                    ))
                }
            })
        }
        if (now < nextFormationAtMs) return
        // An owner this device left is not "an owner nearby": probing it is
        // how the device was captured again.
        val liveOwners = nearbyOwners - deadOwners.keys
        val action = WifiDirectGroupFormation.decide(
            WifiDirectGroupFormation.Local(
                address, appTag, inGroup, failedJoins, liveOwners.isNotEmpty(),
                ownerProbeDue = now - lastJoinAtMs >=
                    WifiDirectGroupFormation.probePeriodMs(failedProbes),
                joinFirst = joinFirstRemaining > 0,
            ),
            adverts.values,
        )
        when (action) {
            WifiDirectGroupFormation.Action.Wait -> return
            WifiDirectGroupFormation.Action.Create -> createGroup(appNetwork)
            WifiDirectGroupFormation.Action.Join -> {
                lastJoinAtMs = now
                // A join with no record of this application heard is a probe
                // toward an owner nobody vouched for (rule 4).
                joinIsProbe = adverts.values.none { it.app == appTag && it.address != address }
                // By name only. Which owner it lands on is the supplicant's
                // choice; one this device left is handled on arrival, in
                // refreshGroupForFormation (see WifiDirectGroupFormation).
                joinGroup(appNetwork)
            }
        }
        nextFormationAtMs = now + formationBackoffMs
    }

    /**
     * The application's group, for creating it and for joining it alike.
     *
     * Never with `setDeviceAddress`: on a join by credentials the supplicant
     * takes it as the owner's BSSID (its P2P interface address), while every
     * address an application sees is the owner's device address, so a join
     * named that way matches no network and fails.
     */
    private fun formationConfig(network: String): WifiP2pConfig? {
        // In-function, like the callers', so lint's NewApi check sees it.
        if (Build.VERSION.SDK_INT < Build.VERSION_CODES.Q) return null
        return WifiP2pConfig.Builder()
            .setNetworkName(network)
            .setPassphrase(WifiDirectGroupFormation.passphrase(appId, network))
            .enablePersistentMode(false)
            .build()
    }

    @SuppressLint("MissingPermission")
    private fun createGroup(network: String) {
        // Here and not only in the caller, so lint's NewApi check sees it.
        if (Build.VERSION.SDK_INT < Build.VERSION_CODES.Q) return
        val config = formationConfig(network) ?: return
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
        val config = formationConfig(network) ?: return
        val ch = channel ?: return
        emitDiagnostic("info", "Joining a Wi-Fi Direct group", mapOf("network" to network))
        wifiP2pManager?.connect(ch, config, object : WifiP2pManager.ActionListener {
            override fun onSuccess() {
                transportHandler.removeCallbacks(joinWindowTimeout)
                transportHandler.postDelayed(joinWindowTimeout, JOIN_WINDOW_MS)
            }
            override fun onFailure(reason: Int) {
                // A join the framework refused outright found no group either.
                // Counted here as well as on the timeout, or a device whose
                // joins keep failing BUSY never reaches the takeover.
                joinFoundNoGroup()
                formationFailed("join group", reason)
            }
        })
    }

    // A named field, not a lambda per join, so stopGroupFormation can remove
    // it. See JOIN_WINDOW_MS.
    private val joinWindowTimeout = Runnable {
        if (state == TransportState.RUNNING && !inGroup) {
            joinFoundNoGroup()
            wifiP2pManager?.cancelConnect(channel, quietListener("cancel join"))
        }
    }

    /** A join ended without a group, by timeout or by refusal. */
    private fun joinFoundNoGroup() {
        failedJoins++
        if (joinFirstRemaining > 0) joinFirstRemaining--
        if (joinIsProbe) failedProbes++
        joinIsProbe = false
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
     * After any group change: count this owner's clients, and note whether the
     * group is the application's, the only kind formation ever dissolves or
     * leaves.
     *
     * An application's group this device owns is this application's to
     * remove on [stop], even one adopted at start rather than created by this
     * run: no other application derives the name, and the group can outlive
     * the process (the framework removes it only when no application on the
     * phone still holds Wi-Fi P2P). Left up after the process that served it
     * died, it is an owner with no listener that answers no service discovery
     * query, and every device of the application that probes it joins a group
     * that carries nothing.
     *
     * Also where a join's counters are settled, because only here is the
     * owner known. A client that landed in the group of an owner it left for
     * proving nothing leaves at once, and the join counts as one that found
     * no group, so the takeover still arrives; any other group starts the
     * counters over.
     */
    @SuppressLint("MissingPermission")
    private fun refreshGroupForFormation() {
        if (!formGroups || !formationSupported) return
        wifiP2pManager?.requestGroupInfo(channel) { group ->
            inAppGroup = group?.networkName == appNetwork
            var leftOwner = false
            if (isGroupOwner) {
                ownerClients = group?.clientList?.size ?: 0
                ownsFormationGroup = inAppGroup
                if (ownsFormationGroup && createdNetwork == null) createdNetwork = appNetwork
            } else {
                ownerClients = 0
                ownsFormationGroup = false
                appGroupOwnerDevice = if (inAppGroup) group?.owner?.deviceAddress else null
                leftOwner = WifiDirectGroupFormation.joinedLeftOwner(inAppGroup, appGroupOwnerDevice, deadOwners.keys)
            }
            failedJoins = WifiDirectGroupFormation.failedJoinsAfterGroupJoined(failedJoins, leftOwner)
            if (leftOwner) {
                if (joinFirstRemaining > 0) joinFirstRemaining--
                joinIsProbe = false
                emitDiagnostic("info", "Wi-Fi Direct join landed on an owner left for proving nothing", mapOf(
                    "owner" to appGroupOwnerDevice,
                    "failedJoins" to failedJoins,
                ))
                leaveDeadApplicationGroup()
            } else {
                joinFirstRemaining = 0
                failedProbes = 0
            }
        }
    }

    /**
     * Leaves an application's group whose owner proves nothing, so formation
     * decides again, and remembers the owner (see [WifiDirectGroupFormation],
     * "An owner that was left is never stayed with"). Runs on the transport
     * thread; re-reads everything, because the group can change between the
     * socket thread's decision and here.
     *
     * Not join-first: the only owner of the application's name in sight is
     * usually the one just left. The failed-join count is left alone: a
     * capture by a remembered owner has just counted toward the takeover.
     * If the removal fails the client is still in the group, the redial goes
     * on, and its next unproved dial at the ceiling asks again.
     */
    @SuppressLint("MissingPermission")
    private fun leaveDeadApplicationGroup() {
        if (state != TransportState.RUNNING || !formationRegistered) return
        if (!inGroup || isGroupOwner || !inAppGroup) return
        val now = SystemClock.elapsedRealtime()
        val owner = appGroupOwnerDevice
        if (owner != null) deadOwners[owner] = now
        emitDiagnostic("info", "Leaving a Wi-Fi Direct group whose owner proves nothing", mapOf(
            "owner" to (owner ?: "unknown"),
        ))
        wifiP2pManager?.removeGroup(channel, object : WifiP2pManager.ActionListener {
            override fun onSuccess() {}
            override fun onFailure(reason: Int) {
                emitDiagnostic("warning", "Wi-Fi Direct leave of a dead group failed; retried on the next dial", mapOf(
                    "reason" to reasonToString(reason),
                ))
            }
        })
        nextFormationAtMs = now + FORMATION_TICK_MS
    }

    @SuppressLint("MissingPermission")
    private fun publishAdvert() {
        if (!formGroups || !formationSupported) return
        val p2p = wifiP2pManager ?: return
        val address = localAddressOrNull() ?: return
        val info = WifiP2pDnsSdServiceInfo.newInstance(
            WifiDirectGroupFormation.INSTANCE_NAME,
            WifiDirectGroupFormation.SERVICE_TYPE,
            WifiDirectGroupFormation.txtRecord(address, appTag),
        )
        val added = object : WifiP2pManager.ActionListener {
            override fun onSuccess() {
                advertPublished = true
                emitDiagnostic("debug", "Wi-Fi Direct record published")
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
            val carrier = if (socket.localAddress in lanAddresses) {
                PeerStreamSockets.Carrier.LAN
            } else {
                PeerStreamSockets.Carrier.P2P
            }
            sockets.run(socket, outbound = false, carrier) { state == TransportState.RUNNING }
        }
    }

    private fun connectToGroupOwner(address: String) {
        // One outbound socket at a time. CONNECTION_CHANGED fires for every
        // change to the group, not only for our joining it, and each used to
        // open another socket to the same owner.
        if (!outboundOpen.compareAndSet(false, true)) return
        socketExecutor.execute {
            var ran = PeerStreamSockets.Ran(proved = null, heard = false)
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
                ran = sockets.run(socket, outbound = true) { state == TransportState.RUNNING }
            } finally {
                outboundOpen.set(false)
                scheduleReconnect(address, ran)
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
    private fun scheduleReconnect(ended: String, ran: PeerStreamSockets.Ran) {
        // Another stream (a LAN one, say) holds the owner's address: this
        // dial was refused for it, or superseded by it. The owner is alive
        // and reachable, so there is nothing to redial until that stream is
        // lost ([ownerStreamLost]).
        val held = ran.proved?.let { sockets.holds(it) } == true
        if (held) heldOwnerAddress = ran.proved
        val unproved = unprovedAtCeiling.updateAndGet {
            GroupOwnerRedial.unprovedAtCeilingAfter(
                count = it,
                proved = ran.proved != null,
                sameOwner = groupOwnerAddress == ended,
                currentDelayMs = reconnectDelayMs.get(),
                maxDelayMs = RECONNECT_MAX_DELAY_MS,
            )
        }
        // The redial below goes on after a leave is asked for: the leave can
        // fail (BUSY during a group transition), and a client in a group with
        // no redial posted has nothing left that would ever fire. A leave that
        // succeeds ends the group, resets the count, and the redial finds no
        // owner to dial.
        if (!isGroupOwner && GroupOwnerRedial.shouldLeave(unproved, applicationGroup = inAppGroup)) {
            transportHandler.post { leaveDeadApplicationGroup() }
        }
        val plan = GroupOwnerRedial.next(
            ended = ended,
            delivered = ran.delivered,
            held = held,
            running = state == TransportState.RUNNING,
            isGroupOwner = isGroupOwner,
            owner = groupOwnerAddress,
            currentDelayMs = reconnectDelayMs.get(),
            initialDelayMs = RECONNECT_INITIAL_DELAY_MS,
            maxDelayMs = RECONNECT_MAX_DELAY_MS,
        )
        if (plan == null) {
            if (ran.delivered) reconnectDelayMs.set(RECONNECT_INITIAL_DELAY_MS)
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

    /**
     * An announced stream to [address] ended. If it was the one that held the
     * group owner's address when this client's own dial was refused, the
     * client dials its owner again. Transport thread.
     */
    private fun ownerStreamLost(address: String) {
        if (address != heldOwnerAddress) return
        heldOwnerAddress = null
        val owner = groupOwnerAddress
        if (state == TransportState.RUNNING && !isGroupOwner && owner != null) {
            connectToGroupOwner(owner)
        }
    }

    /**
     * The LAN carrier's Wi-Fi network came up ([addresses] its own) or went
     * (null). Going ends the LAN streams before the layer is reported, so
     * each announced peer is reported lost while the core still holds it.
     */
    private fun lanNetworkChanged(addresses: Set<InetAddress>?) {
        if (state != TransportState.RUNNING) return
        if (addresses == null) {
            lanAddresses = emptySet()
            sockets.closeCarrier(PeerStreamSockets.Carrier.LAN)
            lanUp = false
        } else {
            lanAddresses = addresses
            lanUp = true
        }
        reportLayer()
    }

    /**
     * Tells the core the stream layer is up while either carrier is, once
     * per edge. Runs on the transport thread: it is an FFI call.
     */
    private fun reportLayer() {
        val up = p2pUp || lanUp
        if (up == layerUp) return
        layerUp = up
        try {
            protocol.wifiDirectStatusChanged(up)
        } catch (e: Exception) {
            Log.e(TAG, "Error notifying protocol", e)
        }
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
            WifiP2pManager.NO_SERVICE_REQUESTS -> "NO_SERVICE_REQUESTS"
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

    /**
     * Dials in a row at the ladder's ceiling, each proving nothing, after
     * which a client leaves a group under the application's name. The group
     * can outlive the process that owned it, so an owner whose application died
     * holds its clients in a group that carries nothing, and a client in a
     * group never runs formation again. Three at the ceiling is about three
     * minutes past the ladder's climb.
     */
    const val LEAVE_AFTER_UNPROVED_AT_CEILING = 3

    /**
     * The count of unproved dials at the ceiling after one more dial ended.
     * A proved stream or a different owner starts it over; a dial below the
     * ceiling leaves it alone, so an owner restarting its application is not
     * mistaken for a dead one.
     */
    fun unprovedAtCeilingAfter(
        count: Int,
        proved: Boolean,
        sameOwner: Boolean,
        currentDelayMs: Long,
        maxDelayMs: Long,
    ): Int = when {
        proved || !sameOwner -> 0
        currentDelayMs >= maxDelayMs -> count + 1
        else -> count
    }

    /**
     * Whether a client leaves its group. Only a group under the application's
     * name: one paired in the system settings is the user's, not ours.
     */
    fun shouldLeave(unprovedAtCeiling: Int, applicationGroup: Boolean): Boolean =
        applicationGroup && unprovedAtCeiling >= LEAVE_AFTER_UNPROVED_AT_CEILING

    /**
     * The next dial toward the owner, or null for none. [delivered] is
     * whether the stream that ended carried the owner (a body after the
     * preamble), which starts the ladder over; a proof alone does not, since
     * a stream the owner refused proves it too. [held] is whether another
     * stream holds the owner's address now: then nothing is redialed.
     */
    fun next(
        ended: String,
        delivered: Boolean,
        held: Boolean,
        running: Boolean,
        isGroupOwner: Boolean,
        owner: String?,
        currentDelayMs: Long,
        initialDelayMs: Long,
        maxDelayMs: Long,
    ): Plan? {
        if (!running || isGroupOwner || owner == null || held) return null
        val delay = if (delivered || owner != ended) initialDelayMs else currentDelayMs
        return Plan(owner, delay, minOf(delay * 2, maxDelayMs))
    }
}
