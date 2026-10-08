package com.offlineprotocol

import android.content.Context
import android.content.pm.PackageManager
import android.net.ConnectivityManager
import android.net.LinkProperties
import android.net.Network
import android.net.NetworkCapabilities
import android.net.NetworkRequest
import android.net.nsd.NsdManager
import android.net.nsd.NsdServiceInfo
import android.net.wifi.WifiManager
import android.os.Build
import android.os.Handler
import android.os.ext.SdkExtensions
import androidx.core.content.ContextCompat
import java.net.InetAddress
import java.net.InetSocketAddress
import java.net.Socket
import java.util.concurrent.ExecutorService
import java.util.concurrent.RejectedExecutionException

/**
 * The peer-stream slot's LAN carrier: DNS-SD `_offlineprotocol._tcp` on the
 * Wi-Fi network this device is on, so it finds iPhones and Python hosts there
 * and they find it (ADR 0028, stream-framing.md "Finding a peer on a LAN").
 *
 * It publishes the record iOS and Python publish (`txtvers=1`, `addr`, the
 * same instance name), browses for theirs, and dials what it finds on the
 * policy iOS dials on ([PeerStreamDialPolicy]): the lower address at once,
 * the higher after a grace delay, so the stream both ends keep usually
 * arrives first. Inbound LAN streams arrive at [WifiDirectManager]'s
 * listener, which already binds every interface. What a stream does once
 * open is [PeerStreamSockets]'s, the same as for a group stream.
 *
 * A record's `addr` is a hint: a dial passes it as the preamble's expected
 * address, and a record whose dial is answered by another address is left
 * out until it is published again.
 *
 * Every method and every framework callback runs on [handler]'s thread
 * (the transport thread); connects and stream reads run on [executor],
 * because nothing that blocks on the network may run on the transport
 * thread (TransportConfinement).
 */
internal class LanPeerDiscovery(
    private val context: Context,
    private val handler: Handler,
    private val executor: ExecutorService,
    private val sockets: PeerStreamSockets,
    private val port: Int,
    private val localAddress: () -> String?,
    /** The transport is running: what a stream's `accepting` asks. */
    private val running: () -> Boolean,
    /** The Wi-Fi network's own addresses when it comes up, null when it goes. */
    private val networkChanged: (Set<InetAddress>?) -> Unit,
    private val diagnostic: (level: String, message: String, context: Map<String, Any?>) -> Unit,
) {
    private data class Record(val address: String, val host: InetAddress, val port: Int)

    private val connectivity = context.getSystemService(ConnectivityManager::class.java)
    private val nsd = context.getSystemService(NsdManager::class.java)

    private var started = false
    private var paused = false
    /** The matching Wi-Fi networks and the one followed ([NetworkChoice]). */
    private val networks = NetworkChoice<Network, LinkProperties>()
    private val network: Network? get() = networks.current
    /** Bumped whenever what was found stops counting, so stale callbacks and timers do nothing. */
    private var generation = 0

    private var registration: NsdManager.RegistrationListener? = null
    /** Our record's name as registered (NSD renames on a conflict). */
    private var registeredName: String? = null
    private var discovery: NsdManager.DiscoveryListener? = null

    /** Resolved peer records by instance name. */
    private val records = HashMap<String, Record>()
    /** The instance name each advertised address is dialed at. */
    private var adverts: Map<String, String> = emptyMap()
    private val unprovable = HashSet<String>()
    private val policy = PeerStreamDialPolicy()

    /** What NSD reports, and its resolves, one at a time ([ResolveQueue]). */
    private val resolves = ResolveQueue<NsdServiceInfo>({ it.serviceName })

    // Held while the advert or the browse is active. Before T extensions 7
    // (Android 12 and lower, and 13 without that update) the Wi-Fi driver
    // drops the multicast mDNS runs on unless an app holds one, so this
    // device would neither hear a peer's answers nor the queries a peer
    // sends to find it (NsdManager's own documentation). The advert needs it
    // as much as the browse: a lock that followed the browse alone was let
    // go on pause and during a browse retry, while the record stayed
    // published and could not be found. From extension 7 the system manages
    // it for a foreground app, and a lock only costs battery, so none is
    // taken.
    private val multicastLock: WifiManager.MulticastLock? =
        if (needsMulticastLock()) {
            context.applicationContext.getSystemService(WifiManager::class.java)
                ?.createMulticastLock("offlineprotocol-lan")
                ?.apply { setReferenceCounted(false) }
        } else {
            null
        }

    private val networkCallback = object : ConnectivityManager.NetworkCallback() {
        override fun onAvailable(network: Network) {
            val properties = try { connectivity.getLinkProperties(network) } catch (_: Exception) { null }
            if (properties != null) handler.post { networkUp(network, properties) }
        }

        override fun onLinkPropertiesChanged(network: Network, properties: LinkProperties) {
            handler.post { networkUp(network, properties) }
        }

        override fun onLost(network: Network) {
            handler.post { networkLost(network) }
        }
    }

    fun start() {
        if (started) return
        if (!localNetworkPermitted()) {
            // Off rather than half on: without the grant NsdManager shows a
            // system service picker on every browse, and sockets to the LAN
            // are refused. A later grant takes effect on the next start.
            diagnostic("warning", "LAN carrier off: ACCESS_LOCAL_NETWORK not granted", emptyMap())
            return
        }
        started = true
        // Without INTERNET removed, the default request skips a Wi-Fi network
        // with no route out, which is exactly the offline LAN this is for.
        val request = NetworkRequest.Builder()
            .addTransportType(NetworkCapabilities.TRANSPORT_WIFI)
            .removeCapability(NetworkCapabilities.NET_CAPABILITY_INTERNET)
            .build()
        try {
            connectivity.registerNetworkCallback(request, networkCallback)
        } catch (e: Exception) {
            diagnostic("error", "LAN discovery could not watch Wi-Fi", mapOf(
                "error" to (e.message ?: e.javaClass.simpleName),
            ))
        }
    }

    /** Ends discovery and advertising. The owner ends the LAN streams. */
    fun stop() {
        if (!started) return
        started = false
        try { connectivity.unregisterNetworkCallback(networkCallback) } catch (_: Exception) {}
        networks.clear()
        stopNsd()
    }

    /** Stops browsing and dialing; the record stays published. */
    fun pause() {
        paused = true
        stopDiscovery()
        // No loss reports arrive while not browsing, so what was found is
        // forgotten and found again on resume.
        forgetRecords()
    }

    fun resume() {
        paused = false
        if (network != null) discover()
    }

    // MARK: - The network

    /** A matching network came up or changed; [NetworkChoice] decides what it means. */
    private fun networkUp(network: Network, properties: LinkProperties) {
        if (!started) return
        when (networks.up(network, properties)) {
            NetworkChoice.Up.ADOPT -> adopt(properties)
            NetworkChoice.Up.UPDATE -> networkChanged(properties.linkAddresses.map { it.address }.toSet())
            NetworkChoice.Up.WAIT -> {}
        }
    }

    private fun networkLost(network: Network) {
        if (!networks.lost(network)) return
        stopNsd()
        networkChanged(null)
        networks.fallback()?.let { (_, properties) -> adopt(properties) }
    }

    /** The network now followed ([networks]`.current`) starts the carrier. */
    private fun adopt(properties: LinkProperties) {
        networkChanged(properties.linkAddresses.map { it.address }.toSet())
        startNsd()
    }

    // MARK: - NSD

    private fun startNsd() {
        generation++
        val address = localAddress()
        if (address == null) {
            diagnostic("warning", "LAN advert skipped: no identity yet", emptyMap())
        } else {
            register(address)
        }
        if (!paused) discover()
    }

    private fun stopNsd() {
        generation++
        registration?.let { try { nsd.unregisterService(it) } catch (_: Exception) {} }
        registration = null
        registeredName = null
        stopDiscovery()
        forgetRecords()
        policy.reset()
    }

    private fun forgetRecords() {
        resolves.clear()
        records.clear()
        adverts = emptyMap()
        unprovable.clear()
    }

    private fun register(address: String) {
        // Order is the framework's: NsdServiceInfo keeps attributes in a map,
        // so `txtvers` may not come first. Every reader looks entries up by key.
        val info = NsdServiceInfo().apply {
            serviceName = instanceName(address)
            serviceType = WifiDirectGroupFormation.SERVICE_TYPE
            port = this@LanPeerDiscovery.port
            setAttribute(WifiDirectGroupFormation.KEY_VERSION, "1")
            setAttribute(WifiDirectGroupFormation.KEY_ADDRESS, address)
            if (Build.VERSION.SDK_INT >= Build.VERSION_CODES.TIRAMISU) {
                setNetwork(this@LanPeerDiscovery.network)
            }
        }
        val listener = object : NsdManager.RegistrationListener {
            override fun onServiceRegistered(info: NsdServiceInfo) {
                handler.post { if (registration === this) registeredName = info.serviceName }
            }

            override fun onRegistrationFailed(info: NsdServiceInfo, code: Int) {
                handler.post { if (registration === this) registrationFailed(mapOf("code" to code)) }
            }

            override fun onServiceUnregistered(info: NsdServiceInfo) {}
            override fun onUnregistrationFailed(info: NsdServiceInfo, code: Int) {}
        }
        registration = listener
        acquireMulticastLock()
        try {
            nsd.registerService(info, NsdManager.PROTOCOL_DNS_SD, listener)
        } catch (e: Exception) {
            registrationFailed(mapOf("error" to (e.message ?: e.javaClass.simpleName)))
        }
    }

    /**
     * The advert failed: tried again later, as iOS rebuilds its listener. A
     * failure was final until the network changed, which left this device
     * invisible to every peer on it for the session.
     */
    private fun registrationFailed(context: Map<String, Any?>) {
        registration = null
        releaseMulticastLockIfIdle()
        diagnostic("error", "LAN advert failed", context)
        retryLater { if (network != null && registration == null) localAddress()?.let { register(it) } }
    }

    private fun discover() {
        if (discovery != null) return
        acquireMulticastLock()
        val listener = object : NsdManager.DiscoveryListener {
            override fun onDiscoveryStarted(serviceType: String) {}
            override fun onDiscoveryStopped(serviceType: String) {}

            override fun onStartDiscoveryFailed(serviceType: String, code: Int) {
                handler.post { if (discovery === this) discoveryFailed(mapOf("code" to code)) }
            }

            override fun onStopDiscoveryFailed(serviceType: String, code: Int) {}

            override fun onServiceFound(info: NsdServiceInfo) {
                handler.post { if (discovery === this) found(info) }
            }

            override fun onServiceLost(info: NsdServiceInfo) {
                handler.post { if (discovery === this) lost(info) }
            }
        }
        discovery = listener
        try {
            val network = network
            if (Build.VERSION.SDK_INT >= Build.VERSION_CODES.TIRAMISU && network != null) {
                nsd.discoverServices(
                    WifiDirectGroupFormation.SERVICE_TYPE, NsdManager.PROTOCOL_DNS_SD,
                    network, { it.run() }, listener,
                )
            } else {
                nsd.discoverServices(WifiDirectGroupFormation.SERVICE_TYPE, NsdManager.PROTOCOL_DNS_SD, listener)
            }
        } catch (e: Exception) {
            discoveryFailed(mapOf("error" to (e.message ?: e.javaClass.simpleName)))
        }
    }

    /**
     * The browse failed to start: tried again later, as iOS rebuilds its
     * browser. A failure was final until the network changed. The lock is
     * let go only if no advert needs it either.
     */
    private fun discoveryFailed(context: Map<String, Any?>) {
        discovery = null
        releaseMulticastLockIfIdle()
        diagnostic("error", "LAN discovery failed to start", context)
        retryLater { if (!paused && network != null) discover() }
    }

    /** Runs [retry] after [REBUILD_DELAY_MS] unless the NSD state it was for has gone. */
    private fun retryLater(retry: () -> Unit) {
        val expected = generation
        handler.postDelayed({ if (started && expected == generation) retry() }, REBUILD_DELAY_MS)
    }

    private fun stopDiscovery() {
        discovery?.let { try { nsd.stopServiceDiscovery(it) } catch (_: Exception) {} }
        discovery = null
        releaseMulticastLockIfIdle()
    }

    private fun acquireMulticastLock() {
        try { multicastLock?.acquire() } catch (_: Exception) {}
    }

    /** Lets the lock go once neither the advert nor the browse needs it. */
    private fun releaseMulticastLockIfIdle() {
        if (registration != null || discovery != null) return
        try { multicastLock?.let { if (it.isHeld) it.release() } } catch (_: Exception) {}
    }

    /**
     * Whether [info] was found on the Wi-Fi network dials are bound to. The
     * same instance can be reported on more than one interface (a Wi-Fi
     * Direct group, a tethering downstream with no Network at all), and
     * records are keyed by name: a copy from elsewhere overwrote the Wi-Fi
     * record with a host no bound dial reaches, and its loss deleted it.
     * From API 33 the advert and the browse are scoped to the network too.
     * Below it NSD carries no network, every interface is browsed, and a
     * record from another one fails its dial and climbs the ladder.
     */
    private fun onOurNetwork(info: NsdServiceInfo): Boolean =
        Build.VERSION.SDK_INT < Build.VERSION_CODES.TIRAMISU || info.network == network

    private fun found(info: NsdServiceInfo) {
        if (!onOurNetwork(info)) return
        if (info.serviceName == registeredName) return
        resolves.found(info)
        resolveNext()
    }

    /** Resolves [name] again: Android's cache answers with its latest port and host. */
    private fun refresh(name: String) {
        resolves.queue(name)
        resolveNext()
    }

    private fun lost(info: NsdServiceInfo) {
        if (!onOurNetwork(info)) return
        val name = info.serviceName
        resolves.lost(name)
        if (records.remove(name) == null) return
        unprovable.remove(name)
        rebuild(emptySet())
    }

    private fun resolveNext() {
        val ticket = resolves.next() ?: return
        val name = ticket.item.serviceName
        val listener = object : NsdManager.ResolveListener {
            override fun onServiceResolved(info: NsdServiceInfo) {
                handler.post {
                    if (resolves.answered(ticket)) resolved(info)
                    resolveNext()
                }
            }

            override fun onResolveFailed(info: NsdServiceInfo, code: Int) {
                handler.post {
                    if (code == NsdManager.FAILURE_ALREADY_ACTIVE && resolves.waitForActive(ticket)) {
                        // A resolve from before a restart is still running:
                        // this one waits for it, a few times, then gives way.
                        handler.postDelayed({ if (resolves.release(ticket)) resolveNext() }, RESOLVE_RETRY_MS)
                        return@post
                    }
                    if (resolves.gaveUp(ticket)) retryResolve(name)
                    resolveNext()
                }
            }
        }
        // NsdManager promises no answer, and one resolve that never comes back
        // held the queue for good: no peer found later was ever resolved,
        // and a refresh after a dead port waited behind it.
        handler.postDelayed({
            if (!resolves.gaveUp(ticket)) return@postDelayed
            if (Build.VERSION.SDK_INT >= Build.VERSION_CODES.UPSIDE_DOWN_CAKE) {
                try { nsd.stopServiceResolution(listener) } catch (_: Exception) {}
            }
            retryResolve(name)
            resolveNext()
        }, RESOLVE_TIMEOUT_MS)
        try {
            @Suppress("DEPRECATION")
            nsd.resolveService(ticket.item, listener)
        } catch (e: Exception) {
            resolves.gaveUp(ticket)
            diagnostic("warning", "LAN resolve failed", mapOf("error" to (e.message ?: e.javaClass.simpleName)))
            retryResolve(name)
            // Posted: an item that throws again must not recurse here.
            handler.post { resolveNext() }
        }
    }

    /**
     * A resolve of [name] gave up (failed, timed out, or gave way to one
     * still running). It is queued again later while NSD still reports the
     * service: NSD reports a service again only after losing it, so a resolve
     * dropped for good left that peer unresolved for the session, unreachable
     * if it never dials this device itself.
     */
    private fun retryResolve(name: String) {
        retryLater { refresh(name) }
    }

    private fun resolved(info: NsdServiceInfo) {
        if (!onOurNetwork(info)) return
        val address = lanAdvertAddress(info.attributes) ?: return
        val local = localAddress() ?: return
        if (address == local) return
        @Suppress("DEPRECATION")
        val host = info.host ?: return
        val name = info.serviceName
        records[name] = Record(address, host, info.port)
        // A record published again may now hold what it advertises.
        unprovable.remove(name)
        rebuild(setOf(name))
        if (adverts[address] != name) return
        val weAreLower = PeerStreamLinks.newStreamWins(outbound = true, localAddress = local, peer = address)
        policy.discovered(address, weAreLower, sockets.holds(address))?.let { schedule(address, it) }
    }

    private fun rebuild(fresh: Set<String>) {
        unprovable.retainAll(records.keys)
        adverts = PeerStreamDialPolicy.adverts(
            records.map { (name, record) -> record.address to name },
            fresh, adverts, unprovable,
        )
    }

    /**
     * An announced stream to [address] ended, on either carrier, inbound or
     * outbound. While its record is still advertised it is dialed on the
     * usual policy, unless a dial is already under way (an outbound stream's
     * own end redials through [ended]).
     *
     * Needed because this API reports no change to a record: a peer that
     * restarts and publishes the same name on a new port, with no goodbye in
     * between, is an update NsdManager's DiscoveryListener never delivers.
     * iOS hears it as a changed record and dials; here nothing would.
     */
    fun peerLost(address: String) {
        if (!started || paused || !adverts.containsKey(address)) return
        val local = localAddress() ?: return
        val weAreLower = PeerStreamLinks.newStreamWins(outbound = true, localAddress = local, peer = address)
        policy.discovered(address, weAreLower, sockets.holds(address))?.let { schedule(address, it) }
    }

    // MARK: - Dialing

    private fun schedule(address: String, delayMs: Long) {
        val expected = generation
        handler.postDelayed({ if (expected == generation) dial(address) }, delayMs)
    }

    /**
     * Opens a stream toward [address], unless it went away or is already held
     * (an inbound stream got here first), or later when no slot is free.
     */
    private fun dial(address: String) {
        val name = adverts[address]
        val record = name?.let { records[it] }
        val network = network
        if (paused || !running() || name == null || record == null || network == null || sockets.holds(address)) {
            policy.abandoned(address)
            return
        }
        if (!sockets.hasRoom()) {
            policy.noSlot(address)?.let { schedule(address, it) }
            return
        }
        val expected = generation
        try {
            executor.execute {
                val ran = connectAndRun(network, record, address)
                handler.post { if (expected == generation) ended(address, name, ran) }
            }
        } catch (_: RejectedExecutionException) {
            policy.abandoned(address)
        }
    }

    /** Runs on [executor]: the connect and the stream's whole life. */
    private fun connectAndRun(network: Network, record: Record, address: String): PeerStreamSockets.Ran {
        val socket = Socket()
        return try {
            // Bound to the Wi-Fi network: an unbound socket follows the
            // default network, which is cellular on a Wi-Fi with no internet.
            network.bindSocket(socket)
            socket.connect(InetSocketAddress(record.host, record.port), DIAL_TIMEOUT_MS)
            sockets.run(socket, outbound = true, PeerStreamSockets.Carrier.LAN, expected = address) { running() }
        } catch (e: Exception) {
            try { socket.close() } catch (_: Exception) {}
            diagnostic("info", "LAN dial failed", mapOf(
                "address" to address,
                "error" to (e.message ?: e.javaClass.simpleName),
            ))
            PeerStreamSockets.Ran(proved = null, heard = false)
        }
    }

    /**
     * An outbound stream is over. It is dialed again, later each time, while
     * its advert stays. A dial answered by a peer that proved nothing, or
     * another address, leaves its record out: a device claiming another's
     * address, or one that moved, would fail the same way on every redial
     * and keep the real record for the address undialed.
     */
    private fun ended(address: String, name: String, ran: PeerStreamSockets.Ran) {
        if (ran.proved == address && ran.carried) {
            // Carried the peer: the ladder starts over. A proof the peer then
            // refused (its stale stream holds us) climbs it like a miss, or
            // the dialer would be announced and lost every second.
            policy.proved(address)
        } else if (ran.heard && ran.proved == null && records.containsKey(name)) {
            unprovable.add(name)
            rebuild(emptySet())
        } else if (!ran.heard) {
            // Nothing answered: the peer may be back on another port, which
            // reached the cache as an update this API does not report.
            refresh(name)
        }
        policy.ended(address, adverts.containsKey(address), sockets.holds(address))
            ?.let { schedule(address, it) }
    }

    /**
     * Whether this app may reach the local network. An app targeting Android
     * 17 (API 37) needs `ACCESS_LOCAL_NETWORK`, a runtime grant; one targeting
     * 36 or lower holds it by default, so the check passes. The module does
     * not declare it: a declaration revokes that default grant.
     */
    private fun localNetworkPermitted(): Boolean =
        Build.VERSION.SDK_INT < API_37 ||
            // The restriction keys off the app's target, so this holds
            // whether or not the platform's default grant to older targets
            // (ConnectivityCompatChanges.USE_NSD_PICKER_WHEN_NO_LOCAL_NET_PERMISSION)
            // shows up in checkSelfPermission.
            context.applicationInfo.targetSdkVersion < API_37 ||
            ContextCompat.checkSelfPermission(context, ACCESS_LOCAL_NETWORK) ==
            PackageManager.PERMISSION_GRANTED

    companion object {
        /** Android 17. Not a named constant in the SDK this module compiles against. */
        private const val API_37 = 37
        private const val ACCESS_LOCAL_NETWORK = "android.permission.ACCESS_LOCAL_NETWORK"

        private fun needsMulticastLock(): Boolean =
            Build.VERSION.SDK_INT < Build.VERSION_CODES.TIRAMISU ||
                SdkExtensions.getExtensionVersion(Build.VERSION_CODES.TIRAMISU) < 7

        /** A connect that takes longer is left to the redial ladder. iOS's `DIAL_TIMEOUT`. */
        private const val DIAL_TIMEOUT_MS = 10_000
        private const val RESOLVE_RETRY_MS = 1_000L
        /** A resolve with no answer by then is given up, and the next one runs. */
        private const val RESOLVE_TIMEOUT_MS = 15_000L
        /** A failed advert or browse is tried again after this long. iOS's `REBUILD_DELAY`. */
        private const val REBUILD_DELAY_MS = 5_000L

        /**
         * The DNS-SD instance name: a digest of the address, as iOS and Python
         * name their own, so a restarted advert replaces its record in each
         * peer's cache instead of publishing a second one beside it.
         */
        fun instanceName(address: String): String =
            "op-" + WifiDirectGroupFormation.hex(WifiDirectGroupFormation.sha256(address), 8)

        /**
         * The peer address a LAN record advertises, or null for a record that
         * is not a peer hint: another version, no `addr`, or a service
         * instance (`sid`, the DNS-SD mapping chapter), which names the same
         * host as its peer record and would open a second stream to it. No
         * `app` entry is required: iOS and Python publish none.
         */
        fun lanAdvertAddress(attributes: Map<String, ByteArray?>): String? {
            if (attributes.containsKey(WifiDirectGroupFormation.KEY_SERVICE_ID)) return null
            if (attributes[WifiDirectGroupFormation.KEY_VERSION]?.decodeToString() != "1") return null
            val address = attributes[WifiDirectGroupFormation.KEY_ADDRESS]?.decodeToString() ?: return null
            return address.takeIf { it.startsWith("off1") }
        }
    }
}
