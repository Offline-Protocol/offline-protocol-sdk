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
import android.os.Looper
import android.util.Log
import androidx.core.content.ContextCompat
import uniffi.offline_protocol.OfflineProtocol
import java.io.*
import java.net.InetSocketAddress
import java.net.ServerSocket
import java.net.Socket
import java.util.concurrent.*
import java.util.concurrent.atomic.AtomicBoolean

/**
 * WiFi Direct Manager implementing TransportManager for WiFi P2P communication
 *
 * ## wifiDirectPeerIdIsUnavailable — why nothing here reaches the protocol layer
 *
 * This manager used to pass `socket.remoteSocketAddress.toString()` (and
 * `"go:<ip>"` for the group-owner link) to `wifiDirectPeerConnected`,
 * `wifiDirectMessageReceived` and `wifiDirectPeerDisconnected`. Those
 * parameters are documented as the peer's **user-level id** — the value the
 * core keys `connected_links` by and matches against `Message.sender` — and a
 * TCP endpoint is not one. The mismatch was survivable while ids were opaque
 * nicknames; under derived addresses it is not.
 *
 * There is no correct value to pass instead. Unlike BLE, which carries a
 * DEVICE_ID and a signed IDENTITY characteristic, this transport has no
 * handshake at all: the socket protocol is a bare `writeInt(len) + bytes`
 * stream, discovery yields only a device name and a MAC, and no service
 * record advertises anything. The only identity on the wire is
 * `Message.sender` *inside* the frame, which is precisely the value the
 * transport peer id exists to cross-check and therefore cannot supply.
 *
 * So the announcements are gone rather than wrong. Nothing is lost by that:
 * `WifiDirectTransport` is never registered with the transport manager (see
 * `OfflineProtocol::new` and `rebuild_transports_for_identity` in the UniFFI
 * crate), so inbound frames were already dropped and no send could ever leave.
 * What the announcements *did* do was reach `notify_neighbor_reachable` →
 * `on_neighbor_discovered`, which entered the socket string into `known_peers`
 * and started an auto key exchange toward it — burning a slot in a
 * capacity-bounded map that evicts genuine neighbors, and emitting a
 * `neighbor_discovered` whose `peer_id` the public API promises is an `off1…`
 * address usable as a `recipient`.
 *
 * Restoring the transport means adding an identity exchange — the natural
 * shape is a length-prefixed preamble carrying the same signed blob the BLE
 * IDENTITY characteristic serves, cross-checked the same way — and registering
 * `WifiDirectTransport`. Both are out of scope here.
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
    override var state: TransportState = TransportState.UNAVAILABLE
        private set
    override var listener: TransportManagerListener? = null

    companion object {
        private const val TAG = "WifiDirectManager"
        
        // Fallback interval for message polling. Primary send path is event-driven
        // via onMessagesAvailable(); this timer only catches edge cases.
        private const val MESSAGE_POLL_INTERVAL_MS = 2000L
        private const val CONNECTION_TIMEOUT_MS = 30000L
        private const val SERVER_PORT = 8988
        private const val SOCKET_TIMEOUT_MS = 5000
    }

    // MARK: - Properties
    
    private val wifiP2pManager: WifiP2pManager? = 
        context.getSystemService(Context.WIFI_P2P_SERVICE) as? WifiP2pManager
    private var channel: WifiP2pManager.Channel? = null
    
    // Handler for main thread operations
    private val mainHandler = Handler(Looper.getMainLooper())
    
    // Executor for socket operations
    private val socketExecutor = Executors.newCachedThreadPool()
    
    // Server socket for receiving connections
    private var serverSocket: ServerSocket? = null
    private var serverRunning = AtomicBoolean(false)
    
    // Connected peers
    private val connectedPeers = ConcurrentHashMap<String, Socket>()
    private val discoveredPeers = ConcurrentHashMap<String, WifiP2pDevice>()
    
    // State tracking
    private var isGroupOwner = false
    private var groupOwnerAddress: String? = null
    private var transportStartAt: Long = 0L

    // Message polling
    private val messagePollingRunnable = object : Runnable {
        override fun run() {
            pollAndSendMessages()
            if (state == TransportState.RUNNING) {
                mainHandler.postDelayed(this, MESSAGE_POLL_INTERVAL_MS)
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
        transportStartAt = System.currentTimeMillis()

        // Initialize channel
        channel = wifiP2pManager?.initialize(context, Looper.getMainLooper()) { 
            emitDiagnostic("warning", "WiFi P2P channel disconnected")
        }

        // Register broadcast receiver
        val intentFilter = IntentFilter().apply {
            addAction(WifiP2pManager.WIFI_P2P_STATE_CHANGED_ACTION)
            addAction(WifiP2pManager.WIFI_P2P_PEERS_CHANGED_ACTION)
            addAction(WifiP2pManager.WIFI_P2P_CONNECTION_CHANGED_ACTION)
            addAction(WifiP2pManager.WIFI_P2P_THIS_DEVICE_CHANGED_ACTION)
        }
        
        if (Build.VERSION.SDK_INT >= Build.VERSION_CODES.TIRAMISU) {
            context.registerReceiver(p2pReceiver, intentFilter, Context.RECEIVER_NOT_EXPORTED)
        } else {
            context.registerReceiver(p2pReceiver, intentFilter)
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
        mainHandler.post(messagePollingRunnable)

        emitDiagnostic("info", "WiFi Direct transport started")
    }

    override fun stop() {
        if (state != TransportState.RUNNING && state != TransportState.STARTING) {
            return
        }

        updateState(TransportState.STOPPING)

        // Stop message polling
        mainHandler.removeCallbacks(messagePollingRunnable)

        // Stop peer discovery
        stopPeerDiscovery()

        // Stop server socket
        stopServerSocket()

        // Close all connections
        closeAllConnections()

        // Unregister receiver
        try {
            context.unregisterReceiver(p2pReceiver)
        } catch (e: Exception) {
            // Ignore - might not be registered
        }

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
        mainHandler.removeCallbacks(messagePollingRunnable)
        stopPeerDiscovery()
    }

    override fun resume() {
        if (state == TransportState.RUNNING) {
            startPeerDiscovery()
            mainHandler.post(messagePollingRunnable)
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
            // WiFi P2P was disabled
            try {
                protocol.wifiDirectStatusChanged(false)
            } catch (e: Exception) {
                Log.e(TAG, "Error notifying protocol", e)
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
            closeAllConnections()
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
            try {
                serverSocket = ServerSocket(SERVER_PORT)
                serverSocket?.soTimeout = SOCKET_TIMEOUT_MS
                
                emitDiagnostic("info", "Server socket started on port $SERVER_PORT")
                
                while (serverRunning.get()) {
                    try {
                        val client = serverSocket?.accept()
                        if (client != null) {
                            handleClientConnection(client)
                        }
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
            val clientAddress = socket.remoteSocketAddress.toString()
            
            emitDiagnostic("info", "Client connected", mapOf(
                "address" to clientAddress
            ))
            
            connectedPeers[clientAddress] = socket

            // NOT announced to the protocol layer — see [wifiDirectPeerIdIsUnavailable].
            // `clientAddress` is a TCP endpoint ("/192.168.49.1:8988"), not a
            // protocol id, and this transport has no handshake that would
            // yield one.
            emitDiagnostic("warning", "Wi-Fi Direct peer not announced: no protocol id available", mapOf(
                "socket" to clientAddress
            ))

            // Handle incoming messages
            try {
                val inputStream = DataInputStream(socket.getInputStream())
                
                while (socket.isConnected && !socket.isClosed) {
                    try {
                        // Read message length
                        val length = inputStream.readInt()
                        if (length > 0 && length < 1024 * 1024) { // Max 1MB
                            // Read message data
                            val data = ByteArray(length)
                            inputStream.readFully(data)

                            // Dropped, not ingested — see
                            // [wifiDirectPeerIdIsUnavailable]. Handing this to
                            // `wifiDirectMessageReceived` would attribute the
                            // frame to a socket string, which the core would
                            // then compare against `Message.sender` and reject.
                            mainHandler.post {
                                try {
                                    emitDiagnostic("warning", "Wi-Fi Direct frame dropped: sender cannot be identified", mapOf(
                                        "socket" to clientAddress,
                                        "size" to length
                                    ))
                                } catch (e: Exception) {
                                    emitDiagnostic("error", "Error processing message", mapOf(
                                        "error" to (e.message ?: "unknown")
                                    ))
                                }
                            }
                        }
                    } catch (e: java.io.EOFException) {
                        break
                    }
                }
            } catch (e: Exception) {
                if (!socket.isClosed) {
                    emitDiagnostic("error", "Error reading from socket", mapOf(
                        "error" to (e.message ?: "unknown")
                    ))
                }
            } finally {
                // Cleanup
                connectedPeers.remove(clientAddress)
                try {
                    socket.close()
                } catch (e: Exception) {
                    // Ignore
                }
                
                // No disconnect notification either: nothing was announced, so
                // there is no neighbor for the core to lose.
                emitDiagnostic("info", "Wi-Fi Direct socket closed", mapOf(
                    "socket" to clientAddress
                ))
            }
        }
    }

    private fun connectToGroupOwner(address: String) {
        socketExecutor.execute {
            try {
                val socket = Socket()
                socket.connect(InetSocketAddress(address, SERVER_PORT), CONNECTION_TIMEOUT_MS.toInt())
                
                val peerAddress = "go:$address"
                connectedPeers[peerAddress] = socket
                
                emitDiagnostic("info", "Connected to group owner", mapOf(
                    "address" to address
                ))

                // Not announced — see [wifiDirectPeerIdIsUnavailable]. "go:<ip>"
                // is no more a protocol id than the socket string is.

                // Handle incoming messages
                handleClientConnection(socket)
                
            } catch (e: Exception) {
                emitDiagnostic("error", "Failed to connect to group owner", mapOf(
                    "address" to address,
                    "error" to (e.message ?: "unknown")
                ))
            }
        }
    }

    private fun closeAllConnections() {
        for ((_, socket) in connectedPeers) {
            try {
                socket.close()
            } catch (e: Exception) {
                // Ignore
            }
        }
        connectedPeers.clear()
    }

    // MARK: - Message Handling (Event-Driven)

    /**
     * Called by the Rust transport callback when new outgoing messages are available.
     * This is the primary send path, replacing the 100ms polling loop.
     */
    fun onMessagesAvailable() {
        mainHandler.post { drainAndSendMessages() }
    }

    /**
     * Drains the Rust message queue and sends each message over WiFi Direct.
     * Called from onMessagesAvailable() and from the fallback polling timer.
     */
    private fun drainAndSendMessages() {
        if (state != TransportState.RUNNING || connectedPeers.isEmpty()) return

        try {
            while (true) {
                val message = protocol.wifiDirectGetNextMessage() ?: break
                sendMessage(message.recipientId, message.data.map { it.toByte() }.toByteArray())
            }
        } catch (e: Exception) {
            emitDiagnostic("error", "Error in drainAndSendMessages", mapOf(
                "error" to (e.message ?: "unknown")
            ))
        }
    }

    private fun pollAndSendMessages() {
        if (state != TransportState.RUNNING || connectedPeers.isEmpty()) return

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

    private fun sendMessage(recipientId: String, data: ByteArray) {
        socketExecutor.execute {
            // Find target socket
            val targetSocket = connectedPeers[recipientId]
            
            val socketsToSend = if (targetSocket != null) {
                listOf(targetSocket)
            } else {
                // Broadcast to all
                connectedPeers.values.toList()
            }
            
            if (socketsToSend.isEmpty()) {
                emitDiagnostic("warning", "No peers to send message to")
                return@execute
            }
            
            for (socket in socketsToSend) {
                try {
                    val outputStream = DataOutputStream(socket.getOutputStream())
                    outputStream.writeInt(data.size)
                    outputStream.write(data)
                    outputStream.flush()

                    emitDiagnostic("debug", "Message sent", mapOf(
                        "to" to (if (targetSocket != null) recipientId else "broadcast"),
                        "size" to data.size
                    ))
                } catch (e: Exception) {
                    emitDiagnostic("error", "Failed to send message", mapOf(
                        "error" to (e.message ?: "unknown")
                    ))
                }
            }
        }
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

