package com.offlineprotocol

import android.Manifest
import android.annotation.SuppressLint
import android.bluetooth.*
import android.bluetooth.le.*
import android.content.Context
import android.content.pm.PackageManager
import android.os.Build
import android.os.Handler
import android.os.Looper
import android.os.ParcelUuid
import androidx.core.content.ContextCompat
import java.nio.ByteBuffer
import java.util.*

/**
 * BLE Manager for Offline Protocol
 * 
 * Handles:
 * - BLE advertising (making device discoverable)
 * - BLE scanning (discovering nearby devices)
 * - GATT server (receiving messages)
 * - GATT client (sending messages)
 */
class BleManager(
    private val context: Context,
    private val deviceId: String,
    private val onPeerDiscovered: (String, String, Int) -> Unit, // deviceId, address, rssi
    private val onPeerUpdated: (String, String, Int) -> Unit,
    private val onPeerLost: (String) -> Unit,
    private val onMessageReceived: (ByteArray) -> Unit,
    private val onStatusChanged: (Status) -> Unit,
    private val onDiagnostic: (String) -> Unit = {}
) {
    enum class Status {
        UNAVAILABLE,
        AVAILABLE,
        SCANNING,
        ADVERTISING,
        CONNECTED,
        DISCONNECTED
    }

    companion object {
        // UUIDs for the Offline Protocol BLE service
        val SERVICE_UUID: UUID = UUID.fromString("6E400001-B5A3-F393-E0A9-E50E24DCCA9E")
        val MESSAGE_CHAR_UUID: UUID = UUID.fromString("6E400002-B5A3-F393-E0A9-E50E24DCCA9E")
        val DEVICE_ID_CHAR_UUID: UUID = UUID.fromString("6E400003-B5A3-F393-E0A9-E50E24DCCA9E")
        
        const val TAG = "BleManager"
    }

    private val bluetoothManager: BluetoothManager? = 
        context.getSystemService(Context.BLUETOOTH_SERVICE) as? BluetoothManager
    
    private val bluetoothAdapter: BluetoothAdapter? = bluetoothManager?.adapter
    
    private var bleAdvertiser: BluetoothLeAdvertiser? = null
    private var bleScanner: BluetoothLeScanner? = null
    private var gattServer: BluetoothGattServer? = null
    
    // Track discovered peers
    private val discoveredPeers = mutableMapOf<String, DiscoveredPeer>()
    
    // Track device IDs that have already been discovered (for deduplication)
    private val discoveredDeviceIds = mutableSetOf<String>()
    
    // Track connected GATT clients
    private val connectedClients = mutableMapOf<String, BluetoothGatt>()
    private val connectingClients = mutableMapOf<String, BluetoothGatt>()

    private val peerLossTimeoutMs = 60_000L
    private val peerCheckIntervalMs = 5_000L
    private val peerCheckHandler = Handler(Looper.getMainLooper())
    private val peerCheckRunnable = object : Runnable {
        override fun run() {
            checkForExpiredPeers()
            peerCheckHandler.postDelayed(this, peerCheckIntervalMs)
        }
    }

    data class DiscoveredPeer(
        val deviceId: String,
        var address: String,
        var device: BluetoothDevice,
        var rssi: Int,
        var lastSeen: Long
    )

    private fun startPeerExpiryMonitoring() {
        peerCheckHandler.removeCallbacks(peerCheckRunnable)
        peerCheckHandler.postDelayed(peerCheckRunnable, peerCheckIntervalMs)
    }

    private fun stopPeerExpiryMonitoring() {
        peerCheckHandler.removeCallbacks(peerCheckRunnable)
    }

    private fun checkForExpiredPeers() {
        val now = System.currentTimeMillis()
        val expired = mutableListOf<String>()

        discoveredPeers.forEach { (deviceId, peer) ->
            val isActive = connectedClients.containsKey(deviceId) || connectingClients.containsKey(deviceId)
            if (!isActive && now - peer.lastSeen > peerLossTimeoutMs) {
                expired.add(deviceId)
            }
        }

        if (expired.isEmpty()) {
            return
        }

        expired.forEach { deviceId ->
            val removed = discoveredPeers.remove(deviceId)
            if (removed != null) {
                discoveredDeviceIds.remove(deviceId)
                connectedClients.remove(deviceId)?.close()
                connectingClients.remove(deviceId)?.close()
                onDiagnostic("[BLE] ⏳ Peer $deviceId expired after ${peerLossTimeoutMs}ms without advertisements")
                onPeerLost(deviceId)
            }
        }
    }

    /**
     * Check if BLE is available and enabled
     */
    fun isBluetoothAvailable(): Boolean {
        return bluetoothAdapter?.isEnabled == true
    }

    /**
     * Check if all required permissions are granted
     */
    fun hasPermissions(): Boolean {
        val required = if (Build.VERSION.SDK_INT >= Build.VERSION_CODES.S) {
            arrayOf(
                Manifest.permission.BLUETOOTH_SCAN,
                Manifest.permission.BLUETOOTH_CONNECT,
                Manifest.permission.BLUETOOTH_ADVERTISE,
                Manifest.permission.ACCESS_FINE_LOCATION
            )
        } else {
            arrayOf(
                Manifest.permission.ACCESS_FINE_LOCATION,
                Manifest.permission.ACCESS_COARSE_LOCATION
            )
        }

        return required.all { 
            ContextCompat.checkSelfPermission(context, it) == PackageManager.PERMISSION_GRANTED
        }
    }

    /**
     * Start BLE operations (advertising + scanning)
     */
    @SuppressLint("MissingPermission")
    fun start(): Boolean {
        if (!isBluetoothAvailable()) {
            android.util.Log.e(TAG, "Bluetooth not available")
            onStatusChanged(Status.UNAVAILABLE)
            return false
        }

        if (!hasPermissions()) {
            android.util.Log.e(TAG, "Missing Bluetooth permissions")
            onStatusChanged(Status.UNAVAILABLE)
            return false
        }

        try {
            // Start GATT server first
            startGattServer()
            
            // Then start advertising
            startAdvertising()
            
            // Finally start scanning
            startScanning()
            
            onStatusChanged(Status.AVAILABLE)
            android.util.Log.d(TAG, "BLE started successfully")
            startPeerExpiryMonitoring()
            return true
        } catch (e: SecurityException) {
            android.util.Log.e(TAG, "Security exception starting BLE", e)
            onStatusChanged(Status.UNAVAILABLE)
            return false
        } catch (e: Exception) {
            android.util.Log.e(TAG, "Error starting BLE", e)
            onStatusChanged(Status.UNAVAILABLE)
            return false
        }
    }

    /**
     * Stop all BLE operations
     */
    @SuppressLint("MissingPermission")
    fun stop() {
        try {
            stopPeerExpiryMonitoring()
            stopAdvertising()
            stopScanning()
            stopGattServer()
            
            // Clear tracking data
            discoveredPeers.clear()
            discoveredDeviceIds.clear()
            connectedClients.values.forEach { it.close() }
            connectedClients.clear()
            connectingClients.values.forEach { it.close() }
            connectingClients.clear()
            
            onStatusChanged(Status.DISCONNECTED)
            android.util.Log.d(TAG, "BLE stopped")
        } catch (e: Exception) {
            android.util.Log.e(TAG, "Error stopping BLE", e)
        }
    }

    /**
     * Start GATT server to receive messages
     */
    @SuppressLint("MissingPermission")
    private fun startGattServer() {
        onDiagnostic("[BLE] 🔧 Starting GATT server...")
        gattServer = bluetoothManager?.openGattServer(context, gattServerCallback)
        
        val service = BluetoothGattService(SERVICE_UUID, BluetoothGattService.SERVICE_TYPE_PRIMARY)
        
        // Message characteristic (write, notify)
        val messageChar = BluetoothGattCharacteristic(
            MESSAGE_CHAR_UUID,
            BluetoothGattCharacteristic.PROPERTY_WRITE or BluetoothGattCharacteristic.PROPERTY_NOTIFY,
            BluetoothGattCharacteristic.PERMISSION_WRITE
        )
        
        // Device ID characteristic (read)
        val deviceIdBytes = deviceId.toByteArray()
        val deviceIdChar = BluetoothGattCharacteristic(
            DEVICE_ID_CHAR_UUID,
            BluetoothGattCharacteristic.PROPERTY_READ,
            BluetoothGattCharacteristic.PERMISSION_READ
        )
        deviceIdChar.value = deviceIdBytes
        
        onDiagnostic("[BLE] 📝 Device ID characteristic value: '$deviceId' (${deviceIdBytes.size} bytes)")
        
        service.addCharacteristic(messageChar)
        service.addCharacteristic(deviceIdChar)
        
        gattServer?.addService(service)
        android.util.Log.d(TAG, "GATT server started")
        onDiagnostic("[BLE] ✅ GATT server started with 2 characteristics")
    }

    /**
     * Stop GATT server
     */
    @SuppressLint("MissingPermission")
    private fun stopGattServer() {
        gattServer?.close()
        gattServer = null
    }

    /**
     * Start BLE advertising
     */
    @SuppressLint("MissingPermission")
    private fun startAdvertising() {
        bleAdvertiser = bluetoothAdapter?.bluetoothLeAdvertiser
        
        if (bleAdvertiser == null) {
            android.util.Log.e(TAG, "BLE advertiser not available")
            onDiagnostic("[BLE] ❌ BLE advertiser not available")
            return
        }

        val settings = AdvertiseSettings.Builder()
            .setAdvertiseMode(AdvertiseSettings.ADVERTISE_MODE_BALANCED)
            .setConnectable(true)
            .setTimeout(0)
            .setTxPowerLevel(AdvertiseSettings.ADVERTISE_TX_POWER_MEDIUM)
            .build()

        val data = AdvertiseData.Builder()
            .setIncludeDeviceName(false)
            .setIncludeTxPowerLevel(false)
            .addServiceUuid(ParcelUuid(SERVICE_UUID))
            .build()

        onDiagnostic("[BLE] 📡 Starting BLE advertising with service UUID: $SERVICE_UUID")
        bleAdvertiser?.startAdvertising(settings, data, advertiseCallback)
        android.util.Log.d(TAG, "BLE advertising started")
    }

    /**
     * Stop BLE advertising
     */
    @SuppressLint("MissingPermission")
    private fun stopAdvertising() {
        bleAdvertiser?.stopAdvertising(advertiseCallback)
        bleAdvertiser = null
    }

    /**
     * Start BLE scanning
     */
    @SuppressLint("MissingPermission")
    private fun startScanning() {
        bleScanner = bluetoothAdapter?.bluetoothLeScanner
        
        if (bleScanner == null) {
            android.util.Log.e(TAG, "BLE scanner not available")
            return
        }

        val scanSettings = ScanSettings.Builder()
            .setScanMode(ScanSettings.SCAN_MODE_BALANCED)
            .build()

        val scanFilters = listOf(
            ScanFilter.Builder()
                .setServiceUuid(ParcelUuid(SERVICE_UUID))
                .build()
        )

        bleScanner?.startScan(scanFilters, scanSettings, scanCallback)
        onStatusChanged(Status.SCANNING)
        android.util.Log.d(TAG, "BLE scanning started")
    }

    /**
     * Stop BLE scanning
     */
    @SuppressLint("MissingPermission")
    private fun stopScanning() {
        bleScanner?.stopScan(scanCallback)
        bleScanner = null
    }

    // Advertise callback
    private val advertiseCallback = object : AdvertiseCallback() {
        override fun onStartSuccess(settingsInEffect: AdvertiseSettings) {
            val msg = "[BLE] ✅ Advertising started successfully - device is now discoverable and connectable"
            android.util.Log.d(TAG, msg)
            onDiagnostic(msg)
            onStatusChanged(Status.ADVERTISING)
        }

        override fun onStartFailure(errorCode: Int) {
            val msg = "[BLE] ❌ Advertising failed with error: $errorCode"
            android.util.Log.e(TAG, msg)
            onDiagnostic(msg)
            onStatusChanged(Status.UNAVAILABLE)
        }
    }

    // Scan callback
    private val scanCallback = object : ScanCallback() {
        @SuppressLint("MissingPermission")
        override fun onScanResult(callbackType: Int, result: ScanResult) {
            val device = result.device
            val rssi = result.rssi
            
            // Check if we already discovered this device by address
            // Update existing peer's RSSI without reconnecting
            val existingPeer = discoveredPeers.values.find { it.address == device.address }
            if (existingPeer != null) {
                // Update RSSI, lastSeen, and device reference
                existingPeer.address = device.address
                existingPeer.device = device
                existingPeer.rssi = rssi
                existingPeer.lastSeen = System.currentTimeMillis()
                onPeerUpdated(existingPeer.deviceId, device.address, rssi)
                // Don't reconnect - peer is already known
                return
            }
            
            // Connect to device to read its device ID (only for new devices)
            device.connectGatt(context, false, object : BluetoothGattCallback() {
                override fun onConnectionStateChange(gatt: BluetoothGatt, status: Int, newState: Int) {
                    if (newState == BluetoothProfile.STATE_CONNECTED) {
                        android.util.Log.d(TAG, "Connected to ${device.address}")
                        gatt.discoverServices()
                    } else if (newState == BluetoothProfile.STATE_DISCONNECTED) {
                        android.util.Log.d(TAG, "Disconnected from ${device.address}")
                        val entry = connectedClients.entries.find { it.value == gatt }
                        if (entry != null) {
                            connectedClients.remove(entry.key)
                            discoveredPeers[entry.key]?.let { peer ->
                                discoveredPeers[entry.key] = peer.copy(lastSeen = System.currentTimeMillis())
                            }
                        }
                        gatt.close()
                    }
                }

                override fun onServicesDiscovered(gatt: BluetoothGatt, status: Int) {
                    if (status == BluetoothGatt.GATT_SUCCESS) {
                        val service = gatt.getService(SERVICE_UUID)
                        val deviceIdChar = service?.getCharacteristic(DEVICE_ID_CHAR_UUID)
                        
                        if (deviceIdChar != null) {
                            gatt.readCharacteristic(deviceIdChar)
                        } else {
                            gatt.close()
                        }
                    } else {
                        gatt.close()
                    }
                }

                override fun onCharacteristicRead(
                    gatt: BluetoothGatt,
                    characteristic: BluetoothGattCharacteristic,
                    status: Int
                ) {
                    if (status == BluetoothGatt.GATT_SUCCESS && 
                        characteristic.uuid == DEVICE_ID_CHAR_UUID) {
                        
                        val remoteDeviceId = String(characteristic.value)
                        
                        // Check if we've already discovered this peer
                        if (discoveredDeviceIds.contains(remoteDeviceId)) {
                            // Peer already discovered - update metadata silently
                            discoveredPeers[remoteDeviceId]?.let { peer ->
                                peer.address = device.address
                                peer.device = device
                                peer.rssi = rssi
                                peer.lastSeen = System.currentTimeMillis()
                                onPeerUpdated(remoteDeviceId, device.address, rssi)
                            }
                            android.util.Log.d(TAG, "Updated existing peer: $remoteDeviceId (RSSI: $rssi)")
                            
                            // CRITICAL FIX: Disconnect after reading device ID
                            // Don't maintain persistent connections - reconnect on-demand for messaging
                            gatt.disconnect()
                            gatt.close()
                            
                        } else {
                            // New peer - emit discovery event
                            android.util.Log.d(TAG, "Discovered NEW peer: $remoteDeviceId at ${device.address} (RSSI: $rssi)")
                            
                            // Add to discovered set
                            discoveredDeviceIds.add(remoteDeviceId)
                            
                            // Store peer
                            discoveredPeers[remoteDeviceId] = DiscoveredPeer(
                                deviceId = remoteDeviceId,
                                address = device.address,
                                device = device,
                                rssi = rssi,
                                lastSeen = System.currentTimeMillis()
                            )
                            
                            // Notify discovery (only once)
                            onPeerDiscovered(remoteDeviceId, device.address, rssi)
                            
                            // CRITICAL FIX: Disconnect after reading device ID
                            // We'll reconnect on-demand when sending messages
                            gatt.disconnect()
                            gatt.close()
                        }
                    } else {
                        android.util.Log.w(TAG, "Failed to read device ID characteristic: status=$status")
                        gatt.close()
                    }
                }
            })
        }

        override fun onScanFailed(errorCode: Int) {
            android.util.Log.e(TAG, "Scan failed with error: $errorCode")
        }
    }

    // GATT server callback
    private val gattServerCallback = object : BluetoothGattServerCallback() {
        @SuppressLint("MissingPermission")
        override fun onConnectionStateChange(device: BluetoothDevice, status: Int, newState: Int) {
            super.onConnectionStateChange(device, status, newState)
            when (newState) {
                BluetoothProfile.STATE_CONNECTED -> {
                    val msg = "[BLE] ✅ GATT client connected: ${device.address}"
                    android.util.Log.d(TAG, msg)
                    onDiagnostic(msg)
                }
                BluetoothProfile.STATE_DISCONNECTED -> {
                    val msg = "[BLE] ⚠️ GATT client disconnected: ${device.address}"
                    android.util.Log.d(TAG, msg)
                    onDiagnostic(msg)
                }
            }
        }
        
        @SuppressLint("MissingPermission")
        override fun onCharacteristicReadRequest(
            device: BluetoothDevice,
            requestId: Int,
            offset: Int,
            characteristic: BluetoothGattCharacteristic
        ) {
            val msg = "[BLE] 📖 Read request from ${device.address} for ${characteristic.uuid}"
            android.util.Log.d(TAG, msg)
            onDiagnostic(msg)
            
            if (characteristic.uuid == DEVICE_ID_CHAR_UUID) {
                val deviceIdBytes = deviceId.toByteArray()
                val msg2 = "[BLE] 📤 Sending device ID '$deviceId' (${deviceIdBytes.size} bytes) to ${device.address}"
                android.util.Log.d(TAG, msg2)
                onDiagnostic(msg2)
                
                gattServer?.sendResponse(
                    device,
                    requestId,
                    BluetoothGatt.GATT_SUCCESS,
                    offset,
                    deviceIdBytes
                )
            } else {
                val msg2 = "[BLE] ❌ Read request for unknown characteristic: ${characteristic.uuid}"
                android.util.Log.w(TAG, msg2)
                onDiagnostic(msg2)
                gattServer?.sendResponse(
                    device,
                    requestId,
                    BluetoothGatt.GATT_FAILURE,
                    offset,
                    null
                )
            }
        }
        
        @SuppressLint("MissingPermission")
        override fun onCharacteristicWriteRequest(
            device: BluetoothDevice,
            requestId: Int,
            characteristic: BluetoothGattCharacteristic,
            preparedWrite: Boolean,
            responseNeeded: Boolean,
            offset: Int,
            value: ByteArray
        ) {
            if (characteristic.uuid == MESSAGE_CHAR_UUID) {
                android.util.Log.d(TAG, "Received message from ${device.address}")
                
                // Notify received message
                onMessageReceived(value)
                
                if (responseNeeded) {
                    gattServer?.sendResponse(
                        device,
                        requestId,
                        BluetoothGatt.GATT_SUCCESS,
                        offset,
                        value
                    )
                }
            } else {
                android.util.Log.w(TAG, "Write request for unknown characteristic: ${characteristic.uuid}")
                if (responseNeeded) {
                    gattServer?.sendResponse(
                        device,
                        requestId,
                        BluetoothGatt.GATT_FAILURE,
                        offset,
                        null
                    )
                }
            }
        }
    }

    /**
     * Send message to a specific peer
     */
    @SuppressLint("MissingPermission")
    fun sendMessage(recipientId: String, messageData: ByteArray): Boolean {
        // CRITICAL FIX: Since we disconnect after reading device ID,
        // we need to reconnect on-demand for messaging
        
        val peer = discoveredPeers[recipientId]
        if (peer == null) {
            android.util.Log.e(TAG, "Peer not discovered: $recipientId")
            return false
        }
        
        // Check if we have an active or in-flight connection
        connectedClients[recipientId]?.let { gatt ->
            return writeMessage(gatt, recipientId, messageData)
        }

        if (!connectingClients.containsKey(recipientId)) {
            android.util.Log.d(TAG, "Peer $recipientId not connected, starting messaging connection")
            connectForMessaging(peer)
        } else {
            android.util.Log.d(TAG, "Peer $recipientId connection already in progress")
        }

        return false
    }

    @SuppressLint("MissingPermission")
    private fun writeMessage(
        gatt: BluetoothGatt,
        recipientId: String,
        messageData: ByteArray
    ): Boolean {
        val service = gatt.getService(SERVICE_UUID)
        val messageChar = service?.getCharacteristic(MESSAGE_CHAR_UUID)

        if (service == null || messageChar == null) {
            android.util.Log.w(TAG, "Message characteristic not ready for $recipientId; rediscovering services")
            onDiagnostic("[BLE] ℹ️ Message characteristic not ready for $recipientId – rediscovering")
            gatt.discoverServices()
            return false
        }

        messageChar.value = messageData
        val success = gatt.writeCharacteristic(messageChar)

        android.util.Log.d(TAG, "Send message to $recipientId: $success")
        if (!success) {
            onDiagnostic("[BLE] ❌ Failed to write BLE fragment to $recipientId")
        }
        return success
    }

    @SuppressLint("MissingPermission")
    private fun connectForMessaging(peer: DiscoveredPeer) {
        onDiagnostic("[BLE] 🔄 Connecting to ${peer.deviceId} (${peer.address}) for messaging")

        val callback = object : BluetoothGattCallback() {
            override fun onConnectionStateChange(gatt: BluetoothGatt, status: Int, newState: Int) {
                if (status != BluetoothGatt.GATT_SUCCESS || newState != BluetoothProfile.STATE_CONNECTED) {
                    android.util.Log.w(TAG, "Messaging connection failed for ${peer.deviceId}: status=$status state=$newState")
                    onDiagnostic("[BLE] ❌ Messaging connection failed for ${peer.deviceId} (status=$status, state=$newState)")
                    connectingClients.remove(peer.deviceId)
                    connectedClients.remove(peer.deviceId)?.close()
                    gatt.disconnect()
                    gatt.close()
                    return
                }

                android.util.Log.d(TAG, "Messaging connection established for ${peer.deviceId}")
                onDiagnostic("[BLE] ✅ Messaging connection established for ${peer.deviceId}")
                gatt.discoverServices()
            }

            override fun onServicesDiscovered(gatt: BluetoothGatt, status: Int) {
                if (status != BluetoothGatt.GATT_SUCCESS) {
                    android.util.Log.w(TAG, "Service discovery failed for ${peer.deviceId}: status=$status")
                    onDiagnostic("[BLE] ❌ Messaging service discovery failed for ${peer.deviceId} (status=$status)")
                    connectingClients.remove(peer.deviceId)
                    gatt.disconnect()
                    gatt.close()
                    return
                }

                val service = gatt.getService(SERVICE_UUID)
                val messageChar = service?.getCharacteristic(MESSAGE_CHAR_UUID)

                if (service == null || messageChar == null) {
                    android.util.Log.w(TAG, "Messaging characteristics missing for ${peer.deviceId}")
                    onDiagnostic("[BLE] ❌ Messaging characteristics missing for ${peer.deviceId}")
                    connectingClients.remove(peer.deviceId)
                    gatt.disconnect()
                    gatt.close()
                    return
                }

                connectingClients.remove(peer.deviceId)
                connectedClients[peer.deviceId] = gatt
                android.util.Log.d(TAG, "Messaging connection ready for ${peer.deviceId}")
                onDiagnostic("[BLE] 🔗 Ready to send BLE fragments to ${peer.deviceId}")
            }

            override fun onCharacteristicWrite(
                gatt: BluetoothGatt,
                characteristic: BluetoothGattCharacteristic,
                status: Int
            ) {
                if (characteristic.uuid == MESSAGE_CHAR_UUID && status != BluetoothGatt.GATT_SUCCESS) {
                    android.util.Log.w(TAG, "Message write failed for ${peer.deviceId}: status=$status")
                    onDiagnostic("[BLE] ❌ Message write failed for ${peer.deviceId} (status=$status)")
                }
            }

            override fun onCharacteristicChanged(
                gatt: BluetoothGatt,
                characteristic: BluetoothGattCharacteristic
            ) {
                // Forward incoming messages if necessary
                if (characteristic.uuid == MESSAGE_CHAR_UUID) {
                    onMessageReceived(characteristic.value)
                }
            }

            override fun onReadRemoteRssi(gatt: BluetoothGatt, rssi: Int, status: Int) {
                if (status == BluetoothGatt.GATT_SUCCESS) {
                    discoveredPeers[peer.deviceId]?.let {
                        it.rssi = rssi
                        it.lastSeen = System.currentTimeMillis()
                    }
                    onPeerUpdated(peer.deviceId, peer.address, rssi)
                }
            }
        }

        val gatt = peer.device.connectGatt(context, false, callback)
        connectingClients[peer.deviceId] = gatt
    }

    /**
     * Get list of discovered peers
     */
    fun getDiscoveredPeers(): List<DiscoveredPeer> {
        return discoveredPeers.values.toList()
    }
}

