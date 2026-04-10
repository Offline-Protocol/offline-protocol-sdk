package com.offlineprotocol.ble

import android.Manifest
import android.bluetooth.*
import android.bluetooth.le.*
import android.content.Context
import android.content.pm.PackageManager
import android.os.Build
import android.os.BatteryManager
import android.os.Handler
import android.os.Looper
import android.os.ParcelUuid
import android.util.Log
import androidx.core.content.ContextCompat
import com.offlineprotocol.BleDiscoveryBootstrapPolicy
import com.offlineprotocol.TransportException
import com.offlineprotocol.TransportManager
import com.offlineprotocol.TransportManagerListener
import com.offlineprotocol.TransportState
import com.offlineprotocol.optNullableString
import com.offlineprotocol.mesh.MeshAdvertisementData
import com.offlineprotocol.mesh.MeshController
import com.offlineprotocol.mesh.MeshController.ConnectionIntent
import com.offlineprotocol.mesh.MeshController.MeshRole
import uniffi.offline_protocol.OfflineProtocol
import android.bluetooth.BluetoothStatusCodes
import java.util.*
import java.util.concurrent.ConcurrentHashMap
import java.util.concurrent.CountDownLatch
import java.util.concurrent.ThreadLocalRandom
import java.util.concurrent.atomic.AtomicInteger
import kotlin.math.min
import kotlin.math.roundToInt

private class LogThrottler(private val defaultIntervalMs: Long = 5000L) {
    private val timestamps = ConcurrentHashMap<String, Long>()

    fun shouldLog(key: String, intervalMs: Long = defaultIntervalMs, nowMs: Long = System.currentTimeMillis()): Boolean {
        val last = timestamps[key]
        if (last != null && nowMs - last < intervalMs) {
            return false
        }
        timestamps[key] = nowMs
        return true
    }
}

/**
 * BLE transport facade implementing [TransportManager] for Bluetooth Low
 * Energy communication. Ensures iOS ↔ Android cross-platform compatibility.
 *
 * The peripheral GATT server is delegated to [PeripheralGattServer] (which
 * attaches a CCCD descriptor to every notify characteristic and runs a
 * service-ready watchdog), advertising is delegated to [LeAdvertiser], and
 * connection bookkeeping lives in [MeshConnectionRegistry]. The central-role
 * path (scanning + the GATT client callback + mesh orchestration + fragment
 * accounting) still lives in this class — it is the next slice of the
 * migration and its size is why this file is large.
 */
class BleTransportFacade(
    private val context: Context,
    // Thread-safe: OfflineProtocol uses Mutex/RwLock internally (see offline-protocol-uniffi)
    private val protocol: OfflineProtocol,
    private val deviceId: String,
    private val diagnosticEmitter: ((String, String, Map<String, Any?>) -> Unit)? = null
) : TransportManager {
    
    // MARK: - TransportManager Implementation
    
    override val transportId = "ble"
    override val transportName = "Bluetooth Low Energy"
    override var state: TransportState = TransportState.UNAVAILABLE
        private set
    override var listener: TransportManagerListener? = null
    
    // MARK: - BLE Constants (matching Rust core and iOS)
    
    companion object {
        private const val TAG = "BleTransportFacade"
        
        // UUIDs must match iOS and Rust core exactly
        private val SERVICE_UUID = UUID.fromString("6E400001-B5A3-F393-E0A9-E50E24DCCA9E")
        private val MESSAGE_CHAR_UUID = UUID.fromString("6E400002-B5A3-F393-E0A9-E50E24DCCA9E")
        private val DEVICE_ID_CHAR_UUID = UUID.fromString("6E400003-B5A3-F393-E0A9-E50E24DCCA9E")
        private val IDENTITY_CHAR_UUID = UUID.fromString("6E400004-B5A3-F393-E0A9-E50E24DCCA9E")
        private const val AD_TYPE_INCOMPLETE_128_BIT_SERVICE_UUIDS = 0x06
        private const val AD_TYPE_COMPLETE_128_BIT_SERVICE_UUIDS = 0x07
        private const val UUID_128_BIT_LENGTH_BYTES = 16
        private val SERVICE_UUID_LE_BYTES = uuidToLittleEndianBytes(SERVICE_UUID)
        
        // Fallback interval for fragment polling. Primary send path is event-driven
        // via onFragmentsAvailable(); this timer only catches edge cases.
        private const val FRAGMENT_POLL_INTERVAL_MS = 2000L
        private const val MAX_FRAGMENT_SIZE = 185
        private const val CONNECTION_TIMEOUT_MS = 10000L
        private const val SCAN_WATCHDOG_INTERVAL_MS = 30000L // Match iOS timing
        private const val SCAN_WATCHDOG_HEARTBEAT_MS = 10000L
        private const val MAX_CONNECTIONS_PER_DEVICE = 4
        private const val MIN_RECONNECT_INTERVAL_MS = 5_000L // Match iOS timing
        private const val MAX_RECONNECT_INTERVAL_MS = 60_000L
        private const val MAX_CONNECTION_RETRIES = 5
        
        // Adaptive Scan Configuration
        /** Minimum RSSI to consider for connection (filter weak signals early) - matches iOS */
        private const val ADAPTIVE_MIN_RSSI = -85
        /** Absolute minimum RSSI below which we refuse to connect - matches iOS */
        private const val MINIMUM_RSSI_TO_CONNECT = -90
        /** Peer count threshold below which we process all advertisements */
        private const val ADAPTIVE_LOW_DENSITY_THRESHOLD = 10
        /** Peer count threshold above which we apply maximum throttling */
        private const val ADAPTIVE_HIGH_DENSITY_THRESHOLD = 50
        /** Maximum connection attempts per minute in dense networks */
        private const val ADAPTIVE_MAX_CONNECTIONS_PER_MINUTE = 6
        /** Minimum interval between connection attempts to the same device (ms) */
        private const val ADAPTIVE_COOLDOWN_PER_DEVICE_MS = 30_000L
        /** Interval for updating visible peer count estimate (ms) */
        private const val ADAPTIVE_PEER_COUNT_WINDOW_MS = 5_000L
        /** Cooldown between provisional bootstrap attempts for unknown devices */
        private const val UNKNOWN_BOOTSTRAP_RATE_LIMIT_MS = 12_000L
        /** Minimum RSSI required for provisional bootstrap attempt */
        private const val UNKNOWN_BOOTSTRAP_MIN_RSSI = -75
        /** Stricter RSSI requirement when scan record is missing */
        private const val UNKNOWN_BOOTSTRAP_MIN_RSSI_NO_SCAN_RECORD = -68
        /** Max provisional bootstrap attempts per minute */
        private const val MAX_UNKNOWN_BOOTSTRAP_ATTEMPTS_PER_MINUTE = 4
        /** Proactive scan refresh interval even when discoveries are occurring (ms) */
        private const val PROACTIVE_SCAN_REFRESH_MS = 60_000L
        /** Force a complete BLE stack refresh periodically even when things seem healthy (ms) */
        private const val FORCED_BLE_REFRESH_MS = 120_000L
        /** Maximum consecutive scan restarts before resetting BLE adapter */
        private const val MAX_CONSECUTIVE_SCAN_RESTARTS = 3
        /** Backoff period after resetting BLE adapter (ms) */
        private const val ADAPTER_RESET_BACKOFF_MS = 45_000L
        /** Connection monitor interval for periodic reconnection attempts (ms) */
        private const val CONNECTION_MONITOR_INTERVAL_MS = 5_000L
        /** Initial aggressive discovery phase duration (ms) - more frequent scanning initially */
        private const val AGGRESSIVE_DISCOVERY_PHASE_MS = 30_000L
        /** TTL for negative cache entries of verified non-mesh devices (ms) */
        private const val NON_MESH_CACHE_TTL_MS = 300_000L // 5 minutes

        private fun uuidToLittleEndianBytes(uuid: UUID): ByteArray {
            val hexUuid = uuid.toString().uppercase().replace("-", "")
            val bigEndianBytes = hexUuid.chunked(2).map { it.toInt(16).toByte() }.toByteArray()
            return bigEndianBytes.reversedArray()
        }
    }
    
    // MARK: - Properties
    
    private val bluetoothManager: BluetoothManager = 
        context.getSystemService(Context.BLUETOOTH_SERVICE) as BluetoothManager
    private val bluetoothAdapter: BluetoothAdapter? = bluetoothManager.adapter
    
    // Scanner components
    private var bluetoothLeScanner: BluetoothLeScanner? = null
    private var scanCallback: ScanCallback? = null
    private var isScanning = false
    
    // Advertiser component (delegates to LeAdvertiser).
    // Lazy so its construction sees mainHandler / logThrottler / peripheralGattServer
    // which are declared later in this file.
    private val leAdvertiser: LeAdvertiser by lazy(LazyThreadSafetyMode.NONE) {
        LeAdvertiser(
            mainHandler = mainHandler,
            host = object : LeAdvertiser.Host {
                override fun isGattServerReady(): Boolean = peripheralGattServer?.isReady == true
                override fun buildAdvertiseData() = this@BleTransportFacade.buildAdvertiseData()
                override fun buildScanResponse() = this@BleTransportFacade.buildScanResponse()
                override fun onBeforeRefresh() { updateSignedIdentity() }
                override fun shouldLog(key: String, intervalMs: Long) =
                    logThrottler.shouldLog(key, intervalMs = intervalMs)
            },
            diagnosticEmitter = { level, message, ctx -> emitDiagnostic(level, message, ctx) },
        )
    }

    // GATT Server (peripheral role). Delegated to [PeripheralGattServer] so
    // that the NOTIFY characteristic always carries a CCCD descriptor,
    // descriptor writes are acked, service registration has a watchdog, and
    // long reads honour offsets.
    private var peripheralGattServer: PeripheralGattServer? = null
    
    // Cached signed identity data for serving via GATT
    // Read by provideIdentityBytes() on the binder thread; written by
    // updateSignedIdentity() on main / binder. @Volatile so the latest
    // reference is visible to binder-thread readers.
    @Volatile
    private var cachedSignedIdentity: com.offlineprotocol.mesh.SignedIdentityData? = null
    
    // Verified peer identities (device address -> SignedIdentityData)
    private val verifiedPeerIdentities = ConcurrentHashMap<String, com.offlineprotocol.mesh.SignedIdentityData>()

    // Addresses whose CCCD subscription has been acked by the remote peer.
    // Until the descriptor write lands we can't trust that the peripheral is
    // actually notifying us, so the link is only considered ready to drain
    // fragments once this set contains the address.
    private val linkReady = ConcurrentHashMap.newKeySet<String>()
    
    // Connection registry keeps track of client/server links and desired roles.
    private val connections = MeshConnectionRegistry()
    private val lastSeenRssi = ConcurrentHashMap<String, Short>()
    private val discoveryLogTimestamps = ConcurrentHashMap<String, Long>()
    @Volatile private var lastDiscoveryAt: Long = 0L

    private val logThrottler = LogThrottler()
    
    // Pending fragments waiting for device ID
    private data class PendingFragment(val data: ByteArray, val timestamp: Long)
    private data class MeshObservation(val advertisement: MeshAdvertisementData, val rssi: Int?, val timestamp: Long)
    // Single address-keyed inbound buffer used by both the GATT-client path
    // (our notify callback) and the GATT-server path (central → peripheral
    // writes). Entries are queued while a peer's stable device ID is still
    // being resolved via a reverse GATT read, and drained by
    // [processPendingFragments] once the device ID characteristic is read.
    // Using the connection-specific address as the key is RPA-safe: the
    // address is stable for the lifetime of a single LL connection on both
    // sides, even if the peer's advertised MAC rotates outside of it.
    private val pendingFragments = HashMap<String, MutableList<PendingFragment>>()
    private val pendingFragmentsLock = Any()
    private val PENDING_FRAGMENT_TIMEOUT_MS = 5000L
    private val connectionRetryCount = ConcurrentHashMap<String, Int>()
    private val LOAD_SATURATION_COUNT = 20
    private val MESH_OBSERVATION_TTL_MS = 120_000L
    private val deviceIdResolutionAttempts = ConcurrentHashMap<String, Long>()

    private val meshController = MeshController(deviceId)
    
    // Adaptive scan state
    /** Timestamps of recent peripheral discoveries for density estimation */
    private val recentDiscoveryTimestamps = Collections.synchronizedList(mutableListOf<Long>())
    /** Last connection attempt timestamps per device for rate limiting */
    private val deviceConnectionAttempts = ConcurrentHashMap<String, Long>()
    /** Global connection attempts in the last minute for rate limiting */
    private val globalConnectionAttempts = Collections.synchronizedList(mutableListOf<Long>())
    /** Current estimated visible peer count */
    @Volatile private var estimatedVisiblePeerCount: Int = 0
    /** Last time we updated the peer count estimate */
    @Volatile private var lastPeerCountUpdate: Long = 0L
    @Volatile private var lastMeshAdvertisement: MeshAdvertisementData? = null
    /** Last time we proactively refreshed the scan */
    @Volatile private var lastProactiveScanRefresh: Long = 0L
    /** Last time we performed a forced BLE refresh */
    @Volatile private var lastForcedBleRefresh: Long = 0L
    /** Rate limiter for provisional unknown-device bootstrap attempts */
    private val unknownBootstrapAttempts = ConcurrentHashMap<String, Long>()
    /** Tracks recently seen advertisements to avoid duplicate processing (hash, timestamp) */
    private data class AdvertisementCacheEntry(val hash: Int, val timestamp: Long)
    private val recentAdvertisementHashes = ConcurrentHashMap<String, AdvertisementCacheEntry>()
    /** Negative cache: devices verified via GATT as non-mesh (address -> timestamp) */
    private val verifiedNonMeshDevices = ConcurrentHashMap<String, Long>()
    /** Counter for consecutive scan restarts without discoveries */
    @Volatile private var scanRestartCount = 0
    /** Last time we reset the BLE adapter */
    @Volatile private var lastAdapterReset: Long = 0L
    /** Connection monitor runnable for periodic reconnection attempts */
    private var connectionMonitorRunnable: Runnable? = null
    
    // Fragment polling
    private val mainHandler = Handler(Looper.getMainLooper())

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

    // Async variant: runs inline when already on the main thread, otherwise
    // posts. Used by BLE callbacks that must mutate main-thread-only state.
    private fun runOnMain(action: () -> Unit) {
        if (Looper.myLooper() == Looper.getMainLooper()) {
            action()
        } else {
            mainHandler.post(action)
        }
    }

    private val fragmentPollingRunnable = object : Runnable {
        override fun run() {
            pollAndSendFragments()
            if (state == TransportState.RUNNING) {
                mainHandler.postDelayed(this, FRAGMENT_POLL_INTERVAL_MS)
            }
        }
    }
    
    // Gradient routing cleanup
    private val ROUTING_CLEANUP_INTERVAL_MS = 30_000L
    private val routingCleanupRunnable = object : Runnable {
        override fun run() {
            protocol.cleanupExpiredRoutes()
            if (state == TransportState.RUNNING) {
                mainHandler.postDelayed(this, ROUTING_CLEANUP_INTERVAL_MS)
            }
        }
    }
    
    // Track outbound fragments with timestamps for timeout handling.
    //
    // Thread-safety contract: ArrayDeque values are NOT thread-safe and MUST only
    // be touched on the mainHandler thread — including `size` reads, since
    // ArrayDeque computes it from `head`/`tail` plus a possibly-resized backing
    // array. ConcurrentHashMap is used only so that key lookups from callback
    // threads don't need to block.
    //
    // Any code that runs off the main thread (BLE callbacks, etc.) and wants to
    // touch this map must either post to mainHandler or read the aggregate
    // count via [totalPendingOutboundFragments], which is updated in lockstep
    // with the deques and is safe to read from any thread.
    private data class OutboundFragment(val data: ByteArray, val timestamp: Long)
    private val pendingOutboundFragments = ConcurrentHashMap<String, ArrayDeque<OutboundFragment>>()
    private val totalPendingOutboundFragments = AtomicInteger(0)
    private val PENDING_OUTBOUND_FRAGMENT_TIMEOUT_MS = 30_000L // 30 seconds
    private val MAX_PENDING_FRAGMENTS_PER_PEER = 100
    private val lastSeenMeshAdvertisements = ConcurrentHashMap<String, MeshObservation>()
    private var transportStartAt: Long = 0L

    private val scanWatchdogRunnable = object : Runnable {
        override fun run() {
            if (!isScanning) {
                return
            }
            val now = System.currentTimeMillis()
            val idleMs = now - lastDiscoveryAt
            if (idleMs >= SCAN_WATCHDOG_INTERVAL_MS) {
                if (logThrottler.shouldLog("scan_watchdog", intervalMs = SCAN_WATCHDOG_INTERVAL_MS)) {
                    Log.w(TAG, "Restarting BLE scan after ${idleMs}ms of inactivity")
                    emitDiagnostic("warning", "Restarting BLE scan due to inactivity", mapOf("idleMs" to idleMs))
                }
                scanRestartCount++
                restartScanning("watchdog")
                evaluateBleHealthAfterRestart()
                return
            }
            mainHandler.postDelayed(this, SCAN_WATCHDOG_HEARTBEAT_MS)
        }
    }
    
    /**
     * Evaluates BLE stack health after consecutive restarts and resets adapter if needed.
     * This mirrors iOS's evaluateCentralHealthAfterRestart mechanism.
     */
    private fun evaluateBleHealthAfterRestart() {
        if (scanRestartCount < MAX_CONSECUTIVE_SCAN_RESTARTS) {
            return
        }
        
        val now = System.currentTimeMillis()
        if (lastAdapterReset > 0 && now - lastAdapterReset < ADAPTER_RESET_BACKOFF_MS) {
            return
        }
        
        Log.w(TAG, "Resetting BLE stack due to repeated scan stalls (restartCount=$scanRestartCount)")
        emitDiagnostic("warning", "Resetting BLE stack due to repeated scan stalls", mapOf(
            "restartCount" to scanRestartCount
        ))
        
        lastAdapterReset = now
        scanRestartCount = 0
        
        // Force stop and restart everything
        mainHandler.post {
            if (state == TransportState.RUNNING) {
                stopScanning("ble_reset")
                stopAdvertising()
                
                // Re-initialize scanner and advertiser
                bluetoothLeScanner = bluetoothAdapter?.bluetoothLeScanner
                leAdvertiser.attachAdvertiser(bluetoothAdapter?.bluetoothLeAdvertiser)
                
                // Restart after a brief delay
                mainHandler.postDelayed({
                    if (state == TransportState.RUNNING) {
                        startScanning("ble_reset")
                        startAdvertising("ble_reset")
                    }
                }, 1000)
            }
        }
    }
    
    // Metrics
    private var bytesSent: Long = 0
    private var bytesReceived: Long = 0
    private var fragmentsSent: Long = 0
    private var fragmentsReceived: Long = 0
    
    // MARK: - TransportManager Implementation
    
    private fun emitDiagnostic(level: String, message: String, context: Map<String, Any?> = emptyMap()) {
        diagnosticEmitter?.invoke(level, message, context)
    }

    override fun isAvailable(): Boolean {
        if (bluetoothAdapter == null) {
            Log.w(TAG, "Bluetooth adapter not available")
            emitDiagnostic("error", "Bluetooth adapter not available")
            return false
        }
        
        if (!context.packageManager.hasSystemFeature(PackageManager.FEATURE_BLUETOOTH_LE)) {
            Log.w(TAG, "BLE not supported on this device")
            emitDiagnostic("error", "BLE not supported on this device")
            return false
        }
        
        return true
    }
    
    override fun start() {
        runOnMainSync {
            startUnsafe()
        }
    }

    private fun startUnsafe() {
        if (state == TransportState.RUNNING) {
            throw TransportException.AlreadyRunning()
        }
        
        if (!isAvailable()) {
            throw TransportException.NotAvailable("BLE not available on this device")
        }
        
        // Check permissions with detailed logging
        Log.i(TAG, "🔐 Checking Bluetooth permissions (Android ${Build.VERSION.SDK_INT})...")
        emitDiagnostic("info", "Checking Bluetooth permissions", mapOf("androidVersion" to Build.VERSION.SDK_INT))
        
        if (!checkPermissions()) {
            val errorMsg = if (Build.VERSION.SDK_INT >= Build.VERSION_CODES.S) {
                "Missing required Bluetooth permissions (BLUETOOTH_SCAN, BLUETOOTH_ADVERTISE, BLUETOOTH_CONNECT). " +
                "Please grant permissions in Settings > Apps > ${context.applicationInfo.loadLabel(context.packageManager)} > Permissions"
            } else {
                "Missing required Bluetooth permissions (BLUETOOTH, BLUETOOTH_ADMIN, ACCESS_FINE_LOCATION). " +
                "Please grant permissions in app settings."
            }
            Log.w(TAG, "❌ $errorMsg")
            emitDiagnostic("error", errorMsg)
            throw TransportException.PermissionDenied(errorMsg)
        }
        
        if (bluetoothAdapter?.isEnabled != true) {
            val errorMsg = "Bluetooth is not enabled. Please enable Bluetooth in Settings."
            Log.w(TAG, "⚠️ $errorMsg")
            emitDiagnostic("error", errorMsg)
            throw TransportException.InvalidState(errorMsg)
        }
        
        Log.i(TAG, "🚀 Starting BLE transport for device: $deviceId")
        emitDiagnostic("info", "Starting BLE transport", mapOf("deviceId" to deviceId))
        updateState(TransportState.STARTING)
        
        try {
            // Initialize scanner
            Log.i(TAG, "📱 Initializing BLE scanner...")
            bluetoothLeScanner = bluetoothAdapter.bluetoothLeScanner
            if (bluetoothLeScanner == null) {
                throw TransportException.InvalidState("BLE scanner is not available")
            }
            
            // Initialize advertiser
            Log.i(TAG, "📡 Initializing BLE advertiser...")
            val advertiser = bluetoothAdapter.bluetoothLeAdvertiser
                ?: throw TransportException.InvalidState("BLE advertiser is not available")
            leAdvertiser.attachAdvertiser(advertiser)
            
            // Setup GATT server
            Log.i(TAG, "🔧 Setting up GATT server...")
            setupGattServer()

            transportStartAt = System.currentTimeMillis()
            meshController.markPeerActive(deviceId)
            refreshSelfMetrics()
            
            // Start advertising
            Log.i(TAG, "📢 Starting BLE advertising...")
            startAdvertising("start")
            
            // Start scanning
            Log.i(TAG, "🔍 Starting BLE scanning...")
            startScanning("start")
            
            // Start fragment polling
            mainHandler.post(fragmentPollingRunnable)
            
            // Start routing cleanup
            mainHandler.postDelayed(routingCleanupRunnable, ROUTING_CLEANUP_INTERVAL_MS)
            
            updateState(TransportState.RUNNING)
            Log.i(TAG, "✅ BLE Manager started successfully - calling bleStatusChanged(true)")
            emitDiagnostic("info", "About to call protocol.bleStatusChanged(true)")
            
            try {
                protocol.bleStatusChanged(true)
                Log.i(TAG, "✅ Successfully called protocol.bleStatusChanged(true)")
                emitDiagnostic("info", "Successfully called protocol.bleStatusChanged(true)")
            } catch (e: Exception) {
                Log.e(TAG, "❌ Failed to call protocol.bleStatusChanged(true): ${e.message}", e)
                emitDiagnostic("error", "Failed to call protocol.bleStatusChanged(true)", mapOf(
                    "error" to (e.message ?: "unknown"),
                    "exception" to e.javaClass.simpleName
                ))
            }
            
            Log.i(TAG, "✅ BLE transport ready - scanning and advertising active")
            emitDiagnostic(
                "info",
                "BLE manager running",
                mapOf(
                    "scanning" to true,
                    "advertising" to true,
                    "mtu" to MAX_FRAGMENT_SIZE
                )
            )
        } catch (e: Exception) {
            Log.e(TAG, "Failed to start BLE manager", e)
            emitDiagnostic(
                "error",
                "Failed to start BLE manager",
                mapOf(
                    "message" to (e.message ?: "unknown"),
                    "exception" to e.javaClass.simpleName
                )
            )
            updateState(TransportState.STOPPED)
            throw TransportException.StartFailed("Failed to start BLE manager", e)
        }
    }
    
    override fun stop() {
        runOnMainSync {
            stopUnsafe()
        }
    }

    // Called via runOnMainSync from stop(), so this always executes on the main thread.
    // removeCallbacks below guarantees no further polling/drain runnables will fire,
    // making the subsequent .clear() calls safe against concurrent access.
    private fun stopUnsafe() {
        if (state != TransportState.RUNNING && state != TransportState.STARTING) {
            return
        }

        updateState(TransportState.STOPPING)

        // Stop fragment polling — must happen before clearing queues
        mainHandler.removeCallbacks(fragmentPollingRunnable)

        // Stop routing cleanup
        mainHandler.removeCallbacks(routingCleanupRunnable)
        
        // Stop scanning
        stopScanning("stop")
        
        // Stop advertising
        stopAdvertising()
        
        // Disconnect all GATT clients
        connections.forEachGatt { gatt ->
            try {
                gatt.disconnect()
                gatt.close()
            } catch (e: Exception) {
                Log.e(TAG, "Error closing GATT client", e)
            }
        }
        connections.clear()
        lastSeenRssi.clear()
        synchronized(pendingFragmentsLock) { pendingFragments.clear() }
        pendingOutboundFragments.clear()
        totalPendingOutboundFragments.set(0)
        linkReady.clear()
        lastSeenMeshAdvertisements.clear()
        verifiedNonMeshDevices.clear()
        unknownBootstrapAttempts.clear()
        recentAdvertisementHashes.clear()
        connectionRetryCount.clear()
        scanRestartCount = 0
        lastAdapterReset = 0L
        transportStartAt = 0L
        lastProactiveScanRefresh = 0L
        lastForcedBleRefresh = 0L

        // Close GATT server (stops service, clears subscribed centrals, drops refs).
        peripheralGattServer?.stop()
        peripheralGattServer = null
        leAdvertiser.shutdown()

        updateState(TransportState.STOPPED)
        protocol.bleStatusChanged(false)
        
        Log.i(TAG, "BLE Manager stopped")
        emitDiagnostic("info", "BLE transport stopped")
    }
    
    override fun pause() {
        runOnMainSync {
            pauseUnsafe()
        }
    }
    
    private fun pauseUnsafe() {
        // For Android background mode
        stopScanning("pause")
        mainHandler.removeCallbacks(fragmentPollingRunnable)
        mainHandler.removeCallbacks(routingCleanupRunnable)
    }
    
    override fun resume() {
        runOnMainSync {
            resumeUnsafe()
        }
    }
    
    private fun resumeUnsafe() {
        // Resume from background
        if (state == TransportState.RUNNING) {
            startScanning("resume")
            mainHandler.post(fragmentPollingRunnable)
            mainHandler.postDelayed(routingCleanupRunnable, ROUTING_CLEANUP_INTERVAL_MS)
        }
    }
    
    override fun getMetrics(): Map<String, Any> {
        return mapOf(
            "bytes_sent" to bytesSent,
            "bytes_received" to bytesReceived,
            "fragments_sent" to fragmentsSent,
            "fragments_received" to fragmentsReceived,
            "connected_peers" to connections.connectionCount(),
            "discovered_peers" to connections.discoveredPeerCount()
        )
    }
    
    // MARK: - Private Methods
    
    private fun updateState(newState: TransportState) {
        state = newState
        listener?.onTransportStateChanged(this, newState)
    }
    
    private fun checkPermissions(): Boolean {
        val missingPermissions = mutableListOf<String>()
        
        if (Build.VERSION.SDK_INT >= Build.VERSION_CODES.S) {
            // Android 12+ (API 31+) requires new Bluetooth permissions
            if (ContextCompat.checkSelfPermission(context, Manifest.permission.BLUETOOTH_SCAN) != PackageManager.PERMISSION_GRANTED) {
                missingPermissions.add("BLUETOOTH_SCAN")
            }
            if (ContextCompat.checkSelfPermission(context, Manifest.permission.BLUETOOTH_ADVERTISE) != PackageManager.PERMISSION_GRANTED) {
                missingPermissions.add("BLUETOOTH_ADVERTISE")
            }
            if (ContextCompat.checkSelfPermission(context, Manifest.permission.BLUETOOTH_CONNECT) != PackageManager.PERMISSION_GRANTED) {
                missingPermissions.add("BLUETOOTH_CONNECT")
            }
        } else {
            // Pre-Android 12 (API <31)
            if (ContextCompat.checkSelfPermission(context, Manifest.permission.BLUETOOTH) != PackageManager.PERMISSION_GRANTED) {
                missingPermissions.add("BLUETOOTH")
            }
            if (ContextCompat.checkSelfPermission(context, Manifest.permission.BLUETOOTH_ADMIN) != PackageManager.PERMISSION_GRANTED) {
                missingPermissions.add("BLUETOOTH_ADMIN")
            }
            if (ContextCompat.checkSelfPermission(context, Manifest.permission.ACCESS_FINE_LOCATION) != PackageManager.PERMISSION_GRANTED) {
                missingPermissions.add("ACCESS_FINE_LOCATION")
            }
        }
        
        if (missingPermissions.isNotEmpty()) {
            Log.w(TAG, "⚠️ Missing Bluetooth permissions: ${missingPermissions.joinToString(", ")}")
            emitDiagnostic("error", "Missing Bluetooth permissions", mapOf(
                "missingPermissions" to missingPermissions,
                "androidVersion" to Build.VERSION.SDK_INT
            ))
            return false
        }
        
        Log.d(TAG, "✅ All Bluetooth permissions granted (Android ${Build.VERSION.SDK_INT})")
        return true
    }
    
    private fun setupGattServer() {
        try {
            // Dispose any previous server before starting a new one.
            peripheralGattServer?.stop()

            val server = PeripheralGattServer(
                context = context,
                mainHandler = mainHandler,
                listener = gattServerListener,
                diagnosticEmitter = { level, message, ctx ->
                    emitDiagnostic(level, message, ctx)
                },
            )
            peripheralGattServer = server

            // Prime cached identity synchronously so the first GATT read from
            // a central can be served off the volatile cache without the
            // binder thread ever calling back into UniFFI.
            updateSignedIdentity()

            server.start(
                serviceUuid = SERVICE_UUID,
                messageUuid = MESSAGE_CHAR_UUID,
                deviceIdUuid = DEVICE_ID_CHAR_UUID,
                identityUuid = IDENTITY_CHAR_UUID,
            )

            Log.i(TAG, "GATT server setup initiated, waiting for service registration callback...")
            emitDiagnostic("info", "GATT server setup initiated")
        } catch (e: SecurityException) {
            Log.e(TAG, "Permission denied while setting up GATT server", e)
            emitDiagnostic("error", "Permission denied in GATT server setup", mapOf("exception" to e.javaClass.simpleName))
            throw e
        } catch (e: Exception) {
            Log.e(TAG, "Error setting up GATT server: ${e.message}", e)
            emitDiagnostic("error", "Error setting up GATT server", mapOf(
                "exception" to e.javaClass.simpleName,
                "message" to (e.message ?: "unknown")
            ))
        }
    }
    
    /**
     * Refresh [cachedSignedIdentity] by signing the current advertisement
     * data with the identity private key. Must only be called from threads
     * that are allowed to block on UniFFI (main thread and advertisement
     * refresh callers); it must **never** be called from the GATT server's
     * binder callback thread, because each call potentially blocks on the
     * protocol mutex and stalls every pending GATT operation for the
     * affected central.
     */
    private fun updateSignedIdentity() {
        try {
            if (!protocol.isMlsInitialized()) {
                Log.d(TAG, "MLS not initialized, cannot create signed identity")
                return
            }

            val publicKey = protocol.getIdentityPublicKey()
            val meshData = meshController.toAdvertisement()
            val advertisementData = meshData.encode()
            val signature = protocol.signData(advertisementData.map { it.toUByte() })

            cachedSignedIdentity = com.offlineprotocol.mesh.SignedIdentityData(
                publicKey = publicKey.map { it.toByte() }.toByteArray(),
                signature = signature.map { it.toByte() }.toByteArray(),
                advertisementData = advertisementData
            )
            Log.d(TAG, "Updated signed identity for GATT serving")
        } catch (e: Exception) {
            Log.w(TAG, "Failed to create signed identity: ${e.message}", e)
            emitDiagnostic("warning", "Failed to create signed identity", mapOf("error" to (e.message ?: "unknown")))
        }
    }
    
    private fun startScanning(reason: String = "manual") {
        if (isScanning) {
            if (logThrottler.shouldLog("scan_already_running")) {
                Log.d(TAG, "Scan already running (reason: $reason)")
            }
            return
        }
        
        try {
            val scanner = bluetoothLeScanner
            if (scanner == null) {
                if (logThrottler.shouldLog("scanner_unavailable")) {
                    Log.w(TAG, "BluetoothLeScanner unavailable; cannot start scan")
                    emitDiagnostic("error", "BLE scanner unavailable", mapOf("reason" to reason))
                }
                return
            }
            val scanSettings = ScanSettings.Builder()
                .setScanMode(ScanSettings.SCAN_MODE_LOW_LATENCY)
                .setCallbackType(ScanSettings.CALLBACK_TYPE_ALL_MATCHES)
                .build()
            
            // Scan without service UUID filter for iOS ↔ Android interoperability
            // iOS's CoreBluetooth has known issues recognizing 128-bit service UUIDs from Android
            // advertisements, and vice versa. Scanning without filter and filtering in software
            // ensures we discover all mesh devices regardless of platform quirks.
            // We filter in handleScanResult using shouldProcessDiscoveredDevice().
            
            scanCallback = object : ScanCallback() {
                override fun onScanResult(callbackType: Int, result: ScanResult) {
                    handleScanResult(result)
                }
                
                override fun onBatchScanResults(results: List<ScanResult>) {
                    results.forEach { handleScanResult(it) }
                }
                
                override fun onScanFailed(errorCode: Int) {
                    val errorMsg = when(errorCode) {
                        SCAN_FAILED_ALREADY_STARTED -> "Scan already started"
                        SCAN_FAILED_APPLICATION_REGISTRATION_FAILED -> "Application registration failed"
                        SCAN_FAILED_INTERNAL_ERROR -> "Internal error"
                        SCAN_FAILED_FEATURE_UNSUPPORTED -> "Feature unsupported"
                        else -> "Unknown error $errorCode"
                    }
                    Log.e(TAG, "❌ BLE scan failed: $errorMsg (code=$errorCode)")
                    isScanning = false
                    emitDiagnostic("error", "BLE scan failed", mapOf(
                        "errorCode" to errorCode,
                        "errorMessage" to errorMsg
                    ))
                }
            }
            
            // Scan without filter - we'll filter in software for cross-platform compatibility
            scanner.startScan(null, scanSettings, scanCallback)
            isScanning = true
            val now = System.currentTimeMillis()
            lastDiscoveryAt = now
            lastProactiveScanRefresh = now
            // Reset restart count on non-watchdog starts
            if (reason != "restart_watchdog") {
                scanRestartCount = 0
            }
            scheduleScanWatchdog()
            startConnectionMonitor()
            if (logThrottler.shouldLog("scan_started")) {
                Log.i(TAG, "BLE scanning started (no filter, reason: $reason)")
                emitDiagnostic("info", "BLE scanning started", mapOf(
                    "reason" to reason,
                    "filterless" to true
                ))
            }
            
            // Rehydrate previously connected devices to avoid waiting for advertisements
            rehydratePreviouslyConnectedDevices()
        } catch (e: SecurityException) {
            Log.e(TAG, "Permission denied while starting scan", e)
            emitDiagnostic("error", "Permission denied while starting scan", mapOf("exception" to e.javaClass.simpleName, "message" to (e.message ?: "unknown")))
            throw e
        }
    }
    
    /**
     * Attempts to reconnect to previously known devices without waiting for advertisements.
     * This speeds up rediscovery after app restart or Bluetooth toggle.
     */
    private fun rehydratePreviouslyConnectedDevices() {
        try {
            val bondedDevices = bluetoothAdapter?.bondedDevices ?: return
            for (device in bondedDevices) {
                val address = device.address
                // Only attempt if we previously had this device in our registry
                if (connections.hasDeviceForAddress(address) && connections.getGatt(address) == null) {
                    if (logThrottler.shouldLog("rehydrate_$address", intervalMs = 30_000)) {
                        Log.d(TAG, "Rehydrating connection to bonded device: $address")
                    }
                    connectToDevice(device)
                }
            }
        } catch (e: SecurityException) {
            Log.w(TAG, "Cannot access bonded devices for rehydration", e)
        }
    }
    
    private fun stopScanning(reason: String = "manual") {
        if (!isScanning) return
        
        try {
            scanCallback?.let { bluetoothLeScanner?.stopScan(it) }
            scanCallback = null
            isScanning = false
            cancelScanWatchdog()
            cancelConnectionMonitor()
            lastDiscoveryAt = 0L
            discoveryLogTimestamps.clear()
            if (logThrottler.shouldLog("scan_stopped")) {
                Log.i(TAG, "Stopped scanning (reason: $reason)")
            }
            emitDiagnostic("info", "Stopped BLE scanning", mapOf("reason" to reason))
        } catch (e: SecurityException) {
            Log.e(TAG, "Permission denied while stopping scan", e)
            emitDiagnostic("error", "Permission denied while stopping scan", mapOf("exception" to e.javaClass.simpleName, "message" to (e.message ?: "unknown")))
        }
    }
    
    private fun restartScanning(reason: String) {
        stopScanning("restart_$reason")
        startScanning("restart_$reason")
    }
    
    private fun scheduleScanWatchdog() {
        cancelScanWatchdog()
        mainHandler.postDelayed(scanWatchdogRunnable, SCAN_WATCHDOG_HEARTBEAT_MS)
    }
    
    private fun cancelScanWatchdog() {
        mainHandler.removeCallbacks(scanWatchdogRunnable)
    }
    
    /**
     * Starts the connection monitor that periodically attempts to reconnect to discovered devices.
     * This mirrors iOS's startConnectionMonitor mechanism for more reliable discovery.
     */
    private fun startConnectionMonitor() {
        cancelConnectionMonitor()
        
        connectionMonitorRunnable = object : Runnable {
            override fun run() {
                if (state != TransportState.RUNNING) {
                    return
                }
                
                val now = System.currentTimeMillis()
                
                // Check for discovered devices that aren't connected
                for ((address, observation) in lastSeenMeshAdvertisements) {
                    // Skip if already connected
                    if (connections.getGatt(address) != null) {
                        continue
                    }
                    
                    // Skip if we've hit connection cap
                    if (currentConnectionCount() >= MAX_CONNECTIONS_PER_DEVICE) {
                        break
                    }
                    
                    // Skip if observation is too old
                    if (now - observation.timestamp > MESH_OBSERVATION_TTL_MS) {
                        continue
                    }
                    
                    // Skip if RSSI too weak
                    val rssi = observation.rssi ?: continue
                    if (rssi < MINIMUM_RSSI_TO_CONNECT) {
                        continue
                    }
                    
                    // Rate limit attempts to this device
                    val lastAttempt = deviceConnectionAttempts[address]
                    if (lastAttempt != null && now - lastAttempt < MIN_RECONNECT_INTERVAL_MS) {
                        continue
                    }
                    
                    // Try to connect
                    try {
                        val device = bluetoothAdapter?.getRemoteDevice(address) ?: continue
                        recordConnectionAttempt(address, now)
                        connectToDevice(device)
                    } catch (e: Exception) {
                        Log.w(TAG, "Connection monitor: failed to get remote device $address", e)
                    }
                }
                
                // Also check for pending fragments that need device ID resolution
                val pendingAddresses = synchronized(pendingFragmentsLock) { pendingFragments.keys.toList() }
                for (address in pendingAddresses) {
                    if (connections.deviceIdForAddress(address) != null) {
                        continue
                    }
                    
                    val lastAttempt = deviceIdResolutionAttempts[address]
                    if (lastAttempt != null && now - lastAttempt < MIN_RECONNECT_INTERVAL_MS) {
                        continue
                    }
                    
                    deviceIdResolutionAttempts[address] = now
                    try {
                        val device = bluetoothAdapter?.getRemoteDevice(address)
                        if (device != null && connections.getGatt(address) == null) {
                            connectToDevice(device)
                        }
                    } catch (e: Exception) {
                        Log.w(TAG, "Connection monitor: failed to resolve device ID for $address", e)
                    }
                }
                
                mainHandler.postDelayed(this, CONNECTION_MONITOR_INTERVAL_MS)
            }
        }
        
        mainHandler.postDelayed(connectionMonitorRunnable!!, CONNECTION_MONITOR_INTERVAL_MS)
    }
    
    private fun cancelConnectionMonitor() {
        connectionMonitorRunnable?.let { mainHandler.removeCallbacks(it) }
        connectionMonitorRunnable = null
    }
    
    // Advertising is owned by LeAdvertiser. These thin wrappers preserve
    // the existing call sites in this file (refreshAdvertising / stopAdvertising
    // are invoked from many places) without threading the delegate object
    // through every caller.

    private fun startAdvertising(reason: String = "manual") {
        leAdvertiser.start(reason)
    }

    private fun stopAdvertising() {
        leAdvertiser.stop()
    }

    private fun refreshAdvertising(reason: String) {
        leAdvertiser.refresh(reason)
    }

    private fun buildAdvertiseData(): AdvertiseData {
        val meshData = meshController.toAdvertisement()
        lastMeshAdvertisement = meshData
        
        // Android has strict 31-byte advertisement limit
        // Include only service UUID, mesh metadata will be exchanged via GATT after connection
        // This matches iOS behavior which also cannot include service data
        return AdvertiseData.Builder()
            .setIncludeDeviceName(false)
            .addServiceUuid(ParcelUuid(SERVICE_UUID))
            // Don't include service data - it often exceeds Android's 31-byte limit
            // Mesh metadata will be read via GATT characteristics after connection
            .build()
    }
    
    /**
     * Builds the scan response data for BLE advertising.
     * 
     * iOS's CoreBluetooth actively queries for scan responses during BLE scanning.
     * Including the service UUID in the scan response makes Android devices more
     * reliably visible to iOS devices, which have known issues recognizing 128-bit
     * service UUIDs from Android's main advertisement packet format.
     */
    private fun buildScanResponse(): AdvertiseData {
        return AdvertiseData.Builder()
            .setIncludeDeviceName(false)
            .addServiceUuid(ParcelUuid(SERVICE_UUID))
            .build()
    }
    
    private fun handleScanResult(result: ScanResult) {
        val device = result.device
        val rssi = result.rssi
        val address = device.address
        val now = System.currentTimeMillis()
        lastDiscoveryAt = now
        
        // Duplicate advertisement detection - avoid processing identical advertisements
        // This improves performance in dense networks
        val advertHash = computeAdvertisementHash(result)
        val cached = recentAdvertisementHashes[address]
        if (cached != null && cached.hash == advertHash && now - cached.timestamp < 1000L) {
            return // Skip duplicate advertisement
        }
        recentAdvertisementHashes[address] = AdvertisementCacheEntry(advertHash, now)
        
        // Prune old advertisement cache entries periodically
        if (recentAdvertisementHashes.size > 100) {
            val cutoff = now - 30_000L
            val iterator = recentAdvertisementHashes.entries.iterator()
            while (iterator.hasNext()) {
                if (iterator.next().value.timestamp < cutoff) {
                    iterator.remove()
                }
            }
        }
        
        // Adaptive scanning: track discoveries for density estimation
        recordDiscoveryForDensity(now)
        
        // Software-based filtering for iOS ↔ Android interoperability
        // Since we scan without a service UUID filter, we filter here instead.
        val scanRecord = result.scanRecord
        val isConnectable = if (Build.VERSION.SDK_INT >= Build.VERSION_CODES.O) {
            result.isConnectable
        } else {
            true // Assume connectable on older Android
        }
        
        if (!shouldProcessDiscoveredDevice(address, scanRecord, rssi, isConnectable, now)) {
            return
        }
        
        // Adaptive scanning: early RSSI filtering in dense networks
        if (shouldFilterByRssi(rssi)) {
            if (logThrottler.shouldLog("adaptive_rssi_filter", intervalMs = 10000)) {
                Log.d(TAG, "Adaptive: filtering weak signal (${rssi}dBm) in dense network ($estimatedVisiblePeerCount peers)")
            }
            return
        }
        
        // Adaptive scanning: probabilistic filtering in very dense networks
        if (shouldProbabilisticallySkip(address)) {
            return // Silently skip to reduce log spam in dense networks
        }
        
        // Extract service information for logging
        val serviceUuids = scanRecord?.serviceUuids
        val serviceData = scanRecord?.getServiceData(ParcelUuid(SERVICE_UUID))
        
        val lastLog = discoveryLogTimestamps[address]
        if (lastLog == null || now - lastLog > 30000) {
            discoveryLogTimestamps[address] = now
            val hasServiceUuid = serviceUuids?.any { it.uuid == SERVICE_UUID } == true
            val hasServiceData = serviceData != null
            Log.d(TAG, "🔍 Discovered device $address RSSI=$rssi (density: $estimatedVisiblePeerCount, hasServiceUuid: $hasServiceUuid, hasServiceData: $hasServiceData)")
            emitDiagnostic(
                "info",
                "Discovered BLE device",
                mapOf(
                    "address" to address,
                    "rssi" to rssi,
                    "connectable" to isConnectable,
                    "visiblePeers" to estimatedVisiblePeerCount,
                    "hasServiceUuid" to hasServiceUuid,
                    "hasServiceData" to hasServiceData,
                    "serviceUuids" to (serviceUuids?.map { it.uuid.toString() } ?: emptyList())
                )
            )
        }
        lastSeenRssi[address] = rssi.toShort()
        val meshMetadata = MeshAdvertisementData.decode(serviceData)
        meshMetadata?.let {
            lastSeenMeshAdvertisements[address] = MeshObservation(it, rssi, now)
        }
        meshController.observeAdvertisement(meshMetadata, rssi)
        pruneMeshObservations(now)

        // When there's no metadata (iOS/Android advertising without service data),
        // still try to connect - metadata will be exchanged via GATT after connection
        val decision = if (meshMetadata == null) {
            // No metadata in advertisement - allow basic connection to exchange info via GATT
            MeshController.MeshDecision(
                intent = ConnectionIntent.INTRA_CLUSTER,
                reason = "no_metadata_in_advert",
                evictPeerId = null
            )
        } else {
            meshController.shouldInitiateOutbound(meshMetadata, rssi)
        }
        
        if (decision.intent == ConnectionIntent.REJECTED) {
            if (logThrottler.shouldLog("mesh_skip_$address", intervalMs = 15000)) {
                Log.v(TAG, "Skipping connection to $address due to ${decision.reason}")
            }
            return
        }
        
        // Adaptive scanning: rate limit connection attempts
        // Skip throttling for first-time discoveries with strong signals for faster connection
        val isFirstDiscovery = !lastSeenMeshAdvertisements.containsKey(address) && connections.getGatt(address) == null
        val hasStrongSignal = rssi >= -70
        
        if (!isFirstDiscovery || !hasStrongSignal) {
            if (shouldThrottleConnection(address, now)) {
                if (logThrottler.shouldLog("adaptive_throttle_$address", intervalMs = 30000)) {
                    Log.d(TAG, "Adaptive: throttling connection to $address")
                }
                return
            }
        } else if (isFirstDiscovery && hasStrongSignal) {
            Log.d(TAG, "Fast-tracking first discovery with strong signal: $address RSSI=$rssi")
            emitDiagnostic("info", "Fast-tracking first discovery", mapOf(
                "address" to address,
                "rssi" to rssi
            ))
        }

        if (!meshController.connectionBudgetAvailable() && decision.evictPeerId != null) {
            evictPeer(decision.evictPeerId, decision.reason)
        }

        val desiredRole = when (decision.intent) {
            ConnectionIntent.INTER_CLUSTER -> MeshRole.BRIDGE
            ConnectionIntent.INTRA_CLUSTER -> MeshRole.MEMBER
            ConnectionIntent.REJECTED -> MeshRole.MEMBER
        }

        if (!meshController.connectionBudgetAvailable()) {
            if (logThrottler.shouldLog("mesh_budget_exhausted", intervalMs = 5000)) {
                Log.d(TAG, "Connection budget exhausted, skipping $address")
            }
            return
        }
        
        // Record the connection attempt for rate limiting
        recordConnectionAttempt(address, now)

        if (currentConnectionCount() >= MAX_CONNECTIONS_PER_DEVICE) {
            if (logThrottler.shouldLog("mesh_conn_cap", intervalMs = 10000)) {
                Log.d(TAG, "Reached max simultaneous connections, skipping $address")
            }
            return
        }

        if (connections.getGatt(address) == null) {
            connections.setPendingRole(address, desiredRole)
            connectToDevice(device)
        } else if (logThrottler.shouldLog("discovery_existing_$address", intervalMs = 30000)) {
            Log.v(TAG, "Device $address already connected/connecting")
        }

        maybeHandleRebalance("scan")
        
        // Check if we should proactively refresh the scan
        maybeProactivelyRefreshScan(now)
    }
    
    /**
     * Determines if a discovered device should be processed.
     * Implements smart filtering since we scan without a service UUID filter
     * (required for iOS ↔ Android interoperability).
     *
     * Accepts:
     * - Devices advertising our service UUID
     * - Devices with our service data
     * - Previously discovered mesh devices
     * - Previously verified peer/device mappings
     * - Strictly rate-limited bootstrap attempts for unknown connectable devices
     */
    private fun shouldProcessDiscoveredDevice(
        address: String,
        scanRecord: android.bluetooth.le.ScanRecord?,
        rssi: Int,
        isConnectable: Boolean,
        now: Long
    ): Boolean {
        // 0. Skip devices previously verified as non-mesh via GATT
        val nonMeshTimestamp = verifiedNonMeshDevices[address]
        if (nonMeshTimestamp != null) {
            if (now - nonMeshTimestamp < NON_MESH_CACHE_TTL_MS) {
                logDiscoveryRejection(address, "non_mesh_cache", now, mapOf("ageMs" to (now - nonMeshTimestamp)))
                return false
            }
            // Entry expired, remove it and allow re-evaluation
            verifiedNonMeshDevices.remove(address)
        }
        
        // 1. Check if device is advertising our service UUID
        val serviceUuids = scanRecord?.serviceUuids
        if (serviceUuids != null) {
            for (uuid in serviceUuids) {
                // Check both full UUID and short form for cross-platform compatibility
                if (uuid.uuid == SERVICE_UUID || uuid.toString().uppercase() == SERVICE_UUID.toString().uppercase()) {
                    if (logThrottler.shouldLog("service_uuid_match_$address", intervalMs = 30_000)) {
                        Log.d(TAG, "✅ Device $address matches service UUID: ${uuid.uuid}")
                    }
                    return true
                }
            }
        }
        
        // Also check service UUIDs in scan record AD structures (for iOS compatibility)
        // iOS sometimes advertises service UUIDs in a format Android's API doesn't parse correctly.
        // Restrict this fallback to 128-bit Service UUID AD fields only.
        val scanRecordBytes = scanRecord?.bytes
        if (scanRecordBytes != null && containsServiceUuidInAdStructures(scanRecordBytes)) {
            if (logThrottler.shouldLog("service_uuid_bytes_match_$address", intervalMs = 30_000)) {
                Log.d(TAG, "✅ Device $address matches service UUID in scan record AD structures")
            }
            return true
        }
        
        // 2. Check for our service data
        val serviceData = scanRecord?.getServiceData(ParcelUuid(SERVICE_UUID))
        if (serviceData != null) {
            return true
        }
        
        // 3. Check if this is a previously discovered mesh device
        if (lastSeenMeshAdvertisements.containsKey(address)) {
            return true
        }
        
        // 4. Check if we already have a device ID mapping for this device
        if (connections.deviceIdForAddress(address) != null) {
            return true
        }
        
        // 5. Check if we have an active GATT connection to this device
        if (connections.getGatt(address) != null) {
            return true
        }

        // 6. Controlled bootstrap for unknown connectable devices.
        // Missing advertisement fields are treated as unknown (not invalid), but we keep
        // strict safeguards to avoid probing arbitrary peripherals.
        if (shouldAllowUnknownBootstrap(address, scanRecord != null, rssi, isConnectable, now)) {
            if (logThrottler.shouldLog("bootstrap_allow_$address", intervalMs = 30_000L)) {
                Log.d(TAG, "Allowing provisional bootstrap for $address (rssi=$rssi, scanRecord=${scanRecord != null})")
                emitDiagnostic("debug", "Allowing provisional bootstrap candidate", mapOf(
                    "address" to address,
                    "rssi" to rssi,
                    "scanRecordPresent" to (scanRecord != null),
                    "connectable" to isConnectable
                ))
            }
            return true
        }
        
        // Filter out all other devices (not our mesh network)
        logDiscoveryRejection(address, "unknown_candidate_blocked", now, mapOf(
            "rssi" to rssi,
            "connectable" to isConnectable,
            "scanRecordPresent" to (scanRecord != null)
        ))
        return false
    }

    private fun containsServiceUuidInAdStructures(scanRecordBytes: ByteArray): Boolean {
        var offset = 0
        while (offset < scanRecordBytes.size) {
            val length = scanRecordBytes[offset].toInt() and 0xFF
            if (length == 0) break

            val nextStructureOffset = offset + length + 1
            if (nextStructureOffset > scanRecordBytes.size) {
                return false
            }
            if (length < 2) {
                offset = nextStructureOffset
                continue
            }

            val adType = scanRecordBytes[offset + 1].toInt() and 0xFF
            if (adType == AD_TYPE_INCOMPLETE_128_BIT_SERVICE_UUIDS || adType == AD_TYPE_COMPLETE_128_BIT_SERVICE_UUIDS) {
                val dataStart = offset + 2
                val dataLength = length - 1
                val uuidCount = dataLength / UUID_128_BIT_LENGTH_BYTES

                for (uuidIndex in 0 until uuidCount) {
                    val uuidOffset = dataStart + (uuidIndex * UUID_128_BIT_LENGTH_BYTES)
                    var matches = true
                    for (byteIndex in 0 until UUID_128_BIT_LENGTH_BYTES) {
                        if (scanRecordBytes[uuidOffset + byteIndex] != SERVICE_UUID_LE_BYTES[byteIndex]) {
                            matches = false
                            break
                        }
                    }
                    if (matches) return true
                }
            }

            offset = nextStructureOffset
        }
        return false
    }

    private fun shouldAllowUnknownBootstrap(
        address: String,
        hasScanRecord: Boolean,
        rssi: Int,
        isConnectable: Boolean,
        now: Long
    ): Boolean {
        val lastAttempt = unknownBootstrapAttempts[address]
        val oneMinuteAgo = now - 60_000L
        val recentBootstrapAttempts = unknownBootstrapAttempts.values.count { it >= oneMinuteAgo }

        val recentConnectionAttempts = synchronized(globalConnectionAttempts) {
            globalConnectionAttempts.count { it >= oneMinuteAgo }
        }
        val shouldAllow = BleDiscoveryBootstrapPolicy.shouldAllowCandidate(
            isConnectable = isConnectable,
            currentConnectionCount = currentConnectionCount(),
            maxConnectionsPerDevice = MAX_CONNECTIONS_PER_DEVICE,
            estimatedVisiblePeerCount = estimatedVisiblePeerCount,
            densePeerThreshold = ADAPTIVE_HIGH_DENSITY_THRESHOLD,
            rssi = rssi,
            hasScanRecord = hasScanRecord,
            minRssiWithScanRecord = UNKNOWN_BOOTSTRAP_MIN_RSSI,
            minRssiWithoutScanRecord = UNKNOWN_BOOTSTRAP_MIN_RSSI_NO_SCAN_RECORD,
            lastAttemptAt = lastAttempt,
            now = now,
            perDeviceCooldownMs = UNKNOWN_BOOTSTRAP_RATE_LIMIT_MS,
            recentBootstrapAttempts = recentBootstrapAttempts,
            maxBootstrapAttemptsPerMinute = MAX_UNKNOWN_BOOTSTRAP_ATTEMPTS_PER_MINUTE,
            recentConnectionAttempts = recentConnectionAttempts,
            maxConnectionAttemptsPerMinute = ADAPTIVE_MAX_CONNECTIONS_PER_MINUTE
        )
        if (!shouldAllow) return false

        unknownBootstrapAttempts[address] = now
        return true
    }

    private fun logDiscoveryRejection(
        address: String,
        reason: String,
        now: Long,
        details: Map<String, Any?> = emptyMap()
    ) {
        if (!logThrottler.shouldLog("reject_${reason}_$address", intervalMs = 30_000L, nowMs = now)) {
            return
        }
        Log.v(TAG, "Skipping discovered device $address ($reason)")
        emitDiagnostic("debug", "Skipping discovered BLE device", details + mapOf(
            "address" to address,
            "reason" to reason
        ))
    }
    
    /**
     * Proactively refreshes the scan periodically to ensure we don't miss devices
     * due to BLE stack issues or cached state.
     */
    private fun maybeProactivelyRefreshScan(now: Long) {
        if (now - lastProactiveScanRefresh >= PROACTIVE_SCAN_REFRESH_MS) {
            lastProactiveScanRefresh = now
            if (logThrottler.shouldLog("proactive_scan_refresh", intervalMs = PROACTIVE_SCAN_REFRESH_MS)) {
                Log.d(TAG, "Proactively refreshing BLE scan")
                emitDiagnostic("info", "Proactive scan refresh")
            }
            restartScanning("proactive_refresh")
        }
        
        // Forced complete BLE refresh - more aggressive than proactive refresh
        // This helps recover from edge cases where the BLE stack becomes stuck
        val lastForced = if (lastForcedBleRefresh == 0L) transportStartAt else lastForcedBleRefresh
        if (now - lastForced >= FORCED_BLE_REFRESH_MS) {
            lastForcedBleRefresh = now
            if (logThrottler.shouldLog("forced_ble_refresh", intervalMs = FORCED_BLE_REFRESH_MS)) {
                Log.i(TAG, "Performing forced BLE refresh for reliability")
                emitDiagnostic("info", "Forced BLE refresh for reliability", mapOf(
                    "connectedPeers" to connections.connectionCount(),
                    "discoveredPeers" to connections.discoveredPeerCount()
                ))
            }
            // Stop and restart both scanning and advertising
            stopScanning("forced_refresh")
            refreshAdvertising("forced_refresh")
            mainHandler.postDelayed({
                if (state == TransportState.RUNNING) {
                    startScanning("forced_refresh")
                }
            }, 500)
        }
    }
    
    /**
     * Computes a hash of the advertisement data for duplicate detection.
     * Uses device address, RSSI bucket, and key advertisement data.
     */
    private fun computeAdvertisementHash(result: ScanResult): Int {
        var hash = result.device.address.hashCode()
        // Use RSSI buckets of 5 dBm to avoid hash changes from minor signal fluctuations
        hash = 31 * hash + (result.rssi / 5)
        
        val scanRecord = result.scanRecord
        if (scanRecord != null) {
            // Include service UUIDs
            scanRecord.serviceUuids?.forEach { uuid ->
                hash = 31 * hash + uuid.hashCode()
            }
            
            // Include service data
            val serviceData = scanRecord.getServiceData(ParcelUuid(SERVICE_UUID))
            if (serviceData != null) {
                hash = 31 * hash + serviceData.contentHashCode()
            }
        }
        
        return hash
    }
    
    /** Lock for atomic connection count check and connect operations */
    private val connectionLock = Any()
    
    private fun connectToDevice(device: BluetoothDevice) {
        try {
            // Atomic check-and-connect to prevent race conditions
            synchronized(connectionLock) {
                // Check RSSI threshold - don't connect to devices with weak signals
                val rssi = lastSeenRssi[device.address]?.toInt() ?: -60
                if (rssi < MINIMUM_RSSI_TO_CONNECT) {
                    if (logThrottler.shouldLog("rssi_skip_${device.address}", intervalMs = 10000)) {
                        Log.d(TAG, "Skipping connection to ${device.address} due to weak RSSI ($rssi < $MINIMUM_RSSI_TO_CONNECT)")
                        emitDiagnostic("debug", "Skipping BLE connect due to weak RSSI", mapOf(
                            "address" to device.address,
                            "rssi" to rssi,
                            "threshold" to MINIMUM_RSSI_TO_CONNECT
                        ))
                    }
                    connections.consumePendingRole(device.address)
                    return
                }
                
                if (currentConnectionCount() >= MAX_CONNECTIONS_PER_DEVICE) {
                    if (logThrottler.shouldLog("mesh_conn_cap", intervalMs = 10000)) {
                        Log.d(TAG, "Connection cap reached, not connecting to ${device.address}")
                    }
                    connections.consumePendingRole(device.address)
                    return
                }
                
                // Double-check we don't already have a connection to this device
                if (connections.getGatt(device.address) != null) {
                    if (logThrottler.shouldLog("already_connecting_${device.address}", intervalMs = 5000)) {
                        Log.d(TAG, "Already have GATT client for ${device.address}")
                    }
                    return
                }
                
                val gatt = device.connectGatt(context, false, gattClientCallback, BluetoothDevice.TRANSPORT_LE)
                connections.registerGatt(device.address, gatt)
            }
            
            Log.i(TAG, "Connecting to device: ${device.address}")
            emitDiagnostic("info", "Connecting to BLE device", mapOf("address" to device.address))
        } catch (e: SecurityException) {
            Log.e(TAG, "Permission denied while connecting to device", e)
            emitDiagnostic("error", "Permission denied while connecting to device", mapOf("exception" to e.javaClass.simpleName, "message" to (e.message ?: "unknown")))
            connections.consumePendingRole(device.address)
        }
    }

    private fun currentConnectionCount(): Int = connections.connectionCount()

    private fun refreshSelfMetrics() {
        val rssiValues = lastSeenRssi.values.map { it.toInt() }
        val averageRssi = if (rssiValues.isEmpty()) null else rssiValues.average().roundToInt()
        val signalQuality = averageRssi?.let { rssi ->
            (((rssi + 100).coerceIn(-100, -20) + 100) / 80.0 * 100).roundToInt().coerceIn(0, 100)
        }
        val pendingCount = synchronized(pendingFragmentsLock) { pendingFragments.values.sumOf { it.size } }
        // Read the aggregate counter instead of iterating ArrayDeques. The
        // ArrayDeques themselves are main-thread-only; this getter is safe to
        // call from any thread because the counter is an AtomicInteger updated
        // in lockstep with every enqueue/dequeue/evict.
        val outboundPending = totalPendingOutboundFragments.get()
        val totalPending = pendingCount + outboundPending
        val stability = 1.0 - min(1.0, pendingCount / 10.0)
        val batteryPercent = currentBatteryPercent()
        val loadPercent = ((totalPending.coerceAtMost(LOAD_SATURATION_COUNT) * 100) / LOAD_SATURATION_COUNT).coerceIn(0, 100)
        val uptimeSeconds = if (transportStartAt == 0L) null else ((System.currentTimeMillis() - transportStartAt) / 1000).coerceAtLeast(0)

        meshController.updateSelfMetrics(
            MeshController.PeerMetrics(
                rssi = averageRssi,
                batteryPercent = batteryPercent,
                signalQuality = signalQuality,
                stability = stability,
                uptimeSeconds = uptimeSeconds?.toLong(),
                loadPercent = loadPercent
            )
        )
        meshController.markPeerActive(deviceId)
        maybeHandleRebalance("self_metrics")
    }

    private fun currentBatteryPercent(): Int? {
        return try {
            val manager = context.getSystemService(Context.BATTERY_SERVICE) as? BatteryManager
            val capacity = manager?.getIntProperty(BatteryManager.BATTERY_PROPERTY_CAPACITY) ?: return null
            capacity.takeIf { it in 0..100 }
        } catch (_: Exception) {
            null
        }
    }

    private fun evictPeer(peerId: String, reason: String) {
        val address = connections.addressForDevice(peerId)
        if (address == null) {
            if (logThrottler.shouldLog("mesh_evict_missing_$peerId")) {
                Log.w(TAG, "Cannot evict $peerId: no known address")
            }
            return
        }

        if (logThrottler.shouldLog("mesh_evict_$peerId", intervalMs = 5000)) {
            Log.i(TAG, "Evicting peer $peerId to reclaim capacity (reason=$reason)")
        }

        connections.getGatt(address)?.let { gatt ->
            try {
                gatt.disconnect()
                gatt.close()
            } catch (e: Exception) {
                Log.w(TAG, "Error while evicting $peerId", e)
            }
        }

        connections.removeGatt(address)
        linkReady.remove(address)
        connections.removeIdentifiersForDevice(peerId)
        connections.removeConnectionRole(peerId)
        lastSeenRssi.remove(address)
        synchronized(pendingFragmentsLock) { pendingFragments.remove(address) }
        // Posted to main because ArrayDeque contents are main-thread-only.
        // If we're already on main this runs inline, so the invariant holds
        // whether eviction is triggered by scan (callback thread) or rebalance.
        runOnMain {
            pendingOutboundFragments.remove(peerId)?.let { removed ->
                if (removed.isNotEmpty()) {
                    totalPendingOutboundFragments.addAndGet(-removed.size)
                }
            }
        }
        deviceIdResolutionAttempts.remove(address)
        connectionRetryCount.remove(address)
        meshController.registerDisconnection(peerId)
        refreshSelfMetrics()

        // Clean up routes through this neighbor
        protocol.removeNeighborRoutes(peerId)
        
        try {
            protocol.blePeerLost(peerId)
        } catch (e: Exception) {
            Log.e(TAG, "Failed to notify protocol of peer eviction", e)
        }

        refreshAdvertising("evict_$reason")
        maybeHandleRebalance("evict")
    }
    
    /**
     * Called by the Rust transport callback when new outgoing fragments are available.
     * This is the primary send path, replacing the 100ms polling loop.
     * Posts to mainHandler to ensure all BLE operations run on the main thread.
     */
    fun onFragmentsAvailable() {
        mainHandler.post { drainAndSendFragments() }
    }

    /**
     * Drains the Rust fragment queue and sends each fragment over BLE.
     * Stops when the queue is empty or all target peers are flow-controlled.
     * Called from onFragmentsAvailable() and from the fallback polling timer.
     */
    private fun drainAndSendFragments() {
        if (state != TransportState.RUNNING) return

        try {
            flushPendingOutboundFragments()

            var consecutiveSkips = 0
            val maxConsecutiveSkips = 5
            val reconnectAttempted = mutableSetOf<String>()

            while (true) {
                val fragment = try {
                    protocol.bleGetNextFragment()
                } catch (e: Exception) {
                    Log.e(TAG, "Error calling bleGetNextFragment(): ${e.message}", e)
                    return
                } ?: break

                val recipientId = fragment.recipientId
                val data = fragment.data.map { it.toByte() }.toByteArray()

                val address = resolveTargetAddress(recipientId)
                val hasConnection = address?.let { connections.getGatt(it) } != null
                if (!hasConnection) {
                    enqueuePendingOutboundFragment(recipientId, data)
                    // Proactively attempt reconnection if we know the address (once per peer per drain)
                    if (address != null && reconnectAttempted.add(address)) {
                        bluetoothAdapter?.let { adapter ->
                            try {
                                val device = adapter.getRemoteDevice(address)
                                connectToDevice(device)
                            } catch (e: Exception) {
                                Log.e(TAG, "Error attempting reconnection for $recipientId during fragment drain", e)
                            }
                        }
                    }
                    consecutiveSkips++
                    if (consecutiveSkips >= maxConsecutiveSkips) {
                        break
                    }
                    continue
                }

                // Maintain FIFO ordering: if this recipient has pending fragments,
                // enqueue instead of sending directly.
                if (pendingOutboundFragments[recipientId]?.isNotEmpty() == true) {
                    enqueuePendingOutboundFragment(recipientId, data)
                    continue
                }

                consecutiveSkips = 0

                if (!sendFragmentData(recipientId, data)) {
                    enqueuePendingOutboundFragment(recipientId, data)
                    break
                }
            }
        } catch (e: Exception) {
            Log.e(TAG, "Error in drainAndSendFragments", e)
        }
    }

    private fun pollAndSendFragments() {
        try {
            // The old logic would return early if there were unsent fragments, preventing new fragments
            // from being polled. This caused messages to get stuck when connections weren't ready.
            val hasUnsentFragments = flushPendingOutboundFragments()
            
            // Still poll for new fragments even if there are unsent pending ones
            // This prevents deadlock where old fragments block new ones
            // Poll for next fragment from protocol
            val fragment = try {
                protocol.bleGetNextFragment()
            } catch (e: Exception) {
                Log.e(TAG, "Error calling bleGetNextFragment(): ${e.message}", e)
                emitDiagnostic("error", "Error calling bleGetNextFragment", mapOf(
                    "error" to (e.message ?: "unknown"),
                    "exception" to e.javaClass.simpleName
                ))
                return
            }
            
            if (fragment == null) {
                // No fragment available - this is normal most of the time
                // But log if we have unsent fragments to help diagnose connection issues
                if (hasUnsentFragments && logThrottler.shouldLog("unsent_fragments_no_new", intervalMs = 5000)) {
                    val recipientCount = pendingOutboundFragments.size
                    Log.w(TAG, "⚠️ Have $recipientCount recipients with unsent fragments, but no new fragments to poll")
                    emitDiagnostic("warning", "Unsent fragments blocking", mapOf(
                        "recipientCount" to recipientCount,
                        "recipients" to pendingOutboundFragments.keys.toList()
                    ))
                } else if (logThrottler.shouldLog("no_fragments", intervalMs = 10000)) {
                    Log.d(TAG, "No fragments available from protocol")
                }
                return
            }
            
            Log.i(TAG, "🚀 GOT FRAGMENT for recipient: ${fragment.recipientId}, size: ${fragment.data.size}")
            emitDiagnostic("debug", "Polling got fragment", mapOf(
                "recipientId" to fragment.recipientId,
                "fragmentSize" to fragment.data.size
            ))
            
            val recipientId = fragment.recipientId
            val data = fragment.data.map { it.toByte() }.toByteArray()

            // Maintain FIFO ordering: if this recipient has pending fragments,
            // enqueue instead of sending directly.
            if (pendingOutboundFragments[recipientId]?.isNotEmpty() == true) {
                enqueuePendingOutboundFragment(recipientId, data)
                return
            }

            val sendResult = sendFragmentData(recipientId, data)
            Log.d(TAG, "Fragment send result for $recipientId: $sendResult")
            
            if (!sendResult) {
                Log.w(TAG, "Failed to send fragment immediately, queuing for retry")
                enqueuePendingOutboundFragment(recipientId, data)
            } else {
                Log.d(TAG, "Fragment sent successfully to $recipientId")
                emitDiagnostic("debug", "Fragment sent successfully", mapOf("recipientId" to recipientId))
            }
        } catch (e: Exception) {
            Log.e(TAG, "Error polling/sending fragments", e)
            emitDiagnostic("error", "Error sending BLE fragment", mapOf("exception" to e.javaClass.simpleName, "message" to (e.message ?: "unknown")))
        }
    }

    private fun flushPendingOutboundFragments(): Boolean {
        var hasUnsentFragments = false
        val now = System.currentTimeMillis()

        val recipients = pendingOutboundFragments.keys.toList()
        for (recipientId in recipients) {
            val queue = pendingOutboundFragments[recipientId] ?: continue

            // Drop expired fragments in place to keep the counter consistent
            // with the deques. `removeIf` on ArrayDeque mutates the backing
            // storage directly, so we avoid the allocate-new-deque dance.
            var expired = 0
            val iter = queue.iterator()
            while (iter.hasNext()) {
                val fragment = iter.next()
                if (now - fragment.timestamp >= PENDING_OUTBOUND_FRAGMENT_TIMEOUT_MS) {
                    iter.remove()
                    expired++
                }
            }
            if (expired > 0) {
                totalPendingOutboundFragments.addAndGet(-expired)
                if (logThrottler.shouldLog("fragments_expired_$recipientId", intervalMs = 10000)) {
                    Log.w(TAG, "⚠️ Dropped $expired expired outbound fragments for $recipientId")
                    emitDiagnostic("warning", "Outbound fragments expired", mapOf(
                        "recipientId" to recipientId,
                        "expired" to expired,
                    ))
                }
            }

            if (queue.isEmpty()) {
                pendingOutboundFragments.remove(recipientId)
                continue
            }

            // Try to send each remaining fragment in FIFO order
            val sendIter = queue.iterator()
            while (sendIter.hasNext()) {
                val fragment = sendIter.next()
                if (sendFragmentData(recipientId, fragment.data)) {
                    sendIter.remove()
                    totalPendingOutboundFragments.decrementAndGet()
                } else {
                    hasUnsentFragments = true
                    break
                }
            }

            if (queue.isEmpty()) {
                pendingOutboundFragments.remove(recipientId)
            }
        }

        return hasUnsentFragments
    }

    private fun enqueuePendingOutboundFragment(recipientId: String, data: ByteArray) {
        val queue = pendingOutboundFragments.getOrPut(recipientId) { ArrayDeque() }
        queue.addLast(OutboundFragment(data, System.currentTimeMillis()))
        totalPendingOutboundFragments.incrementAndGet()
        // Drop oldest fragment if the queue exceeds the per-peer cap
        if (queue.size > MAX_PENDING_FRAGMENTS_PER_PEER) {
            queue.removeFirst()
            totalPendingOutboundFragments.decrementAndGet()
            Log.w(TAG, "Pending outbound fragment queue capped for $recipientId, dropping oldest (max=$MAX_PENDING_FRAGMENTS_PER_PEER)")
        }
    }

    private fun resolveTargetAddress(recipientId: String): String? {
        if (recipientId == deviceId) {
            return null
        }
        connections.addressForDevice(recipientId)?.let { return it }
        connections.connectionRoleEntries()
            .sortedBy { entry -> if (entry.value == MeshRole.BRIDGE) 0 else 1 }
            .firstOrNull()
            ?.key
            ?.let { return connections.addressForDevice(it) }
        return null
    }

    private fun sendFragmentData(recipientId: String, data: ByteArray): Boolean {
        // Find GATT client for recipient
        val address = resolveTargetAddress(recipientId)
        val gatt = address?.let { connections.getGatt(it) }
        
        // Until the remote has ack'd our CCCD write, the return path is not
        // verified and the BLE stack may still be executing the setup ops
        // (deviceId read → identity read → writeDescriptor). Issuing a write
        // now risks either silent loss (on stacks that drop the op) or
        // stalling the chain. Enqueue and let onDescriptorWrite trigger the
        // drain.
        if (address != null && !linkReady.contains(address)) {
            if (logThrottler.shouldLog("link_not_ready_$recipientId", intervalMs = 5000)) {
                Log.d(TAG, "Link to $recipientId not yet ready (CCCD unacked), deferring write")
            }
            return false
        }

        if (gatt == null) {
            //  Proactively try to connect if we don't have a connection
            // This helps resolve cases where fragments are queued but connection isn't established
            if (logThrottler.shouldLog("missing_gatt_$recipientId", intervalMs = 5000)) {
                Log.w(TAG, "⚠️ No connected device for recipient: $recipientId - attempting to find and connect")
                emitDiagnostic("warning", "No connected device for BLE fragment - attempting connection", mapOf("recipientId" to recipientId))
            }
            
            // Try to find the device and connect
            if (address != null) {
                // We know the address but don't have a connection - try to reconnect
                bluetoothAdapter?.let { adapter ->
                    try {
                        val device = adapter.getRemoteDevice(address)
                        connectToDevice(device)
                    } catch (e: Exception) {
                        Log.e(TAG, "Error attempting reconnection for $recipientId", e)
                    }
                }
            } else {
                // We don't even know the address - this is a more serious issue
                // The device ID might not be resolved yet or route might not exist
                Log.w(TAG, "⚠️ Cannot resolve address for recipient: $recipientId")
            }
            return false
        }
        
        //  Validate connection state before attempting to send
        if (gatt.device.bondState == BluetoothDevice.BOND_NONE) {
            // Device is not bonded - this might be okay for BLE, but log it
            if (logThrottler.shouldLog("unbonded_device_$recipientId", intervalMs = 10000)) {
                Log.d(TAG, "Device $recipientId is not bonded (this may be normal for BLE)")
            }
        }
        
        val service = gatt.getService(SERVICE_UUID)
        val characteristic = service?.getCharacteristic(MESSAGE_CHAR_UUID)
        
        if (service == null || characteristic == null) {
            if (logThrottler.shouldLog("missing_char_$recipientId")) {
                Log.w(TAG, "Message characteristic not found for recipient: $recipientId")
                emitDiagnostic("warning", "Message characteristic missing", mapOf("recipientId" to recipientId))
            }
            return false
        }

        characteristic.writeType = BluetoothGattCharacteristic.WRITE_TYPE_NO_RESPONSE

        // On API 33+ we use the value-parameter overload, which returns a
        // BluetoothStatusCodes result. On older APIs we have to set the shared
        // characteristic value field and call the legacy overload — its
        // Boolean return *is* load-bearing: it reports false when the internal
        // TX queue is full, another GATT op is in flight, or the characteristic
        // is unwritable. Treating every pre-Tiramisu call as success silently
        // drops fragments, which is exactly the class of bug the rest of this
        // code is trying to defend against.
        val writeOk = try {
            if (Build.VERSION.SDK_INT >= Build.VERSION_CODES.TIRAMISU) {
                val result = gatt.writeCharacteristic(characteristic, data, BluetoothGattCharacteristic.WRITE_TYPE_NO_RESPONSE)
                if (result != BluetoothStatusCodes.SUCCESS) {
                    Log.w(TAG, "Write characteristic returned non-success status: $result for recipient: $recipientId")
                    emitDiagnostic("warning", "BLE write returned non-success status", mapOf(
                        "recipientId" to recipientId,
                        "status" to result.toString()
                    ))
                    false
                } else {
                    true
                }
            } else {
                @Suppress("DEPRECATION")
                characteristic.value = data
                @Suppress("DEPRECATION")
                val queued = gatt.writeCharacteristic(characteristic)
                if (!queued) {
                    emitDiagnostic("warning", "BLE writeCharacteristic returned false", mapOf(
                        "recipientId" to recipientId,
                    ))
                }
                queued
            }
        } catch (e: Exception) {
            Log.e(TAG, "Error writing characteristic to $recipientId", e)
            emitDiagnostic("error", "Error writing BLE fragment", mapOf(
                "recipientId" to recipientId,
                "exception" to e.javaClass.simpleName,
                "message" to (e.message ?: "unknown")
            ))
            false
        }

        if (!writeOk) {
            if (logThrottler.shouldLog("write_failed_$recipientId", intervalMs = 2000)) {
                Log.w(TAG, "Failed to write BLE fragment for $recipientId")
                emitDiagnostic("warning", "Failed to write BLE fragment", mapOf("recipientId" to recipientId))
            }
            return false
        }
        
        // Write was initiated successfully (actual completion is asynchronous for WRITE_TYPE_NO_RESPONSE)
        bytesSent += data.size
        fragmentsSent++
        meshController.markPeerActive(recipientId)
        meshController.markPeerActive(deviceId)
        return true
    }
    
    private fun handleReceivedData(data: ByteArray, address: String) {
        try {
            // Get sender device ID
            val senderId = connections.deviceIdForAddress(address)
            if (senderId == null) {
                // Queue fragment to process later when device ID is available
                synchronized(pendingFragmentsLock) {
                    val pendingList = pendingFragments.getOrPut(address) { mutableListOf() }
                    if (pendingList.size >= MAX_PENDING_FRAGMENTS_PER_PEER) {
                        pendingList.removeFirst()
                    }
                    pendingList.add(PendingFragment(data, System.currentTimeMillis()))
                }
                if (logThrottler.shouldLog("queue_pending_$address")) {
                    Log.d(TAG, "Queued fragment while awaiting device ID for $address")
                    emitDiagnostic(
                        "info",
                        "Queued BLE fragment pending device ID",
                        mapOf("address" to address, "length" to data.size)
                    )
                }
                
                // Proactively attempt to resolve the device ID by initiating a client connection
                bluetoothAdapter?.let { adapter ->
                    try {
                        val device = adapter.getRemoteDevice(address)
                        mainHandler.post {
                            val hasGattClient = connections.getGatt(device.address) != null
                            val mappedId = connections.deviceIdForAddress(device.address)
                            val now = System.currentTimeMillis()
                            val lastAttempt = deviceIdResolutionAttempts[address] ?: 0L
                            val shouldAttempt = now - lastAttempt > PENDING_FRAGMENT_TIMEOUT_MS
                            if ((!hasGattClient || mappedId.isNullOrEmpty()) && shouldAttempt) {
                                deviceIdResolutionAttempts[address] = now
                                if (logThrottler.shouldLog("resolve_device_$address", intervalMs = 5000)) {
                                    Log.d(TAG, "Attempting to resolve device ID for $address via client connection")
                                    emitDiagnostic(
                                        "debug",
                                        "Resolving BLE sender device ID",
                                        mapOf("address" to address, "hasGattClient" to hasGattClient, "knownId" to (mappedId != null))
                                    )
                                }
                                connectToDevice(device)
                            }
                        }
                    } catch (e: IllegalArgumentException) {
                        if (logThrottler.shouldLog("resolve_device_error_$address", intervalMs = 10000)) {
                            Log.w(TAG, "Failed to obtain remote device for address $address", e)
                            emitDiagnostic(
                                "warning",
                                "Failed to resolve BLE device for pending fragment",
                                mapOf("address" to address, "message" to (e.message ?: "unknown"))
                            )
                        }
                    }
                }

                // Clean up old pending fragments
                cleanupPendingFragments()
                return
            }

            // If pending fragments exist for this address, append to maintain FIFO ordering.
            // processPendingFragments() will handle them all in order.
            synchronized(pendingFragmentsLock) {
                val list = pendingFragments[address]
                if (list != null && list.isNotEmpty()) {
                    if (list.size >= MAX_PENDING_FRAGMENTS_PER_PEER) {
                        list.removeFirst()
                    }
                    list.add(PendingFragment(data, System.currentTimeMillis()))
                    return
                }
            }

            lastSeenRssi[address]?.toInt()?.let { observedRssi ->
                meshController.updatePeerMetrics(
                    senderId,
                    MeshController.PeerMetrics(rssi = observedRssi)
                )
            }
            meshController.markPeerActive(senderId)
            meshController.markPeerActive(deviceId)

            // Convert to UByte list
            val bytes = data.map { it.toUByte() }
            
            // Pass to protocol
            Log.i(TAG, "📥 RECEIVED FRAGMENT from $senderId, size: ${data.size}")
            emitDiagnostic("info", "Fragment received from BLE", mapOf(
                "senderId" to senderId,
                "fragmentSize" to data.size
            ))
            
            try {
                protocol.bleFragmentReceived(senderId, bytes)
                Log.i(TAG, "✅ Fragment processed successfully for sender: $senderId")
                
                // Drain all completed messages (a fragment may complete multiple messages)
                var completedMessage = protocol.receiveMessage()
                if (completedMessage == null) {
                    Log.d(TAG, "📦 Fragment processed, waiting for more fragments to complete message")
                }
                while (completedMessage != null) {
                    Log.i(TAG, "🎉 COMPLETE MESSAGE ASSEMBLED FROM FRAGMENTS!")
                    Log.i(TAG, "📬 Received message: $completedMessage")
                    emitDiagnostic("info", "Complete message assembled from fragments", mapOf(
                        "senderId" to senderId,
                        "messageContent" to completedMessage
                    ))
                    learnRouteFromMessage(completedMessage, senderId, address)
                    completedMessage = protocol.receiveMessage()
                }
            } catch (e: Exception) {
                Log.e(TAG, "❌ Error processing fragment from $senderId: ${e.message}", e)
                emitDiagnostic("error", "Error processing received fragment", mapOf(
                    "senderId" to senderId,
                    "fragmentSize" to data.size,
                    "error" to (e.message ?: "unknown"),
                    "exception" to e.javaClass.simpleName
                ))
            }
            
            bytesReceived += data.size
            fragmentsReceived++
        } catch (e: Exception) {
            Log.e(TAG, "Error processing received fragment", e)
            emitDiagnostic("error", "Error processing received fragment", mapOf("exception" to e.javaClass.simpleName, "message" to (e.message ?: "unknown")))
        }
    }
    
    private fun processPendingFragments(address: String, deviceId: String) {
        deviceIdResolutionAttempts.remove(address)
        val fragments = synchronized(pendingFragmentsLock) { pendingFragments.remove(address) } ?: return
        val role = connections.consumePendingRole(address) ?: MeshRole.MEMBER
        meshController.registerConnection(deviceId, role)
        connections.setConnectionRole(deviceId, role)
        meshController.markPeerActive(deviceId)
        meshController.markPeerActive(this.deviceId)
        refreshSelfMetrics()
        mainHandler.post {
            if (state == TransportState.RUNNING) {
                refreshAdvertising("membership_change")
            }
        }
        
        for (fragment in fragments) {
            try {
                val bytes = fragment.data.map { it.toUByte() }
                protocol.bleFragmentReceived(deviceId, bytes)
                bytesReceived += fragment.data.size
                fragmentsReceived++

                // Drain all completed messages (a multi-fragment message may complete here)
                var msg = protocol.receiveMessage()
                while (msg != null) {
                    Log.i(TAG, "🎉 Complete message assembled from queued fragments for $deviceId")
                    emitDiagnostic("info", "Complete message assembled from queued fragments", mapOf(
                        "senderId" to deviceId,
                        "messageContent" to msg
                    ))
                    learnRouteFromMessage(msg, deviceId, address)
                    msg = protocol.receiveMessage()
                }
            } catch (e: Exception) {
                Log.e(TAG, "Error processing pending fragment", e)
            }
        }
    }
    
    private fun cleanupPendingFragments() {
        synchronized(pendingFragmentsLock) {
            val now = System.currentTimeMillis()
            val addressesToRemove = mutableListOf<String>()
            for ((address, fragments) in pendingFragments) {
                fragments.removeAll { now - it.timestamp > PENDING_FRAGMENT_TIMEOUT_MS }
                if (fragments.isEmpty()) {
                    addressesToRemove.add(address)
                }
            }
            addressesToRemove.forEach { pendingFragments.remove(it) }
        }
    }

    private fun pruneMeshObservations(now: Long) {
        val iterator = lastSeenMeshAdvertisements.entries.iterator()
        while (iterator.hasNext()) {
            val entry = iterator.next()
            if (now - entry.value.timestamp > MESH_OBSERVATION_TTL_MS) {
                iterator.remove()
            }
        }

        val unknownIterator = unknownBootstrapAttempts.entries.iterator()
        while (unknownIterator.hasNext()) {
            if (now - unknownIterator.next().value > 60_000L) {
                unknownIterator.remove()
            }
        }
    }
    
    // MARK: - Gradient Routing
    
    /** Computes route quality from RSSI value (0.0 to 1.0) */
    private fun computeRouteQuality(rssi: Int?): Float {
        if (rssi == null) return 0.5f
        // Map RSSI from [-100, -20] to [0.0, 1.0]
        val clamped = rssi.coerceIn(-100, -20)
        return (clamped + 100).toFloat() / 80f
    }
    
    /** Learns a route from a received message */
    private fun learnRouteFromMessage(messageJson: String, neighborId: String, neighborAddress: String?) {
        try {
            val json = org.json.JSONObject(messageJson)
            val sender = json.optNullableString("sender") ?: return
            val hopCount = json.optInt("hop_count", 0)
            
            // Don't learn route to ourselves
            if (sender == deviceId) return
            
            // Compute quality from RSSI
            val rssi = neighborAddress?.let { lastSeenRssi[it]?.toInt() }
            val quality = computeRouteQuality(rssi)
            
            // Learn the route: sender can be reached through neighborId (sequence_number from message or 0)
            val seqNum = json.optInt("sequence_number", 0).coerceAtLeast(0).toUInt()
            protocol.learnRoute(
                sender,
                neighborId,
                minOf(255, hopCount + 1).toUByte(),
                quality,
                seqNum
            )
        } catch (e: Exception) {
            Log.w(TAG, "Failed to learn route from message: ${e.message}")
        }
    }
    
    // MARK: - Adaptive Scan Methods
    
    /** Updates the estimated visible peer count based on recent discoveries. */
    private fun updateVisiblePeerCount(now: Long) {
        // Only update periodically to avoid overhead
        if (now - lastPeerCountUpdate < 1000L) {
            return
        }
        lastPeerCountUpdate = now
        
        // Clean up old timestamps
        val windowStart = now - ADAPTIVE_PEER_COUNT_WINDOW_MS
        synchronized(recentDiscoveryTimestamps) {
            recentDiscoveryTimestamps.removeAll { it < windowStart }
        }
        
        // Estimate peer count from unique discoveries in window
        val recentCount = recentDiscoveryTimestamps.size
        val cachedCount = lastSeenMeshAdvertisements.size
        estimatedVisiblePeerCount = maxOf(recentCount, cachedCount)
    }
    
    /** Records a device discovery for density estimation. */
    private fun recordDiscoveryForDensity(now: Long) {
        recentDiscoveryTimestamps.add(now)
        updateVisiblePeerCount(now)
    }
    
    /** Checks if we should skip this device based on RSSI filtering. */
    private fun shouldFilterByRssi(rssi: Int): Boolean {
        // During aggressive discovery phase, don't apply density-based filtering
        val now = System.currentTimeMillis()
        if (transportStartAt > 0 && now - transportStartAt < AGGRESSIVE_DISCOVERY_PHASE_MS) {
            // Only filter out extremely weak signals during aggressive phase
            return rssi < MINIMUM_RSSI_TO_CONNECT
        }
        
        // In dense networks, apply stricter RSSI filtering
        val threshold = when {
            estimatedVisiblePeerCount > ADAPTIVE_HIGH_DENSITY_THRESHOLD -> -70
            estimatedVisiblePeerCount > ADAPTIVE_LOW_DENSITY_THRESHOLD -> ADAPTIVE_MIN_RSSI
            else -> return false // Sparse network - accept all signals
        }
        return rssi < threshold
    }
    
    /** Checks if we should throttle connection attempts based on rate limits. */
    private fun shouldThrottleConnection(address: String, now: Long): Boolean {
        // During aggressive discovery phase, use much shorter cooldowns
        val isAggressivePhase = transportStartAt > 0 && now - transportStartAt < AGGRESSIVE_DISCOVERY_PHASE_MS
        
        // Prune old entries
        val oneMinuteAgo = now - 60_000L
        synchronized(globalConnectionAttempts) {
            globalConnectionAttempts.removeAll { it < oneMinuteAgo }
        }
        
        val effectiveCooldown = if (isAggressivePhase) 5_000L else ADAPTIVE_COOLDOWN_PER_DEVICE_MS
        deviceConnectionAttempts.entries.removeIf { 
            now - it.value >= effectiveCooldown 
        }
        
        // Check per-device cooldown
        val lastAttempt = deviceConnectionAttempts[address]
        if (lastAttempt != null && now - lastAttempt < effectiveCooldown) {
            return true
        }
        
        // During aggressive phase, allow more connection attempts
        if (isAggressivePhase) {
            // Allow up to 3x the normal rate during aggressive phase
            val maxAttempts = ADAPTIVE_MAX_CONNECTIONS_PER_MINUTE * 3
            if (globalConnectionAttempts.size >= maxAttempts) {
                return true
            }
            return false
        }
        
        // In dense networks, apply global rate limiting
        if (estimatedVisiblePeerCount > ADAPTIVE_LOW_DENSITY_THRESHOLD) {
            val currentAttempts = globalConnectionAttempts.size
            if (currentAttempts >= ADAPTIVE_MAX_CONNECTIONS_PER_MINUTE) {
                if (logThrottler.shouldLog("adaptive_rate_limit", intervalMs = 5000)) {
                    Log.d(TAG, "Adaptive: rate limiting connections ($currentAttempts/$ADAPTIVE_MAX_CONNECTIONS_PER_MINUTE in last minute)")
                }
                return true
            }
        }
        
        return false
    }
    
    /** Records a connection attempt for rate limiting. */
    private fun recordConnectionAttempt(address: String, now: Long) {
        deviceConnectionAttempts[address] = now
        globalConnectionAttempts.add(now)
    }
    
    /** Returns true if we should apply probabilistic filtering based on network density. */
    private fun shouldProbabilisticallySkip(address: String): Boolean {
        if (estimatedVisiblePeerCount <= ADAPTIVE_LOW_DENSITY_THRESHOLD) {
            return false
        }
        
        // Calculate skip probability based on density
        val density = (estimatedVisiblePeerCount - ADAPTIVE_LOW_DENSITY_THRESHOLD).toDouble()
        val range = (ADAPTIVE_HIGH_DENSITY_THRESHOLD - ADAPTIVE_LOW_DENSITY_THRESHOLD).toDouble()
        val skipProbability = minOf(0.8, density / range * 0.8)
        
        // Use address hash for deterministic selection
        val hash = address.hashCode()
        val normalizedHash = (kotlin.math.abs(hash) % 1000) / 1000.0
        
        return normalizedHash < skipProbability
    }

    private fun addressForNodeHash(nodeHash: Long): String? {
        return lastSeenMeshAdvertisements.entries.firstOrNull {
            it.value.advertisement.nodeIdHash == nodeHash
        }?.key
    }

    private fun maybeHandleRebalance(trigger: String) {
        val directive = meshController.evaluateRebalance() ?: return
        val decision = directive.decision
        val candidateHash = directive.candidate.nodeIdHash
        val candidateAddress = addressForNodeHash(candidateHash)
        if (candidateAddress == null) {
            if (logThrottler.shouldLog("rebalance_missing_candidate", intervalMs = 10_000)) {
                Log.v(TAG, "No address found for rebalance candidate hash=${candidateHash.toString(16)}")
            }
            return
        }

        if (decision.evictPeerId != null) {
            evictPeer(decision.evictPeerId, "rebalance_${trigger}")
        }

        if (!meshController.connectionBudgetAvailable() && decision.evictPeerId == null) {
            return
        }

        if (connections.getGatt(candidateAddress) != null) {
            return
        }

        val device = try {
            bluetoothAdapter?.getRemoteDevice(candidateAddress)
        } catch (e: IllegalArgumentException) {
            null
        } ?: return

        val desiredRole = when (decision.intent) {
            ConnectionIntent.INTER_CLUSTER -> MeshRole.BRIDGE
            ConnectionIntent.INTRA_CLUSTER, ConnectionIntent.REJECTED -> MeshRole.MEMBER
        }

        connections.setPendingRole(candidateAddress, desiredRole)
        connectToDevice(device)
    }
    
    // MARK: - GATT Server Listener
    //
    // Bridges PeripheralGattServer callbacks into facade state. Callbacks
    // fire on the platform's binder thread; every handler that touches
    // mutable transport state is reposted on [mainHandler] before running,
    // matching the threading model used by start/stop/pause/resume.
    //
    // The `provide*` hooks stay on the binder thread by necessity — they
    // must return bytes synchronously — but they are **pure reads** of
    // @Volatile fields. They must not call into UniFFI or any other path
    // that can block on the protocol mutex, because stalling a GATT binder
    // callback delays every pending operation for that central and risks
    // the system ANR watchdog. Producers (MLS init, advertisement rebuild)
    // call [updateSignedIdentity] on the main thread to refresh the cache
    // *before* the read lands.

    private val gattServerListener = object : PeripheralGattServer.Listener {
        override fun onReady() {
            // LeAdvertiser owns its own pending-reason latch; just drain it.
            leAdvertiser.onGattServerReady()
        }

        override fun onSetupFailed(reason: String) {
            Log.e(TAG, "GATT server setup failed: $reason")
            emitDiagnostic(
                "error",
                "gatt_server_setup_failed",
                mapOf("reason" to reason),
            )
            listener?.onTransportError(
                this@BleTransportFacade,
                TransportException.StartFailed("GATT server setup failed: $reason"),
            )
            // Tear the transport down so the caller sees a coherent stopped
            // state. Without this the facade stays in RUNNING while the GATT
            // server is gone, scans keep firing, and every fragment write
            // fails silently.
            mainHandler.post {
                if (state == TransportState.RUNNING || state == TransportState.STARTING) {
                    try {
                        stopUnsafe()
                    } catch (e: Exception) {
                        Log.e(TAG, "Error tearing down after GATT setup failure", e)
                    }
                }
            }
        }

        override fun onCentralConnected(device: BluetoothDevice) {
            mainHandler.post { handleCentralConnectedOnMain(device) }
        }

        override fun onCentralDisconnected(device: BluetoothDevice, status: Int) {
            mainHandler.post { handleCentralDisconnectedOnMain(device, status) }
        }

        override fun onInboundFragment(device: BluetoothDevice, bytes: ByteArray) {
            Log.i(TAG, "📥 MESSAGE CHARACTERISTIC WRITE from ${device.address}, processing...")
            emitDiagnostic(
                "info",
                "GATT write request received",
                mapOf(
                    "deviceAddress" to device.address,
                    "dataSize" to bytes.size,
                ),
            )
            val address = device.address
            mainHandler.post { handleReceivedData(bytes, address) }
        }

        override fun provideDeviceIdBytes(device: BluetoothDevice): ByteArray? {
            Log.d(TAG, "Sent device ID to ${device.address}")
            return deviceId.toByteArray(Charsets.UTF_8)
        }

        override fun provideIdentityBytes(device: BluetoothDevice): ByteArray? {
            // Pure volatile read. Never call updateSignedIdentity() here —
            // it would block this binder thread on the protocol mutex.
            // If the cache isn't primed yet, return null; the central will
            // retry. See the comment on [updateSignedIdentity].
            val identity = cachedSignedIdentity?.encode()
            if (identity == null) {
                // Trigger an out-of-band refresh on the main thread so the
                // next read has something to return.
                mainHandler.post { updateSignedIdentity() }
                return null
            }
            Log.d(TAG, "Sent signed identity to ${device.address}")
            return identity
        }
    }

    private fun handleCentralConnectedOnMain(device: BluetoothDevice) {
        val observation = lastSeenMeshAdvertisements[device.address]
        val decision = meshController.shouldAcceptInboundConnection(
            connections.deviceIdForAddress(device.address),
            observation?.advertisement,
            observation?.rssi
        )
        if (decision.evictPeerId != null) {
            evictPeer(decision.evictPeerId, "inbound_swap")
        }
        if (decision.intent == ConnectionIntent.REJECTED) {
            Log.w(TAG, "Rejecting inbound connection from ${device.address}: ${decision.reason}")
            peripheralGattServer?.cancelConnection(device)
            return
        }
        // Check connection capacity
        if (currentConnectionCount() >= MAX_CONNECTIONS_PER_DEVICE) {
            Log.w(TAG, "Rejecting inbound connection from ${device.address}: connection cap reached")
            peripheralGattServer?.cancelConnection(device)
            return
        }
        val role = when (decision.intent) {
            ConnectionIntent.INTER_CLUSTER -> MeshRole.BRIDGE
            ConnectionIntent.INTRA_CLUSTER, ConnectionIntent.REJECTED -> MeshRole.MEMBER
        }
        connections.trackServerConnection(device.address)
        connections.setPendingRole(device.address, role)
        Log.i(TAG, "GATT server: Device connected: ${device.address} (role=$role)")
        emitDiagnostic("info", "Device connected to GATT server", mapOf("address" to device.address))
    }

    private fun handleCentralDisconnectedOnMain(device: BluetoothDevice, status: Int) {
        val address = device.address
        connections.untrackServerConnection(address)
        connections.consumePendingRole(address)
        // Status 0 = clean local disconnect; status 19 (0x13) =
        // HCI_CONN_TERMINATE_PEER_USER, i.e. the remote end disconnected
        // cleanly. Both are normal lifecycle events where the peer is
        // likely to come back, so we keep the address→deviceId mapping and
        // RSSI cached to speed up reconnection. Any other status is a real
        // failure and we tear the peer state down.
        val isCleanDisconnect = status == 0 || status == 19
        if (!isCleanDisconnect) {
            lastSeenRssi.remove(address)
            connections.deviceIdForAddress(address)?.let { peerId ->
                protocol.removeNeighborRoutes(peerId)
                try {
                    protocol.blePeerLost(peerId)
                } catch (e: Exception) {
                    Log.e(TAG, "Error notifying peer lost", e)
                    emitDiagnostic("error", "Error notifying peer lost", mapOf("exception" to e.javaClass.simpleName, "message" to (e.message ?: "unknown")))
                }
                meshController.registerDisconnection(peerId)
                refreshSelfMetrics()
                connections.removeIdentifiersForAddress(address)
                deviceIdResolutionAttempts.remove(address)
                connections.removeConnectionRole(peerId)
                if (state == TransportState.RUNNING) {
                    refreshAdvertising("membership_change")
                }
                maybeHandleRebalance("disconnect")
            }
        }
    }

    // MARK: - GATT Client Callback
    
    private val gattClientCallback = object : BluetoothGattCallback() {
        override fun onConnectionStateChange(gatt: BluetoothGatt, status: Int, newState: Int) {
            when (newState) {
                BluetoothProfile.STATE_CONNECTED -> {
                    val address = gatt.device.address
                    Log.i(TAG, "GATT client: Connected to $address")
                    emitDiagnostic("info", "Connected to BLE device", mapOf("address" to address))
                    // Socket is up, so any prior backoff for this address is
                    // obsolete. Reset here (not on deviceId read) so a rebound
                    // link that races a subsequent disconnect still benefits.
                    connectionRetryCount.remove(address)
                    linkReady.remove(address)
                    try {
                        Log.d(TAG, "🔎 Starting service discovery for $address")
                        val discoveryStarted = gatt.discoverServices()
                        Log.d(TAG, "🔎 Service discovery ${if (discoveryStarted) "started" else "FAILED"} for $address")
                        emitDiagnostic("debug", "Service discovery initiated", mapOf(
                            "address" to address,
                            "started" to discoveryStarted
                        ))
                        if (!discoveryStarted) {
                            // discoverServices returns false when the stack is
                            // busy or the client is in a bad state. The async
                            // onServicesDiscovered callback will never fire, so
                            // the connection would sit here forever holding a
                            // slot. Tear it down and let the reconnect path
                            // retry cleanly.
                            emitDiagnostic("warning", "discoverServices returned false; closing gatt", mapOf(
                                "address" to address,
                            ))
                            try {
                                gatt.disconnect()
                                gatt.close()
                            } catch (e: Exception) {
                                Log.w(TAG, "Error closing gatt after discoverServices false", e)
                            }
                            connections.removeGatt(address)
                        }
                    } catch (e: SecurityException) {
                        Log.e(TAG, "❌ Permission denied discovering services", e)
                        emitDiagnostic("error", "Permission denied discovering services", mapOf("exception" to e.javaClass.simpleName, "message" to (e.message ?: "unknown")))
                    } catch (e: Exception) {
                        Log.e(TAG, "❌ Error discovering services", e)
                        emitDiagnostic("error", "Error discovering services", mapOf("exception" to e.javaClass.simpleName, "message" to (e.message ?: "unknown")))
                    }
                }
                BluetoothProfile.STATE_DISCONNECTED -> {
                    val address = gatt.device.address
                    val wasConnected = connections.getGatt(address) != null
                    connections.removeGatt(address)
                    linkReady.remove(address)
                    
                    // Don't remove from discovered list - keep trying to reconnect
                    // Only remove RSSI if it's a permanent error
                    if (status == 133) { // Connection timeout
                        lastSeenRssi.remove(address)
                    }
                    
                    // Try to reconnect if we were connected and state is still running
                    if (wasConnected && state == TransportState.RUNNING) {
                        // Increment retry count and calculate backoff
                        val retryCount = (connectionRetryCount[address] ?: 0) + 1
                        connectionRetryCount[address] = retryCount
                        
                        // Give up after max retries
                        if (retryCount > MAX_CONNECTION_RETRIES) {
                            Log.w(TAG, "Max retries ($MAX_CONNECTION_RETRIES) exceeded for $address on disconnect, giving up")
                            emitDiagnostic("warning", "Max connection retries exceeded", mapOf(
                                "address" to address,
                                "retryCount" to retryCount
                            ))
                            connectionRetryCount.remove(address)
                            // Notify peer lost since we're giving up
                            connections.deviceIdForAddress(address)?.let { peerId ->
                                protocol.removeNeighborRoutes(peerId)
                                try {
                                    protocol.blePeerLost(peerId)
                                } catch (e: Exception) {
                                    Log.e(TAG, "Error notifying peer lost", e)
                                }
                                meshController.registerDisconnection(peerId)
                                refreshSelfMetrics()
                                connections.removeIdentifiersForAddress(address)
                                connections.removeConnectionRole(peerId)
                                mainHandler.post {
                                    if (state == TransportState.RUNNING) {
                                        refreshAdvertising("disconnect_max_retries")
                                    }
                                }
                                maybeHandleRebalance("disconnect_max_retries")
                            }
                            return@onConnectionStateChange
                        }
                        
                        // Exponential backoff: 5s, 10s, 20s, 40s, 60s (capped)
                        val backoffInterval = minOf(MAX_RECONNECT_INTERVAL_MS, MIN_RECONNECT_INTERVAL_MS * (1L shl (retryCount - 1)))
                        
                        mainHandler.postDelayed({
                            if (state == TransportState.RUNNING && connections.hasDeviceForAddress(address)) {
                                try {
                                    val device = bluetoothAdapter?.getRemoteDevice(address)
                                    if (device != null) {
                                        connectToDevice(device)
                                    }
                                } catch (e: Exception) {
                                    Log.e(TAG, "Error reconnecting to device", e)
                                }
                            }
                        }, backoffInterval)
                    } else {
                        // Notify protocol of peer loss only if we're not reconnecting
                        connections.deviceIdForAddress(address)?.let { deviceId ->
                            // Clean up routes through this neighbor
                            protocol.removeNeighborRoutes(deviceId)
                            try {
                                protocol.blePeerLost(deviceId)
                            } catch (e: Exception) {
                                Log.e(TAG, "Error notifying peer lost", e)
                                emitDiagnostic("error", "Error notifying peer lost", mapOf("exception" to e.javaClass.simpleName, "message" to (e.message ?: "unknown")))
                            }
                            meshController.registerDisconnection(deviceId)
                            refreshSelfMetrics()
                            connections.removeIdentifiersForAddress(address)
                            deviceIdResolutionAttempts.remove(address)
                            connections.removeConnectionRole(deviceId)
                            mainHandler.post {
                                if (state == TransportState.RUNNING) {
                                    refreshAdvertising("membership_change")
                                }
                            }
                            maybeHandleRebalance("disconnect")
                        }
                    }
                }
            }
        }
        
        override fun onServicesDiscovered(gatt: BluetoothGatt, status: Int) {
            val address = gatt.device.address
            Log.i(TAG, "🔎 onServicesDiscovered callback: $address, status=$status (${if (status == BluetoothGatt.GATT_SUCCESS) "SUCCESS" else "FAILED"})")

            if (status != BluetoothGatt.GATT_SUCCESS) {
                Log.e(TAG, "❌ Service discovery FAILED for $address, status=$status")
                emitDiagnostic("error", "Service discovery failed", mapOf("address" to address, "status" to status))
                closeGattClient(gatt, "service_discovery_failed")
                return
            }

            val service = gatt.getService(SERVICE_UUID)
            if (service == null) {
                Log.w(TAG, "⚠️ Service UUID not found on $address. Available services: ${gatt.services.map { it.uuid }}")
                emitDiagnostic("warning", "Offline protocol service not found", mapOf(
                    "address" to address,
                    "serviceCount" to gatt.services.size
                ))
                verifiedNonMeshDevices[address] = System.currentTimeMillis()
                closeGattClient(gatt, "no_mesh_service")
                return
            }

            Log.i(TAG, "✅ Found offline protocol service on $address")
            emitDiagnostic("info", "GATT services discovered", mapOf("address" to address))

            // Kick off the first GATT operation only. Android's BLE stack
            // serializes one GATT op at a time, so we chain:
            //   onServicesDiscovered → readCharacteristic(deviceId)
            //   onCharacteristicRead(deviceId) → readCharacteristic(identity)
            //   onCharacteristicRead(identity) → setNotification + writeDescriptor(CCCD)
            //   onDescriptorWrite(CCCD success) → mark linkReady + drain
            // Firing more than one op here causes the second to be silently
            // dropped on some vendors, which is the class of bug where the
            // connection "looks healthy" but no messages flow.
            val deviceIdChar = service.getCharacteristic(DEVICE_ID_CHAR_UUID)
            if (deviceIdChar == null) {
                Log.w(TAG, "⚠️ Device ID characteristic NOT FOUND on $address")
                emitDiagnostic("warning", "Device ID characteristic missing", mapOf("address" to address))
                closeGattClient(gatt, "device_id_char_missing")
                return
            }

            Log.d(TAG, "📖 Reading device ID characteristic from $address")
            try {
                val readStarted = gatt.readCharacteristic(deviceIdChar)
                if (!readStarted) {
                    Log.w(TAG, "📖 readCharacteristic(deviceId) returned false for $address")
                    emitDiagnostic("warning", "readCharacteristic(deviceId) returned false", mapOf("address" to address))
                    closeGattClient(gatt, "device_id_read_rejected")
                }
            } catch (e: SecurityException) {
                Log.e(TAG, "❌ Permission denied reading characteristic", e)
                emitDiagnostic("error", "Permission denied reading device ID characteristic", mapOf("exception" to e.javaClass.simpleName, "message" to (e.message ?: "unknown")))
                closeGattClient(gatt, "device_id_read_permission_denied")
            }
        }
        
        override fun onCharacteristicRead(gatt: BluetoothGatt, characteristic: BluetoothGattCharacteristic, status: Int) {
            val address = gatt.device.address
            Log.i(TAG, "📖 onCharacteristicRead: $address, char=${characteristic.uuid}, status=$status")

            if (status != BluetoothGatt.GATT_SUCCESS) {
                Log.w(TAG, "⚠️ Characteristic read failed for $address, char=${characteristic.uuid}, status=$status")
                emitDiagnostic("warning", "Characteristic read failed", mapOf(
                    "address" to address,
                    "char" to characteristic.uuid.toString(),
                    "status" to status,
                ))
                // Only the device-ID read is load-bearing for link setup.
                // An identity read failure is diagnostic and should not tear
                // the link down — the peer is still usable, just unverified.
                if (characteristic.uuid == DEVICE_ID_CHAR_UUID) {
                    closeGattClient(gatt, "device_id_read_failed")
                }
                return
            }

            when (characteristic.uuid) {
                DEVICE_ID_CHAR_UUID -> handleDeviceIdRead(gatt, characteristic)
                IDENTITY_CHAR_UUID -> handleIdentityRead(gatt, characteristic)
            }
        }

        private fun handleDeviceIdRead(gatt: BluetoothGatt, characteristic: BluetoothGattCharacteristic) {
            val address = gatt.device.address
            @Suppress("DEPRECATION")
            val deviceIdValue = characteristic.value?.toString(Charsets.UTF_8)
            Log.i(TAG, "✅ Read device ID from $address: $deviceIdValue")

            if (deviceIdValue.isNullOrEmpty()) {
                Log.w(TAG, "⚠️ Empty or null device ID from $address")
                emitDiagnostic("warning", "Empty device ID characteristic value", mapOf("address" to address))
                closeGattClient(gatt, "empty_device_id")
                return
            }

            Log.i(TAG, "📝 Mapping $address -> $deviceIdValue")
            connections.setDeviceIdentifier(address, deviceIdValue)

            val role = connections.consumePendingRole(address) ?: MeshRole.MEMBER
            meshController.registerConnection(deviceIdValue, role)
            connections.setConnectionRole(deviceIdValue, role)
            meshController.markPeerActive(deviceIdValue)
            meshController.markPeerActive(deviceId)
            refreshSelfMetrics()
            mainHandler.post {
                if (state == TransportState.RUNNING) {
                    refreshAdvertising("membership_change")
                }
            }
            val rssiInt = lastSeenRssi[address]?.toInt()
            if (rssiInt != null) {
                meshController.updatePeerMetrics(
                    deviceIdValue,
                    MeshController.PeerMetrics(rssi = rssiInt)
                )
            }

            // Notify protocol of peer discovery
            val rssi = lastSeenRssi[address] ?: (-60).toShort()
            try {
                protocol.blePeerDiscovered(deviceIdValue, rssi)
            } catch (e: Exception) {
                Log.e(TAG, "Error notifying peer discovered", e)
                emitDiagnostic("error", "Error notifying peer discovered", mapOf("exception" to e.javaClass.simpleName, "message" to (e.message ?: "unknown")))
            }

            // Drain all pending inbound fragments keyed by this address —
            // the same buffer holds central-side notify fragments and
            // server-side write fragments, since both paths queue under the
            // connection-specific address.
            processPendingFragments(address, deviceIdValue)

            // Chain the next GATT op: read the identity characteristic.
            // Only issue it now, after the deviceId read callback has fired,
            // so the stack is idle and won't drop the request.
            val service = gatt.getService(SERVICE_UUID)
            val identityChar = service?.getCharacteristic(IDENTITY_CHAR_UUID)
            if (identityChar == null) {
                Log.w(TAG, "⚠️ Identity characteristic not found on $address, skipping to CCCD")
                enableNotificationsOnLink(gatt)
                return
            }

            try {
                val started = gatt.readCharacteristic(identityChar)
                if (!started) {
                    Log.w(TAG, "⚠️ readCharacteristic(identity) returned false for $address; proceeding to CCCD")
                    emitDiagnostic("warning", "readCharacteristic(identity) returned false", mapOf("address" to address))
                    // Identity is non-blocking — skip to CCCD instead of tearing the link down.
                    enableNotificationsOnLink(gatt)
                }
            } catch (e: SecurityException) {
                Log.e(TAG, "❌ Permission denied reading identity characteristic", e)
                emitDiagnostic("error", "Permission denied reading identity characteristic", mapOf("exception" to e.javaClass.simpleName, "message" to (e.message ?: "unknown")))
                enableNotificationsOnLink(gatt)
            }
        }

        private fun handleIdentityRead(gatt: BluetoothGatt, characteristic: BluetoothGattCharacteristic) {
            @Suppress("DEPRECATION")
            handleReceivedIdentity(characteristic.value, gatt.device.address)
            // Identity read complete → now enable notifications.
            enableNotificationsOnLink(gatt)
        }

        private fun enableNotificationsOnLink(gatt: BluetoothGatt) {
            val address = gatt.device.address
            val service = gatt.getService(SERVICE_UUID)
            val messageChar = service?.getCharacteristic(MESSAGE_CHAR_UUID)
            if (messageChar == null) {
                Log.w(TAG, "⚠️ Message characteristic NOT FOUND on $address")
                emitDiagnostic("warning", "Message characteristic missing", mapOf("address" to address))
                closeGattClient(gatt, "message_char_missing")
                return
            }
            Log.d(TAG, "🔔 Enabling notifications for message characteristic on $address")
            try {
                val notifyEnabled = gatt.setCharacteristicNotification(messageChar, true)
                if (!notifyEnabled) {
                    Log.w(TAG, "🔔 Local notification sink FAILED for $address")
                    emitDiagnostic("warning", "setCharacteristicNotification returned false", mapOf("address" to address))
                    closeGattClient(gatt, "set_notification_failed")
                    return
                }
                val cccd = messageChar.getDescriptor(PeripheralGattServer.CCCD_UUID)
                if (cccd == null) {
                    Log.w(TAG, "⚠️ CCCD descriptor missing on remote message characteristic for $address")
                    emitDiagnostic("warning", "Remote CCCD descriptor missing", mapOf("address" to address))
                    closeGattClient(gatt, "remote_cccd_missing")
                    return
                }
                val cccdWriteOk = if (Build.VERSION.SDK_INT >= Build.VERSION_CODES.TIRAMISU) {
                    val cccdWriteStatus = gatt.writeDescriptor(
                        cccd,
                        BluetoothGattDescriptor.ENABLE_NOTIFICATION_VALUE,
                    )
                    Log.d(TAG, "🔔 CCCD write status=$cccdWriteStatus for $address")
                    cccdWriteStatus == BluetoothStatusCodes.SUCCESS
                } else {
                    @Suppress("DEPRECATION")
                    cccd.value = BluetoothGattDescriptor.ENABLE_NOTIFICATION_VALUE
                    @Suppress("DEPRECATION")
                    val queued = gatt.writeDescriptor(cccd)
                    Log.d(TAG, "🔔 CCCD write ${if (queued) "queued" else "FAILED"} for $address")
                    queued
                }
                if (!cccdWriteOk) {
                    emitDiagnostic("warning", "CCCD writeDescriptor failed to initiate", mapOf("address" to address))
                    closeGattClient(gatt, "cccd_write_rejected")
                }
                // Success path waits for onDescriptorWrite to fire.
            } catch (e: SecurityException) {
                Log.e(TAG, "❌ Permission denied setting notification", e)
                emitDiagnostic("error", "Permission denied enabling notifications", mapOf("exception" to e.javaClass.simpleName, "message" to (e.message ?: "unknown")))
                closeGattClient(gatt, "set_notification_permission_denied")
            }
        }

        override fun onDescriptorWrite(gatt: BluetoothGatt, descriptor: BluetoothGattDescriptor, status: Int) {
            val address = gatt.device.address
            if (descriptor.uuid != PeripheralGattServer.CCCD_UUID) {
                return
            }
            if (status != BluetoothGatt.GATT_SUCCESS) {
                Log.w(TAG, "⚠️ CCCD write failed for $address, status=$status; tearing link down")
                emitDiagnostic("warning", "CCCD write failed", mapOf(
                    "address" to address,
                    "status" to status,
                ))
                closeGattClient(gatt, "cccd_write_failed")
                return
            }
            Log.i(TAG, "✅ CCCD write acknowledged for $address — link ready")
            emitDiagnostic("info", "BLE link ready", mapOf("address" to address))
            linkReady.add(address)
            // The link is now bidirectional: we can receive notifications and
            // any outbound fragments that were queued while waiting for this
            // handshake should drain on the next poll / onFragmentsAvailable.
            runOnMain {
                if (state == TransportState.RUNNING) {
                    drainAndSendFragments()
                }
            }
        }

        private fun closeGattClient(gatt: BluetoothGatt, reason: String) {
            val address = gatt.device.address
            try {
                gatt.disconnect()
                gatt.close()
            } catch (e: Exception) {
                Log.w(TAG, "Error closing gatt for $address ($reason)", e)
            }
            connections.removeGatt(address)
            linkReady.remove(address)
        }
        
        /**
         * Handles received identity data from a peer.
         * Verifies the signature and stores the verified identity.
         */
        private fun handleReceivedIdentity(data: ByteArray?, address: String) {
            val signedIdentity = com.offlineprotocol.mesh.SignedIdentityData.decode(data)
            if (signedIdentity == null) {
                Log.w(TAG, "Failed to decode identity data from $address")
                emitDiagnostic("warning", "Failed to decode peer identity", mapOf("address" to address))
                return
            }
            
            try {
                // Convert ByteArray to List<UByte> for UniFFI bindings
                val isValid = protocol.verifySignature(
                    signedIdentity.publicKey.map { it.toUByte() },
                    signedIdentity.advertisementData.map { it.toUByte() },
                    signedIdentity.signature.map { it.toUByte() }
                )
                
                if (isValid) {
                    // Store the verified identity
                    verifiedPeerIdentities[address] = signedIdentity
                    
                    // Derive the user ID from the public key
                    val derivedUserId = signedIdentity.deriveUserId()
                    Log.i(TAG, "✅ Verified peer identity: $derivedUserId for $address")
                    emitDiagnostic("info", "Verified peer identity", mapOf(
                        "address" to address,
                        "derivedUserId" to derivedUserId
                    ))
                    
                    // Update routing with the cryptographically derived user ID
                    val rssi = lastSeenRssi[address] ?: (-60).toShort()
                    val quality = minOf(1.0f, maxOf(0.0f, (rssi.toFloat() + 100f) / 80f))
                    protocol.learnRoute(derivedUserId, derivedUserId, 1.toUByte(), quality, 0u)
                } else {
                    Log.w(TAG, "⚠️ Invalid signature for peer $address")
                    emitDiagnostic("warning", "Invalid peer signature", mapOf("address" to address))
                }
            } catch (e: Exception) {
                Log.e(TAG, "Failed to verify signature: ${e.message}", e)
                emitDiagnostic("error", "Signature verification failed", mapOf("error" to (e.message ?: "unknown")))
            }
        }
        
        override fun onCharacteristicChanged(gatt: BluetoothGatt, characteristic: BluetoothGattCharacteristic) {
            if (characteristic.uuid == MESSAGE_CHAR_UUID) {
                @Suppress("DEPRECATION")
                val data = characteristic.value
                if (data != null) {
                    handleReceivedData(data, gatt.device.address)
                }
            }
        }
    }
}

