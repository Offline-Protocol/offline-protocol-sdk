package com.offlineprotocol

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
import com.offlineprotocol.ble.MeshConnectionRegistry
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
 * BLE Manager implementing TransportManager for Bluetooth Low Energy communication
 * Ensures iOS ↔ Android cross-platform compatibility
 */
class BleManager(
    private val context: Context,
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
        private const val TAG = "BleManager"
        
        // UUIDs must match iOS and Rust core exactly
        private val SERVICE_UUID = UUID.fromString("6E400001-B5A3-F393-E0A9-E50E24DCCA9E")
        private val MESSAGE_CHAR_UUID = UUID.fromString("6E400002-B5A3-F393-E0A9-E50E24DCCA9E")
        private val DEVICE_ID_CHAR_UUID = UUID.fromString("6E400003-B5A3-F393-E0A9-E50E24DCCA9E")
        
        private const val FRAGMENT_POLL_INTERVAL_MS = 100L // 100ms
        private const val MAX_FRAGMENT_SIZE = 185
        private const val CONNECTION_TIMEOUT_MS = 10000L
        private const val SCAN_WATCHDOG_INTERVAL_MS = 20000L
        private const val SCAN_WATCHDOG_HEARTBEAT_MS = 10000L
        private const val MAX_CONNECTIONS_PER_DEVICE = 4
        private const val ADVERTISE_RESTART_MIN_MS = 200L
        private const val ADVERTISE_RESTART_MAX_MS = 1200L
        private const val MIN_ADVERTISE_INTERVAL_MS = 1500L
    }
    
    // MARK: - Properties
    
    private val bluetoothManager: BluetoothManager = 
        context.getSystemService(Context.BLUETOOTH_SERVICE) as BluetoothManager
    private val bluetoothAdapter: BluetoothAdapter? = bluetoothManager.adapter
    
    // Scanner components
    private var bluetoothLeScanner: BluetoothLeScanner? = null
    private var scanCallback: ScanCallback? = null
    private var isScanning = false
    
    // Advertiser components
    private var bluetoothLeAdvertiser: BluetoothLeAdvertiser? = null
    private var advertiseCallback: AdvertiseCallback? = null
    private var isAdvertising = false
    
    // GATT Server (peripheral role)
    private var gattServer: BluetoothGattServer? = null
    private var messageCharacteristic: BluetoothGattCharacteristic? = null
    private var deviceIdCharacteristic: BluetoothGattCharacteristic? = null
    
    // Connection registry keeps track of client/server links and desired roles.
    private val connections = MeshConnectionRegistry()
    private val lastSeenRssi = ConcurrentHashMap<String, Short>()
    private val discoveryLogTimestamps = ConcurrentHashMap<String, Long>()
    @Volatile private var lastDiscoveryAt: Long = 0L

    private val logThrottler = LogThrottler()
    
    // Pending fragments waiting for device ID
    private data class PendingFragment(val data: ByteArray, val timestamp: Long)
    private data class MeshObservation(val advertisement: MeshAdvertisementData, val rssi: Int?, val timestamp: Long)
    private val pendingFragments = ConcurrentHashMap<String, MutableList<PendingFragment>>()
    private val PENDING_FRAGMENT_TIMEOUT_MS = 5000L
    private val LOAD_SATURATION_COUNT = 20
    private val MESH_OBSERVATION_TTL_MS = 120_000L
    private val deviceIdResolutionAttempts = ConcurrentHashMap<String, Long>()

    private val meshController = MeshController(deviceId)
    @Volatile private var lastMeshAdvertisement: MeshAdvertisementData? = null
    
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
    private val fragmentPollingRunnable = object : Runnable {
        override fun run() {
            pollAndSendFragments()
            if (state == TransportState.RUNNING) {
                mainHandler.postDelayed(this, FRAGMENT_POLL_INTERVAL_MS)
            }
        }
    }
    private val pendingOutboundFragments = mutableMapOf<String, MutableList<ByteArray>>()
    private val lastSeenMeshAdvertisements = ConcurrentHashMap<String, MeshObservation>()
    private var pendingAdvertiseRestart: Runnable? = null
    private var lastAdvertiseRestartAt: Long = 0L
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
                restartScanning("watchdog")
                return
            }
            mainHandler.postDelayed(this, SCAN_WATCHDOG_HEARTBEAT_MS)
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
        
        if (!checkPermissions()) {
            Log.w(TAG, "Bluetooth permissions not granted")
            throw TransportException.PermissionDenied("Bluetooth permissions not granted")
        }
        
        if (bluetoothAdapter?.isEnabled != true) {
            Log.w(TAG, "Bluetooth is not enabled")
            throw TransportException.InvalidState("Bluetooth is not enabled")
        }
        
        Log.i(TAG, "Starting BLE transport for device: $deviceId")
        emitDiagnostic("info", "Starting BLE transport", mapOf("deviceId" to deviceId))
        updateState(TransportState.STARTING)
        
        try {
            // Initialize scanner
            bluetoothLeScanner = bluetoothAdapter.bluetoothLeScanner
            
            // Initialize advertiser
            bluetoothLeAdvertiser = bluetoothAdapter.bluetoothLeAdvertiser
            
            // Setup GATT server
            setupGattServer()

            transportStartAt = System.currentTimeMillis()
            meshController.markPeerActive(deviceId)
            refreshSelfMetrics()
            
            // Start advertising
            startAdvertising("start")
            
            // Start scanning
            startScanning("start")
            
            // Start fragment polling
            mainHandler.post(fragmentPollingRunnable)
            
            updateState(TransportState.RUNNING)
            Log.i(TAG, "BLE Manager started successfully - calling bleStatusChanged(true)")
            Log.i(TAG, "About to call protocol.bleStatusChanged(true)")
            emitDiagnostic("info", "About to call protocol.bleStatusChanged(true)")
            
            try {
                protocol.bleStatusChanged(true)
                Log.i(TAG, "Successfully called protocol.bleStatusChanged(true)")
                emitDiagnostic("info", "Successfully called protocol.bleStatusChanged(true)")
            } catch (e: Exception) {
                Log.e(TAG, "Failed to call protocol.bleStatusChanged(true): ${e.message}", e)
                emitDiagnostic("error", "Failed to call protocol.bleStatusChanged(true)", mapOf(
                    "error" to (e.message ?: "unknown"),
                    "exception" to e.javaClass.simpleName
                ))
            }
            
            Log.i(TAG, "BLE Manager started successfully - scanning and advertising active")
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

    private fun stopUnsafe() {
        if (state != TransportState.RUNNING && state != TransportState.STARTING) {
            return
        }
        
        updateState(TransportState.STOPPING)
        
        // Stop fragment polling
        mainHandler.removeCallbacks(fragmentPollingRunnable)
        
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
        pendingFragments.clear()
        pendingOutboundFragments.clear()
        lastSeenMeshAdvertisements.clear()
        transportStartAt = 0L
        
        // Close GATT server
        gattServer?.close()
        gattServer = null
        
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
        return if (Build.VERSION.SDK_INT >= Build.VERSION_CODES.S) {
            // Android 12+ requires new permissions
            ContextCompat.checkSelfPermission(context, Manifest.permission.BLUETOOTH_SCAN) == PackageManager.PERMISSION_GRANTED &&
            ContextCompat.checkSelfPermission(context, Manifest.permission.BLUETOOTH_ADVERTISE) == PackageManager.PERMISSION_GRANTED &&
            ContextCompat.checkSelfPermission(context, Manifest.permission.BLUETOOTH_CONNECT) == PackageManager.PERMISSION_GRANTED
        } else {
            // Pre-Android 12
            ContextCompat.checkSelfPermission(context, Manifest.permission.BLUETOOTH) == PackageManager.PERMISSION_GRANTED &&
            ContextCompat.checkSelfPermission(context, Manifest.permission.BLUETOOTH_ADMIN) == PackageManager.PERMISSION_GRANTED &&
            ContextCompat.checkSelfPermission(context, Manifest.permission.ACCESS_FINE_LOCATION) == PackageManager.PERMISSION_GRANTED
        }
    }
    
    private fun setupGattServer() {
        try {
            gattServer = bluetoothManager.openGattServer(context, gattServerCallback)
            
            // Create message characteristic (write without response + notify)
            messageCharacteristic = BluetoothGattCharacteristic(
                MESSAGE_CHAR_UUID,
                BluetoothGattCharacteristic.PROPERTY_WRITE_NO_RESPONSE or BluetoothGattCharacteristic.PROPERTY_NOTIFY,
                BluetoothGattCharacteristic.PERMISSION_WRITE
            )
            
            // Create device ID characteristic (read)
            deviceIdCharacteristic = BluetoothGattCharacteristic(
                DEVICE_ID_CHAR_UUID,
                BluetoothGattCharacteristic.PROPERTY_READ,
                BluetoothGattCharacteristic.PERMISSION_READ
            )
            deviceIdCharacteristic?.value = deviceId.toByteArray(Charsets.UTF_8)
            
            // Create service
            val service = BluetoothGattService(SERVICE_UUID, BluetoothGattService.SERVICE_TYPE_PRIMARY)
            service.addCharacteristic(messageCharacteristic)
            service.addCharacteristic(deviceIdCharacteristic)
            
            // Add service to GATT server
            gattServer?.addService(service)
            
            Log.i(TAG, "GATT server configured")
        } catch (e: SecurityException) {
            Log.e(TAG, "Permission denied while setting up GATT server", e)
            throw e
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
            
            val scanFilter = ScanFilter.Builder()
                .setServiceUuid(ParcelUuid(SERVICE_UUID))
                .build()
            
            scanCallback = object : ScanCallback() {
                override fun onScanResult(callbackType: Int, result: ScanResult) {
                    handleScanResult(result)
                }
                
                override fun onBatchScanResults(results: List<ScanResult>) {
                    results.forEach { handleScanResult(it) }
                }
                
                override fun onScanFailed(errorCode: Int) {
                    Log.e(TAG, "Scan failed with error code: $errorCode")
                    isScanning = false
                    emitDiagnostic("error", "BLE scan failed", mapOf("errorCode" to errorCode))
                }
            }
            
            scanner.startScan(listOf(scanFilter), scanSettings, scanCallback)
            isScanning = true
            lastDiscoveryAt = System.currentTimeMillis()
            scheduleScanWatchdog()
            if (logThrottler.shouldLog("scan_started")) {
                Log.i(TAG, "Started BLE scanning (reason: $reason)")
                emitDiagnostic("info", "Started BLE scanning", mapOf("reason" to reason))
            }
        } catch (e: SecurityException) {
            Log.e(TAG, "Permission denied while starting scan", e)
            emitDiagnostic("error", "Permission denied while starting scan", mapOf("exception" to e.javaClass.simpleName, "message" to (e.message ?: "unknown")))
            throw e
        }
    }
    
    private fun stopScanning(reason: String = "manual") {
        if (!isScanning) return
        
        try {
            scanCallback?.let { bluetoothLeScanner?.stopScan(it) }
            scanCallback = null
            isScanning = false
            cancelScanWatchdog()
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
    
    private fun startAdvertising(reason: String = "manual") {
        if (isAdvertising) return
        
        try {
            val settings = AdvertiseSettings.Builder()
                .setAdvertiseMode(AdvertiseSettings.ADVERTISE_MODE_LOW_LATENCY)
                .setConnectable(true)
                .setTimeout(0)
                .setTxPowerLevel(AdvertiseSettings.ADVERTISE_TX_POWER_HIGH)
                .build()

            val advertiseData = buildAdvertiseData()
            
            advertiseCallback = object : AdvertiseCallback() {
                override fun onStartSuccess(settingsInEffect: AdvertiseSettings) {
                    Log.i(TAG, "Advertising started successfully (reason=$reason)")
                    isAdvertising = true
                    lastAdvertiseRestartAt = System.currentTimeMillis()
                    emitDiagnostic("info", "BLE advertising started")
                }
                
                override fun onStartFailure(errorCode: Int) {
                    Log.e(TAG, "Advertising failed with error code: $errorCode")
                    isAdvertising = false
                    emitDiagnostic("error", "BLE advertising failed", mapOf("errorCode" to errorCode))
                }
            }
            
            bluetoothLeAdvertiser?.startAdvertising(settings, advertiseData, advertiseCallback)
            
            // Reduced logging
        } catch (e: SecurityException) {
            Log.e(TAG, "Permission denied while starting advertising", e)
            emitDiagnostic("error", "Permission denied while starting advertising", mapOf("exception" to e.javaClass.simpleName, "message" to (e.message ?: "unknown")))
            throw e
        }
    }
    
    private fun stopAdvertising() {
        if (!isAdvertising) return
        
        try {
            advertiseCallback?.let { bluetoothLeAdvertiser?.stopAdvertising(it) }
            advertiseCallback = null
            isAdvertising = false
            pendingAdvertiseRestart?.let {
                mainHandler.removeCallbacks(it)
                pendingAdvertiseRestart = null
            }
            Log.i(TAG, "Stopped advertising")
            emitDiagnostic("info", "Stopped BLE advertising")
        } catch (e: SecurityException) {
            Log.e(TAG, "Permission denied while stopping advertising", e)
            emitDiagnostic("error", "Permission denied while stopping advertising", mapOf("exception" to e.javaClass.simpleName, "message" to (e.message ?: "unknown")))
        }
    }

    private fun refreshAdvertising(reason: String) {
        stopAdvertising()
        scheduleAdvertisingRestart(reason)
    }

    private fun scheduleAdvertisingRestart(reason: String) {
        val now = System.currentTimeMillis()
        val elapsed = now - lastAdvertiseRestartAt
        val cooldownDelay = if (elapsed < MIN_ADVERTISE_INTERVAL_MS) MIN_ADVERTISE_INTERVAL_MS - elapsed else 0L
        val jitter = ThreadLocalRandom.current().nextLong(ADVERTISE_RESTART_MIN_MS, ADVERTISE_RESTART_MAX_MS + 1)
        val delay = cooldownDelay + jitter
        pendingAdvertiseRestart?.let { mainHandler.removeCallbacks(it) }
        val runnable = Runnable {
            pendingAdvertiseRestart = null
            startAdvertising(reason)
        }
        pendingAdvertiseRestart = runnable
        mainHandler.postDelayed(runnable, delay)
    }

    private fun buildAdvertiseData(): AdvertiseData {
        val meshData = meshController.toAdvertisement()
        lastMeshAdvertisement = meshData
        val encoded = meshData.encode()
        return AdvertiseData.Builder()
            .setIncludeDeviceName(false)
            .addServiceUuid(ParcelUuid(SERVICE_UUID))
            .addServiceData(ParcelUuid(SERVICE_UUID), encoded)
            .build()
    }
    
    private fun handleScanResult(result: ScanResult) {
        val device = result.device
        val rssi = result.rssi
        val address = device.address
        val now = System.currentTimeMillis()
        lastDiscoveryAt = now
        val lastLog = discoveryLogTimestamps[address]
        val isConnectable = if (Build.VERSION.SDK_INT >= Build.VERSION_CODES.O) {
            result.isConnectable
        } else {
            null
        }
        if (lastLog == null || now - lastLog > 30000) {
            discoveryLogTimestamps[address] = now
            Log.d(TAG, "Discovered device $address RSSI=$rssi")
            emitDiagnostic(
                "info",
                "Discovered BLE device",
                mapOf(
                    "address" to address,
                    "rssi" to rssi,
                    "connectable" to (isConnectable ?: true)
                )
            )
        }
        lastSeenRssi[address] = rssi.toShort()

        val scanRecord = result.scanRecord
        val serviceData = scanRecord?.getServiceData(ParcelUuid(SERVICE_UUID))
        val meshMetadata = MeshAdvertisementData.decode(serviceData)
        meshMetadata?.let {
            lastSeenMeshAdvertisements[address] = MeshObservation(it, rssi, now)
        }
        meshController.observeAdvertisement(meshMetadata, rssi)
        pruneMeshObservations(now)

        val decision = meshController.shouldInitiateOutbound(meshMetadata, rssi)
        if (decision.intent == ConnectionIntent.REJECTED) {
            if (logThrottler.shouldLog("mesh_skip_$address", intervalMs = 15000)) {
                Log.v(TAG, "Skipping connection to $address due to ${decision.reason}")
            }
            return
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
    }
    
    private fun connectToDevice(device: BluetoothDevice) {
        try {
            if (currentConnectionCount() >= MAX_CONNECTIONS_PER_DEVICE) {
                if (logThrottler.shouldLog("mesh_conn_cap", intervalMs = 10000)) {
                    Log.d(TAG, "Connection cap reached, not connecting to ${device.address}")
                }
                connections.consumePendingRole(device.address)
                return
            }
            val gatt = device.connectGatt(context, false, gattClientCallback, BluetoothDevice.TRANSPORT_LE)
            connections.registerGatt(device.address, gatt)
            
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
        val pendingCount = pendingFragments.values.sumOf { it.size }
        val outboundPending = pendingOutboundFragments.values.sumOf { it.size }
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
        connections.removeIdentifiersForDevice(peerId)
        connections.removeConnectionRole(peerId)
        lastSeenRssi.remove(address)
        pendingFragments.remove(address)
        pendingOutboundFragments.remove(peerId)
        deviceIdResolutionAttempts.remove(address)
        meshController.registerDisconnection(peerId)
        refreshSelfMetrics()

        try {
            protocol.blePeerLost(peerId)
        } catch (e: Exception) {
            Log.e(TAG, "Failed to notify protocol of peer eviction", e)
        }

        refreshAdvertising("evict_$reason")
        maybeHandleRebalance("evict")
    }
    
    private fun pollAndSendFragments() {
        try {
            if (flushPendingOutboundFragments()) {
                return
            }

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
                // Only log occasionally to avoid spam
                if (logThrottler.shouldLog("no_fragments", intervalMs = 10000)) {
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

        val recipients = pendingOutboundFragments.keys.toList()
        for (recipientId in recipients) {
            val queue = pendingOutboundFragments[recipientId] ?: continue
            val iterator = queue.listIterator()
            while (iterator.hasNext()) {
                val data = iterator.next()
                if (sendFragmentData(recipientId, data)) {
                    iterator.remove()
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
        val queue = pendingOutboundFragments.getOrPut(recipientId) { mutableListOf() }
        queue.add(data)
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
        
        if (gatt == null) {
            if (logThrottler.shouldLog("missing_gatt_$recipientId")) {
                Log.w(TAG, "No connected device for recipient: $recipientId")
                emitDiagnostic("warning", "No connected device for BLE fragment", mapOf("recipientId" to recipientId))
            }
            return false
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

        characteristic.value = data
        characteristic.writeType = BluetoothGattCharacteristic.WRITE_TYPE_NO_RESPONSE

        val writeOk = try {
            if (Build.VERSION.SDK_INT >= Build.VERSION_CODES.TIRAMISU) {
                gatt.writeCharacteristic(characteristic, data, BluetoothGattCharacteristic.WRITE_TYPE_NO_RESPONSE) == BluetoothStatusCodes.SUCCESS
            } else {
                gatt.writeCharacteristic(characteristic)
            }
        } catch (e: Exception) {
            Log.e(TAG, "Error writing characteristic", e)
            emitDiagnostic("error", "Error writing BLE fragment", mapOf("recipientId" to recipientId, "exception" to e.javaClass.simpleName, "message" to (e.message ?: "unknown")))
            false
        }

        if (!writeOk) {
            if (logThrottler.shouldLog("write_failed_$recipientId", intervalMs = 2000)) {
                Log.w(TAG, "Failed to write BLE fragment for $recipientId")
                emitDiagnostic("warning", "Failed to write BLE fragment", mapOf("recipientId" to recipientId))
            }
            return false
        }
        
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
                val pendingList = pendingFragments.getOrPut(address) { mutableListOf() }
                pendingList.add(PendingFragment(data, System.currentTimeMillis()))
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
                
                // CRITICAL: Immediately check if this completed a message
                val completedMessage = protocol.receiveMessage()
                if (completedMessage != null) {
                    Log.i(TAG, "🎉 COMPLETE MESSAGE ASSEMBLED FROM FRAGMENTS!")
                    Log.i(TAG, "📬 Received message: $completedMessage")
                    emitDiagnostic("info", "Complete message assembled from fragments", mapOf(
                        "senderId" to senderId,
                        "messageContent" to completedMessage
                    ))
                } else {
                    Log.d(TAG, "📦 Fragment processed, waiting for more fragments to complete message")
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
        val fragments = pendingFragments.remove(address) ?: return
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
            } catch (e: Exception) {
                Log.e(TAG, "Error processing pending fragment", e)
            }
        }
    }
    
    private fun cleanupPendingFragments() {
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

    private fun pruneMeshObservations(now: Long) {
        val iterator = lastSeenMeshAdvertisements.entries.iterator()
        while (iterator.hasNext()) {
            val entry = iterator.next()
            if (now - entry.value.timestamp > MESH_OBSERVATION_TTL_MS) {
                iterator.remove()
            }
        }
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
    
    // MARK: - GATT Server Callback
    
    private val gattServerCallback = object : BluetoothGattServerCallback() {
        override fun onConnectionStateChange(device: BluetoothDevice, status: Int, newState: Int) {
            when (newState) {
                BluetoothProfile.STATE_CONNECTED -> {
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
                        gattServer?.cancelConnection(device)
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
                BluetoothProfile.STATE_DISCONNECTED -> {
                    val address = device.address
                    connections.untrackServerConnection(address)
                    connections.consumePendingRole(address)
                    // Don't immediately remove - connection might be re-established
                    // Only remove if it's a permanent error (status != 0)
                    if (status != 0 && status != 19) { // Not normal disconnect or connection timeout
                        lastSeenRssi.remove(address)
                        connections.deviceIdForAddress(address)?.let { peerId ->
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
        
        override fun onCharacteristicReadRequest(
            device: BluetoothDevice,
            requestId: Int,
            offset: Int,
            characteristic: BluetoothGattCharacteristic
        ) {
            try {
                if (characteristic.uuid == DEVICE_ID_CHAR_UUID) {
                    gattServer?.sendResponse(device, requestId, BluetoothGatt.GATT_SUCCESS, offset, characteristic.value)
                    Log.d(TAG, "Sent device ID to ${device.address}")
                } else {
                    gattServer?.sendResponse(device, requestId, BluetoothGatt.GATT_FAILURE, offset, null)
                }
            } catch (e: SecurityException) {
                Log.e(TAG, "Permission denied in read request", e)
                emitDiagnostic("error", "Permission denied in characteristic read", mapOf("exception" to e.javaClass.simpleName, "message" to (e.message ?: "unknown")))
            }
        }
        
        override fun onCharacteristicWriteRequest(
            device: BluetoothDevice,
            requestId: Int,
            characteristic: BluetoothGattCharacteristic,
            preparedWrite: Boolean,
            responseNeeded: Boolean,
            offset: Int,
            value: ByteArray
        ) {
            try {
                Log.i(TAG, "📨 GATT WRITE REQUEST from ${device.address}, char: ${characteristic.uuid}, size: ${value.size}")
                emitDiagnostic("info", "GATT write request received", mapOf(
                    "deviceAddress" to device.address,
                    "characteristicUuid" to characteristic.uuid.toString(),
                    "dataSize" to value.size,
                    "responseNeeded" to responseNeeded
                ))
                
                if (characteristic.uuid == MESSAGE_CHAR_UUID) {
                    Log.i(TAG, "📥 MESSAGE CHARACTERISTIC WRITE from ${device.address}, processing...")
                    // Handle incoming fragment
                    handleReceivedData(value, device.address)
                    
                    if (responseNeeded) {
                        gattServer?.sendResponse(device, requestId, BluetoothGatt.GATT_SUCCESS, offset, value)
                        Log.d(TAG, "✅ Sent GATT_SUCCESS response to ${device.address}")
                    }
                } else {
                    Log.w(TAG, "❌ Unknown characteristic write: ${characteristic.uuid}")
                    if (responseNeeded) {
                        gattServer?.sendResponse(device, requestId, BluetoothGatt.GATT_FAILURE, offset, null)
                    }
                }
            } catch (e: SecurityException) {
                Log.e(TAG, "Permission denied in write request", e)
                emitDiagnostic("error", "Permission denied in characteristic write", mapOf("exception" to e.javaClass.simpleName, "message" to (e.message ?: "unknown")))
            }
        }
    }
    
    // MARK: - GATT Client Callback
    
    private val gattClientCallback = object : BluetoothGattCallback() {
        override fun onConnectionStateChange(gatt: BluetoothGatt, status: Int, newState: Int) {
            when (newState) {
                BluetoothProfile.STATE_CONNECTED -> {
                    Log.i(TAG, "GATT client: Connected to ${gatt.device.address}")
                    emitDiagnostic("info", "Connected to BLE device", mapOf("address" to gatt.device.address))
                    try {
                        gatt.discoverServices()
                    } catch (e: SecurityException) {
                        Log.e(TAG, "Permission denied discovering services", e)
                        emitDiagnostic("error", "Permission denied discovering services", mapOf("exception" to e.javaClass.simpleName, "message" to (e.message ?: "unknown")))
                    }
                }
                BluetoothProfile.STATE_DISCONNECTED -> {
                    val address = gatt.device.address
                    val wasConnected = connections.getGatt(address) != null
                    connections.removeGatt(address)
                    
                    // Don't remove from discovered list - keep trying to reconnect
                    // Only remove RSSI if it's a permanent error
                    if (status == 133) { // Connection timeout
                        lastSeenRssi.remove(address)
                    }
                    
                    // Try to reconnect if we were connected and state is still running
                    if (wasConnected && state == TransportState.RUNNING) {
                        // Attempt reconnection after a short delay
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
                        }, 1000)
                    } else {
                        // Notify protocol of peer loss only if we're not reconnecting
                        connections.deviceIdForAddress(address)?.let { deviceId ->
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
            if (status == BluetoothGatt.GATT_SUCCESS) {
                val service = gatt.getService(SERVICE_UUID)
                if (service != null) {
                    emitDiagnostic("info", "GATT services discovered", mapOf("address" to gatt.device.address))
                    // Read device ID characteristic
                    val deviceIdChar = service.getCharacteristic(DEVICE_ID_CHAR_UUID)
                    if (deviceIdChar != null) {
                        try {
                            gatt.readCharacteristic(deviceIdChar)
                        } catch (e: SecurityException) {
                            Log.e(TAG, "Permission denied reading characteristic", e)
                            emitDiagnostic("error", "Permission denied reading device ID characteristic", mapOf("exception" to e.javaClass.simpleName, "message" to (e.message ?: "unknown")))
                        }
                    }
                    
                    // Enable notifications for message characteristic
                    val messageChar = service.getCharacteristic(MESSAGE_CHAR_UUID)
                    if (messageChar != null) {
                        try {
                            gatt.setCharacteristicNotification(messageChar, true)
                            Log.d(TAG, "Enabled notifications for message characteristic")
                            emitDiagnostic("info", "Enabled notifications for message characteristic", mapOf("address" to gatt.device.address))
                        } catch (e: SecurityException) {
                            Log.e(TAG, "Permission denied setting notification", e)
                            emitDiagnostic("error", "Permission denied enabling notifications", mapOf("exception" to e.javaClass.simpleName, "message" to (e.message ?: "unknown")))
                        }
                    }
                }
            } else {
                emitDiagnostic("error", "Service discovery failed", mapOf("address" to gatt.device.address, "status" to status))
            }
        }
        
        override fun onCharacteristicRead(gatt: BluetoothGatt, characteristic: BluetoothGattCharacteristic, status: Int) {
            if (status == BluetoothGatt.GATT_SUCCESS && characteristic.uuid == DEVICE_ID_CHAR_UUID) {
                val deviceIdValue = characteristic.value?.toString(Charsets.UTF_8)
                if (deviceIdValue != null) {
                    connections.setDeviceIdentifier(gatt.device.address, deviceIdValue)
                    
                    val role = connections.consumePendingRole(gatt.device.address) ?: MeshRole.MEMBER
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
                    val rssiInt = lastSeenRssi[gatt.device.address]?.toInt()
                    if (rssiInt != null) {
                        meshController.updatePeerMetrics(
                            deviceIdValue,
                            MeshController.PeerMetrics(rssi = rssiInt)
                        )
                    }
                    
                    // Notify protocol of peer discovery
                    val rssi = lastSeenRssi[gatt.device.address] ?: (-60).toShort()
                    try {
                        protocol.blePeerDiscovered(deviceIdValue, rssi)
                    } catch (e: Exception) {
                        Log.e(TAG, "Error notifying peer discovered", e)
                        emitDiagnostic("error", "Error notifying peer discovered", mapOf("exception" to e.javaClass.simpleName, "message" to (e.message ?: "unknown")))
                    }
                    
                    // Process any pending fragments for this device
                    processPendingFragments(gatt.device.address, deviceIdValue)
                }
            }
        }
        
        override fun onCharacteristicChanged(gatt: BluetoothGatt, characteristic: BluetoothGattCharacteristic) {
            if (characteristic.uuid == MESSAGE_CHAR_UUID) {
                val data = characteristic.value
                if (data != null) {
                    handleReceivedData(data, gatt.device.address)
                }
            }
        }
    }
}

