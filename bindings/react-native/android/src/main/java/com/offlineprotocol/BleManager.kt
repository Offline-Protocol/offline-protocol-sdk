package com.offlineprotocol

import android.Manifest
import android.bluetooth.*
import android.bluetooth.le.*
import android.content.Context
import android.content.pm.PackageManager
import android.os.Build
import android.os.Handler
import android.os.Looper
import android.os.ParcelUuid
import android.util.Log
import androidx.core.content.ContextCompat
import uniffi.offline_protocol.OfflineProtocol
import java.util.*
import java.util.concurrent.ConcurrentHashMap

/**
 * BLE Manager implementing TransportManager for Bluetooth Low Energy communication
 * Ensures iOS ↔ Android cross-platform compatibility
 */
class BleManager(
    private val context: Context,
    private val protocol: OfflineProtocol,
    private val deviceId: String
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
    
    // GATT Clients (central role - connections to discovered devices)
    private val gattClients = ConcurrentHashMap<String, BluetoothGatt>()
    private val deviceAddressToId = ConcurrentHashMap<String, String>()
    private val deviceIdToAddress = ConcurrentHashMap<String, String>()
    
    // Fragment polling
    private val mainHandler = Handler(Looper.getMainLooper())
    private val fragmentPollingRunnable = object : Runnable {
        override fun run() {
            pollAndSendFragments()
            if (state == TransportState.RUNNING) {
                mainHandler.postDelayed(this, FRAGMENT_POLL_INTERVAL_MS)
            }
        }
    }
    
    // Metrics
    private var bytesSent: Long = 0
    private var bytesReceived: Long = 0
    private var fragmentsSent: Long = 0
    private var fragmentsReceived: Long = 0
    
    // MARK: - TransportManager Implementation
    
    override fun isAvailable(): Boolean {
        if (bluetoothAdapter == null) {
            Log.w(TAG, "Bluetooth adapter not available")
            return false
        }
        
        if (!context.packageManager.hasSystemFeature(PackageManager.FEATURE_BLUETOOTH_LE)) {
            Log.w(TAG, "BLE not supported on this device")
            return false
        }
        
        return true
    }
    
    override fun start() {
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
        updateState(TransportState.STARTING)
        
        try {
            // Initialize scanner
            bluetoothLeScanner = bluetoothAdapter.bluetoothLeScanner
            
            // Initialize advertiser
            bluetoothLeAdvertiser = bluetoothAdapter.bluetoothLeAdvertiser
            
            // Setup GATT server
            setupGattServer()
            
            // Start advertising
            startAdvertising()
            
            // Start scanning
            startScanning()
            
            // Start fragment polling
            mainHandler.post(fragmentPollingRunnable)
            
            updateState(TransportState.RUNNING)
            protocol.bleStatusChanged(true)
            
            Log.i(TAG, "BLE Manager started successfully - scanning and advertising active")
        } catch (e: Exception) {
            Log.e(TAG, "Failed to start BLE manager", e)
            updateState(TransportState.STOPPED)
            throw TransportException.StartFailed("Failed to start BLE manager", e)
        }
    }
    
    override fun stop() {
        if (state != TransportState.RUNNING && state != TransportState.STARTING) {
            return
        }
        
        updateState(TransportState.STOPPING)
        
        // Stop fragment polling
        mainHandler.removeCallbacks(fragmentPollingRunnable)
        
        // Stop scanning
        stopScanning()
        
        // Stop advertising
        stopAdvertising()
        
        // Disconnect all GATT clients
        gattClients.values.forEach { gatt ->
            try {
                gatt.disconnect()
                gatt.close()
            } catch (e: Exception) {
                Log.e(TAG, "Error closing GATT client", e)
            }
        }
        gattClients.clear()
        deviceAddressToId.clear()
        deviceIdToAddress.clear()
        
        // Close GATT server
        gattServer?.close()
        gattServer = null
        
        updateState(TransportState.STOPPED)
        protocol.bleStatusChanged(false)
        
        Log.i(TAG, "BLE Manager stopped")
    }
    
    override fun pause() {
        // For Android background mode
        stopScanning()
        mainHandler.removeCallbacks(fragmentPollingRunnable)
    }
    
    override fun resume() {
        // Resume from background
        if (state == TransportState.RUNNING) {
            startScanning()
            mainHandler.post(fragmentPollingRunnable)
        }
    }
    
    override fun getMetrics(): Map<String, Any> {
        return mapOf(
            "bytes_sent" to bytesSent,
            "bytes_received" to bytesReceived,
            "fragments_sent" to fragmentsSent,
            "fragments_received" to fragmentsReceived,
            "connected_peers" to gattClients.size,
            "discovered_peers" to deviceAddressToId.size
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
    
    private fun startScanning() {
        if (isScanning) return
        
        try {
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
                }
            }
            
            bluetoothLeScanner?.startScan(listOf(scanFilter), scanSettings, scanCallback)
            isScanning = true
            
            Log.i(TAG, "Started scanning for service: $SERVICE_UUID")
        } catch (e: SecurityException) {
            Log.e(TAG, "Permission denied while starting scan", e)
            throw e
        }
    }
    
    private fun stopScanning() {
        if (!isScanning) return
        
        try {
            scanCallback?.let { bluetoothLeScanner?.stopScan(it) }
            scanCallback = null
            isScanning = false
            Log.i(TAG, "Stopped scanning")
        } catch (e: SecurityException) {
            Log.e(TAG, "Permission denied while stopping scan", e)
        }
    }
    
    private fun startAdvertising() {
        if (isAdvertising) return
        
        try {
            val settings = AdvertiseSettings.Builder()
                .setAdvertiseMode(AdvertiseSettings.ADVERTISE_MODE_LOW_LATENCY)
                .setConnectable(true)
                .setTimeout(0)
                .setTxPowerLevel(AdvertiseSettings.ADVERTISE_TX_POWER_HIGH)
                .build()
            
            val data = AdvertiseData.Builder()
                .setIncludeDeviceName(false)
                .addServiceUuid(ParcelUuid(SERVICE_UUID))
                .build()
            
            advertiseCallback = object : AdvertiseCallback() {
                override fun onStartSuccess(settingsInEffect: AdvertiseSettings) {
                    Log.i(TAG, "Advertising started successfully")
                    isAdvertising = true
                }
                
                override fun onStartFailure(errorCode: Int) {
                    Log.e(TAG, "Advertising failed with error code: $errorCode")
                    isAdvertising = false
                }
            }
            
            bluetoothLeAdvertiser?.startAdvertising(settings, data, advertiseCallback)
            
            Log.i(TAG, "Starting advertising with service: $SERVICE_UUID")
        } catch (e: SecurityException) {
            Log.e(TAG, "Permission denied while starting advertising", e)
            throw e
        }
    }
    
    private fun stopAdvertising() {
        if (!isAdvertising) return
        
        try {
            advertiseCallback?.let { bluetoothLeAdvertiser?.stopAdvertising(it) }
            advertiseCallback = null
            isAdvertising = false
            Log.i(TAG, "Stopped advertising")
        } catch (e: SecurityException) {
            Log.e(TAG, "Permission denied while stopping advertising", e)
        }
    }
    
    private fun handleScanResult(result: ScanResult) {
        val device = result.device
        val rssi = result.rssi.toShort()
        val address = device.address
        
        Log.d(TAG, "Discovered device: $address RSSI: $rssi")
        
        // Connect to device if not already connected
        if (!gattClients.containsKey(address)) {
            connectToDevice(device)
        }
    }
    
    private fun connectToDevice(device: BluetoothDevice) {
        try {
            val gatt = device.connectGatt(context, false, gattClientCallback, BluetoothDevice.TRANSPORT_LE)
            gattClients[device.address] = gatt
            
            Log.i(TAG, "Connecting to device: ${device.address}")
        } catch (e: SecurityException) {
            Log.e(TAG, "Permission denied while connecting to device", e)
        }
    }
    
    private fun pollAndSendFragments() {
        try {
            // Poll for next fragment from protocol
            val fragment = protocol.bleGetNextFragment() ?: return
            
            val recipientId = fragment.recipientId
            val data = fragment.data.map { it.toByte() }.toByteArray()
            
            // Find GATT client for recipient
            val address = deviceIdToAddress[recipientId]
            val gatt = address?.let { gattClients[it] }
            
            if (gatt == null) {
                Log.w(TAG, "No connected device for recipient: $recipientId")
                return
            }
            
            // Find message characteristic
            val service = gatt.getService(SERVICE_UUID)
            val characteristic = service?.getCharacteristic(MESSAGE_CHAR_UUID)
            
            if (characteristic == null) {
                Log.w(TAG, "Message characteristic not found for recipient: $recipientId")
                return
            }
            
            // Write data
            characteristic.value = data
            characteristic.writeType = BluetoothGattCharacteristic.WRITE_TYPE_NO_RESPONSE
            gatt.writeCharacteristic(characteristic)
            
            bytesSent += data.size
            fragmentsSent++
            
            Log.d(TAG, "Sent fragment to $recipientId: ${data.size} bytes")
        } catch (e: Exception) {
            Log.e(TAG, "Error polling/sending fragments", e)
        }
    }
    
    private fun handleReceivedData(data: ByteArray, address: String) {
        try {
            // Get sender device ID
            val senderId = deviceAddressToId[address]
            if (senderId == null) {
                Log.w(TAG, "Unknown sender address: $address")
                return
            }
            
            // Convert to UByte list
            val bytes = data.map { it.toUByte() }
            
            // Pass to protocol
            protocol.bleFragmentReceived(senderId, bytes)
            
            bytesReceived += data.size
            fragmentsReceived++
            
            Log.d(TAG, "Received fragment from $senderId: ${data.size} bytes")
        } catch (e: Exception) {
            Log.e(TAG, "Error processing received fragment", e)
        }
    }
    
    // MARK: - GATT Server Callback
    
    private val gattServerCallback = object : BluetoothGattServerCallback() {
        override fun onConnectionStateChange(device: BluetoothDevice, status: Int, newState: Int) {
            when (newState) {
                BluetoothProfile.STATE_CONNECTED -> {
                    Log.i(TAG, "GATT server: Device connected: ${device.address}")
                }
                BluetoothProfile.STATE_DISCONNECTED -> {
                    Log.i(TAG, "GATT server: Device disconnected: ${device.address}")
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
                if (characteristic.uuid == MESSAGE_CHAR_UUID) {
                    // Handle incoming fragment
                    handleReceivedData(value, device.address)
                    
                    if (responseNeeded) {
                        gattServer?.sendResponse(device, requestId, BluetoothGatt.GATT_SUCCESS, offset, value)
                    }
                } else {
                    if (responseNeeded) {
                        gattServer?.sendResponse(device, requestId, BluetoothGatt.GATT_FAILURE, offset, null)
                    }
                }
            } catch (e: SecurityException) {
                Log.e(TAG, "Permission denied in write request", e)
            }
        }
    }
    
    // MARK: - GATT Client Callback
    
    private val gattClientCallback = object : BluetoothGattCallback() {
        override fun onConnectionStateChange(gatt: BluetoothGatt, status: Int, newState: Int) {
            when (newState) {
                BluetoothProfile.STATE_CONNECTED -> {
                    Log.i(TAG, "GATT client: Connected to ${gatt.device.address}")
                    try {
                        gatt.discoverServices()
                    } catch (e: SecurityException) {
                        Log.e(TAG, "Permission denied discovering services", e)
                    }
                }
                BluetoothProfile.STATE_DISCONNECTED -> {
                    Log.i(TAG, "GATT client: Disconnected from ${gatt.device.address}")
                    gattClients.remove(gatt.device.address)
                    
                    // Notify protocol of peer loss
                    deviceAddressToId[gatt.device.address]?.let { deviceId ->
                        protocol.blePeerLost(deviceId)
                        deviceAddressToId.remove(gatt.device.address)
                        deviceIdToAddress.remove(deviceId)
                    }
                }
            }
        }
        
        override fun onServicesDiscovered(gatt: BluetoothGatt, status: Int) {
            if (status == BluetoothGatt.GATT_SUCCESS) {
                val service = gatt.getService(SERVICE_UUID)
                if (service != null) {
                    // Read device ID characteristic
                    val deviceIdChar = service.getCharacteristic(DEVICE_ID_CHAR_UUID)
                    if (deviceIdChar != null) {
                        try {
                            gatt.readCharacteristic(deviceIdChar)
                        } catch (e: SecurityException) {
                            Log.e(TAG, "Permission denied reading characteristic", e)
                        }
                    }
                    
                    // Enable notifications for message characteristic
                    val messageChar = service.getCharacteristic(MESSAGE_CHAR_UUID)
                    if (messageChar != null) {
                        try {
                            gatt.setCharacteristicNotification(messageChar, true)
                            Log.d(TAG, "Enabled notifications for message characteristic")
                        } catch (e: SecurityException) {
                            Log.e(TAG, "Permission denied setting notification", e)
                        }
                    }
                }
            }
        }
        
        override fun onCharacteristicRead(gatt: BluetoothGatt, characteristic: BluetoothGattCharacteristic, status: Int) {
            if (status == BluetoothGatt.GATT_SUCCESS && characteristic.uuid == DEVICE_ID_CHAR_UUID) {
                val deviceIdValue = characteristic.value?.toString(Charsets.UTF_8)
                if (deviceIdValue != null) {
                    deviceAddressToId[gatt.device.address] = deviceIdValue
                    deviceIdToAddress[deviceIdValue] = gatt.device.address
                    
                    // Notify protocol of peer discovery
                    // RSSI will be updated from scan results
                    protocol.blePeerDiscovered(deviceIdValue, -50)
                    
                    Log.i(TAG, "Peer discovered: $deviceIdValue at ${gatt.device.address}")
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

