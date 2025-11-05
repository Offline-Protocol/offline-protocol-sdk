package com.offlineprotocol

import android.Manifest
import android.annotation.SuppressLint
import android.bluetooth.*
import android.bluetooth.le.*
import android.content.Context
import android.content.pm.PackageManager
import android.os.Build
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
    private val onPeerLost: (String) -> Unit,
    private val onMessageReceived: (ByteArray) -> Unit,
    private val onStatusChanged: (Status) -> Unit
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
    
    // Track connected GATT clients
    private val connectedClients = mutableMapOf<String, BluetoothGatt>()

    data class DiscoveredPeer(
        val deviceId: String,
        val address: String,
        var rssi: Int,
        var lastSeen: Long
    )

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
            stopAdvertising()
            stopScanning()
            stopGattServer()
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
        gattServer = bluetoothManager?.openGattServer(context, gattServerCallback)
        
        val service = BluetoothGattService(SERVICE_UUID, BluetoothGattService.SERVICE_TYPE_PRIMARY)
        
        // Message characteristic (write, notify)
        val messageChar = BluetoothGattCharacteristic(
            MESSAGE_CHAR_UUID,
            BluetoothGattCharacteristic.PROPERTY_WRITE or BluetoothGattCharacteristic.PROPERTY_NOTIFY,
            BluetoothGattCharacteristic.PERMISSION_WRITE
        )
        
        // Device ID characteristic (read)
        val deviceIdChar = BluetoothGattCharacteristic(
            DEVICE_ID_CHAR_UUID,
            BluetoothGattCharacteristic.PROPERTY_READ,
            BluetoothGattCharacteristic.PERMISSION_READ
        )
        deviceIdChar.value = deviceId.toByteArray()
        
        service.addCharacteristic(messageChar)
        service.addCharacteristic(deviceIdChar)
        
        gattServer?.addService(service)
        android.util.Log.d(TAG, "GATT server started")
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
            android.util.Log.d(TAG, "Advertising started successfully")
            onStatusChanged(Status.ADVERTISING)
        }

        override fun onStartFailure(errorCode: Int) {
            android.util.Log.e(TAG, "Advertising failed with error: $errorCode")
            onStatusChanged(Status.UNAVAILABLE)
        }
    }

    // Scan callback
    private val scanCallback = object : ScanCallback() {
        @SuppressLint("MissingPermission")
        override fun onScanResult(callbackType: Int, result: ScanResult) {
            val device = result.device
            val rssi = result.rssi
            
            // Connect to device to read its device ID
            device.connectGatt(context, false, object : BluetoothGattCallback() {
                override fun onConnectionStateChange(gatt: BluetoothGatt, status: Int, newState: Int) {
                    if (newState == BluetoothProfile.STATE_CONNECTED) {
                        android.util.Log.d(TAG, "Connected to ${device.address}")
                        gatt.discoverServices()
                    } else if (newState == BluetoothProfile.STATE_DISCONNECTED) {
                        android.util.Log.d(TAG, "Disconnected from ${device.address}")
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
                        android.util.Log.d(TAG, "Discovered peer: $remoteDeviceId at ${device.address} (RSSI: $rssi)")
                        
                        // Store peer
                        discoveredPeers[remoteDeviceId] = DiscoveredPeer(
                            remoteDeviceId,
                            device.address,
                            rssi,
                            System.currentTimeMillis()
                        )
                        
                        // Notify discovery
                        onPeerDiscovered(remoteDeviceId, device.address, rssi)
                        
                        // Keep connection for messaging
                        connectedClients[remoteDeviceId] = gatt
                    } else {
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
            }
        }
    }

    /**
     * Send message to a specific peer
     */
    @SuppressLint("MissingPermission")
    fun sendMessage(recipientId: String, messageData: ByteArray): Boolean {
        val gatt = connectedClients[recipientId]
        
        if (gatt == null) {
            android.util.Log.e(TAG, "No connection to peer: $recipientId")
            return false
        }

        val service = gatt.getService(SERVICE_UUID)
        val messageChar = service?.getCharacteristic(MESSAGE_CHAR_UUID)
        
        if (messageChar == null) {
            android.util.Log.e(TAG, "Message characteristic not found")
            return false
        }

        messageChar.value = messageData
        val success = gatt.writeCharacteristic(messageChar)
        
        android.util.Log.d(TAG, "Send message to $recipientId: $success")
        return success
    }

    /**
     * Get list of discovered peers
     */
    fun getDiscoveredPeers(): List<DiscoveredPeer> {
        return discoveredPeers.values.toList()
    }
}

