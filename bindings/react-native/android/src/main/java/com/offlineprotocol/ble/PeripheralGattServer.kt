package com.offlineprotocol.ble

import android.bluetooth.BluetoothDevice
import android.bluetooth.BluetoothGatt
import android.bluetooth.BluetoothGattCharacteristic
import android.bluetooth.BluetoothGattDescriptor
import android.bluetooth.BluetoothGattServer
import android.bluetooth.BluetoothGattServerCallback
import android.bluetooth.BluetoothGattService
import android.bluetooth.BluetoothManager
import android.bluetooth.BluetoothProfile
import android.bluetooth.BluetoothStatusCodes
import android.content.Context
import android.os.Build
import android.os.Handler
import android.util.Log
import java.util.UUID
import java.util.concurrent.ConcurrentHashMap

/**
 * Return the portion of [value] that should be sent in response to a GATT read
 * at [offset]. Android's long-read flow issues repeated
 * `onCharacteristicReadRequest` calls with increasing offset until the central
 * has reassembled the whole value; the app must return the tail starting at
 * that offset on each call. Exposed internal for unit testing.
 */
internal fun sliceForReadOffset(value: ByteArray, offset: Int): ByteArray = when {
    offset <= 0 -> value
    offset >= value.size -> ByteArray(0)
    else -> value.copyOfRange(offset, value.size)
}

/**
 * Owns the Android peripheral-side GATT server. Wraps the raw
 * [BluetoothGattServer] and enforces:
 *
 *   1. Every notify-propertied characteristic automatically gets a CCCD
 *      (0x2902) descriptor via [registerNotifyCharacteristic], so remote
 *      centrals can subscribe to notifications. The helper is the only
 *      sanctioned way to create notify characteristics on this server.
 *   2. [BluetoothGattServerCallback.onDescriptorWriteRequest] is implemented
 *      so CCCD writes from centrals are acked and a `subscribedCentrals` set
 *      is maintained.
 *   3. Service registration has a 5-second watchdog: if `onServiceAdded`
 *      never fires, we tear down, retry once, and escalate to
 *      [Listener.onSetupFailed] on a second failure — so a flaky GATT stack
 *      cannot silently leave the device invisible on the mesh.
 *   4. `onCharacteristicReadRequest` honours the read offset, so long-read
 *      flows (identity payloads that exceed the default 20-byte ATT MTU
 *      payload) return the correct tail on each follow-up read.
 *   5. `onCharacteristicWriteRequest` enforces a hard upper bound on inbound
 *      payload size as defence in depth against malformed/hostile centrals.
 */
class PeripheralGattServer(
    private val context: Context,
    private val mainHandler: Handler,
    private val listener: Listener,
    private val diagnosticEmitter: (level: String, message: String, context: Map<String, Any?>) -> Unit =
        { _, _, _ -> }
) {

    /** Callbacks surfaced to the orchestrating transport. */
    interface Listener {
        fun onCentralConnected(device: BluetoothDevice)
        fun onCentralDisconnected(device: BluetoothDevice, status: Int)
        fun onInboundFragment(device: BluetoothDevice, bytes: ByteArray)
        /** Return the current device-ID bytes, or null to fail the read. */
        fun provideDeviceIdBytes(device: BluetoothDevice): ByteArray?
        /** Return the current signed-identity bytes, or null to fail the read. */
        fun provideIdentityBytes(device: BluetoothDevice): ByteArray?
        fun onReady()
        fun onSetupFailed(reason: String)
    }

    companion object {
        private const val TAG = "PeripheralGattServer"
        val CCCD_UUID: UUID = UUID.fromString("00002902-0000-1000-8000-00805f9b34fb")
        private const val SERVICE_READY_TIMEOUT_MS = 5_000L
        private const val RETRY_DELAY_MS = 500L
        private const val MAX_SETUP_ATTEMPTS = 2
        /** Hard ceiling on an inbound GATT write payload. Well above the
         *  facade's MAX_FRAGMENT_SIZE (185 bytes) to tolerate MTU-negotiation
         *  headroom, but bounded so a hostile central cannot balloon memory. */
        const val MAX_INBOUND_WRITE_BYTES = 4096
    }

    private val bluetoothManager: BluetoothManager =
        context.getSystemService(Context.BLUETOOTH_SERVICE) as BluetoothManager

    private var gattServer: BluetoothGattServer? = null
    private var messageCharacteristic: BluetoothGattCharacteristic? = null
    private var deviceIdCharacteristic: BluetoothGattCharacteristic? = null
    private var identityCharacteristic: BluetoothGattCharacteristic? = null

    @Volatile
    var isReady: Boolean = false
        private set

    /** Centrals that have written ENABLE_NOTIFICATION_VALUE to the CCCD. */
    private val subscribedCentrals: MutableSet<BluetoothDevice> =
        ConcurrentHashMap.newKeySet()

    /**
     * Serializes pre-Tiramisu notification writes. On API < 33 the value
     * being notified is written to the shared [BluetoothGattCharacteristic]
     * field before dispatch, so two threads notifying different centrals
     * race on that field. Tiramisu+ uses the value-parameter overload and
     * bypasses this lock entirely.
     */
    private val notifyLock = Any()

    private var serviceReadyTimeoutRunnable: Runnable? = null
    private var setupRetryRunnable: Runnable? = null
    private var setupAttempts = 0
    private var pendingSetup: PendingSetup? = null

    private data class PendingSetup(
        val serviceUuid: UUID,
        val messageUuid: UUID,
        val deviceIdUuid: UUID,
        val identityUuid: UUID,
    )

    /**
     * Open the GATT server and register the mesh service. Idempotent: a
     * second call while already started tears down and re-attempts setup.
     */
    fun start(
        serviceUuid: UUID,
        messageUuid: UUID,
        deviceIdUuid: UUID,
        identityUuid: UUID,
    ) {
        stop()
        setupAttempts = 0
        pendingSetup = PendingSetup(serviceUuid, messageUuid, deviceIdUuid, identityUuid)
        attemptSetup()
    }

    /** Close the server and drop all cached state. Safe to call repeatedly. */
    fun stop() {
        serviceReadyTimeoutRunnable?.let { mainHandler.removeCallbacks(it) }
        serviceReadyTimeoutRunnable = null
        setupRetryRunnable?.let { mainHandler.removeCallbacks(it) }
        setupRetryRunnable = null
        try {
            gattServer?.close()
        } catch (e: SecurityException) {
            Log.w(TAG, "Permission denied while closing GATT server", e)
        }
        gattServer = null
        messageCharacteristic = null
        deviceIdCharacteristic = null
        identityCharacteristic = null
        subscribedCentrals.clear()
        isReady = false
        pendingSetup = null
        setupAttempts = 0
    }

    /**
     * Push a notification to a specific subscribed central. Returns true on
     * a successful platform-level submit. Silently returns false if the
     * central is not subscribed or the server is not set up.
     *
     * On API 33+ this uses the value-parameter overload, which hands the
     * payload to the platform atomically and is therefore safe to call
     * concurrently from multiple threads. On older APIs the payload is
     * assigned to the shared characteristic field first, so calls are
     * serialized under [notifyLock].
     */
    fun notifySubscribed(device: BluetoothDevice, bytes: ByteArray): Boolean {
        if (!subscribedCentrals.contains(device)) return false
        val char = messageCharacteristic ?: return false
        val server = gattServer ?: return false
        return try {
            if (Build.VERSION.SDK_INT >= Build.VERSION_CODES.TIRAMISU) {
                val status = server.notifyCharacteristicChanged(device, char, false, bytes)
                status == BluetoothStatusCodes.SUCCESS
            } else {
                synchronized(notifyLock) {
                    @Suppress("DEPRECATION")
                    char.value = bytes
                    @Suppress("DEPRECATION")
                    server.notifyCharacteristicChanged(device, char, false)
                }
            }
        } catch (e: SecurityException) {
            Log.w(TAG, "Permission denied while notifying ${device.address}", e)
            false
        }
    }

    /** Push a notification to every subscribed central. Returns the count of successful notifies. */
    fun notifyAllSubscribed(bytes: ByteArray): Int {
        var count = 0
        for (device in subscribedCentrals) {
            if (notifySubscribed(device, bytes)) count++
        }
        return count
    }

    fun subscribedCentralCount(): Int = subscribedCentrals.size

    /** Exposed for tests and diagnostics. */
    fun isSubscribed(device: BluetoothDevice): Boolean = subscribedCentrals.contains(device)

    /** Cancel an in-flight connection to a central. */
    fun cancelConnection(device: BluetoothDevice) {
        try {
            gattServer?.cancelConnection(device)
        } catch (e: SecurityException) {
            Log.w(TAG, "Permission denied cancelling connection to ${device.address}", e)
        }
    }

    // --- Internal setup ---

    private fun attemptSetup() {
        val setup = pendingSetup ?: return
        setupAttempts++
        isReady = false

        val server = try {
            bluetoothManager.openGattServer(context, gattServerCallback)
        } catch (e: SecurityException) {
            Log.e(TAG, "Permission denied opening GATT server", e)
            diagnosticEmitter(
                "error",
                "gatt_server_permission_denied",
                mapOf("exception" to e.javaClass.simpleName, "message" to (e.message ?: "unknown")),
            )
            listener.onSetupFailed("permission_denied: ${e.message}")
            return
        }
        if (server == null) {
            Log.e(TAG, "openGattServer returned null")
            handleSetupFailed("open_gatt_server_null")
            return
        }
        gattServer = server

        val message = registerNotifyCharacteristic(setup.messageUuid)
        val deviceIdChar = BluetoothGattCharacteristic(
            setup.deviceIdUuid,
            BluetoothGattCharacteristic.PROPERTY_READ,
            BluetoothGattCharacteristic.PERMISSION_READ,
        )
        val identityChar = BluetoothGattCharacteristic(
            setup.identityUuid,
            BluetoothGattCharacteristic.PROPERTY_READ,
            BluetoothGattCharacteristic.PERMISSION_READ,
        )
        messageCharacteristic = message
        deviceIdCharacteristic = deviceIdChar
        identityCharacteristic = identityChar

        val service = BluetoothGattService(
            setup.serviceUuid,
            BluetoothGattService.SERVICE_TYPE_PRIMARY,
        )
        service.addCharacteristic(message)
        service.addCharacteristic(deviceIdChar)
        service.addCharacteristic(identityChar)

        val added = try {
            server.addService(service)
        } catch (e: SecurityException) {
            Log.e(TAG, "Permission denied adding service", e)
            handleSetupFailed("permission_denied_add_service")
            return
        }
        if (!added) {
            handleSetupFailed("add_service_returned_false")
            return
        }

        armServiceReadyTimeout()
    }

    private fun armServiceReadyTimeout() {
        serviceReadyTimeoutRunnable?.let { mainHandler.removeCallbacks(it) }
        val runnable = Runnable {
            if (!isReady) {
                Log.w(TAG, "GATT service ready timeout (attempt=$setupAttempts)")
                diagnosticEmitter(
                    "warn",
                    "gatt_service_ready_timeout",
                    mapOf("attempt" to setupAttempts),
                )
                handleSetupFailed("service_ready_timeout")
            }
        }
        serviceReadyTimeoutRunnable = runnable
        mainHandler.postDelayed(runnable, SERVICE_READY_TIMEOUT_MS)
    }

    private fun handleSetupFailed(reason: String) {
        serviceReadyTimeoutRunnable?.let { mainHandler.removeCallbacks(it) }
        serviceReadyTimeoutRunnable = null
        setupRetryRunnable?.let { mainHandler.removeCallbacks(it) }
        setupRetryRunnable = null
        try {
            gattServer?.close()
        } catch (_: SecurityException) {
            // Best effort — we're tearing down anyway.
        }
        gattServer = null
        messageCharacteristic = null
        deviceIdCharacteristic = null
        identityCharacteristic = null
        isReady = false

        if (setupAttempts >= MAX_SETUP_ATTEMPTS) {
            diagnosticEmitter(
                "error",
                "gatt_service_ready_timeout_final",
                mapOf("attempts" to setupAttempts, "reason" to reason),
            )
            val setup = pendingSetup
            pendingSetup = null
            setupAttempts = 0
            if (setup != null) {
                listener.onSetupFailed(reason)
            }
            return
        }

        val retry = Runnable {
            setupRetryRunnable = null
            if (pendingSetup != null) {
                attemptSetup()
            }
        }
        setupRetryRunnable = retry
        mainHandler.postDelayed(retry, RETRY_DELAY_MS)
    }

    /**
     * The only sanctioned path for creating a notify-propertied
     * characteristic on this server. Attaches a CCCD (0x2902) descriptor
     * unconditionally so remote centrals can subscribe via the standard
     * GATT notification subscription flow.
     */
    private fun registerNotifyCharacteristic(uuid: UUID): BluetoothGattCharacteristic {
        val char = BluetoothGattCharacteristic(
            uuid,
            BluetoothGattCharacteristic.PROPERTY_WRITE_NO_RESPONSE or
                BluetoothGattCharacteristic.PROPERTY_NOTIFY,
            BluetoothGattCharacteristic.PERMISSION_WRITE,
        )
        val cccd = BluetoothGattDescriptor(
            CCCD_UUID,
            BluetoothGattDescriptor.PERMISSION_READ or BluetoothGattDescriptor.PERMISSION_WRITE,
        )
        char.addDescriptor(cccd)
        return char
    }

    // --- GattServerCallback ---

    private val gattServerCallback = object : BluetoothGattServerCallback() {
        override fun onServiceAdded(status: Int, service: BluetoothGattService?) {
            if (status == BluetoothGatt.GATT_SUCCESS) {
                Log.i(TAG, "GATT service added: ${service?.uuid}")
                diagnosticEmitter(
                    "info",
                    "gatt_service_registered",
                    mapOf("serviceUUID" to (service?.uuid?.toString() ?: "unknown")),
                )
                isReady = true
                serviceReadyTimeoutRunnable?.let { mainHandler.removeCallbacks(it) }
                serviceReadyTimeoutRunnable = null
                setupAttempts = 0
                listener.onReady()
            } else {
                Log.e(TAG, "GATT service add failed: status=$status")
                diagnosticEmitter(
                    "error",
                    "gatt_service_add_failed",
                    mapOf(
                        "status" to status,
                        "serviceUUID" to (service?.uuid?.toString() ?: "unknown"),
                    ),
                )
                isReady = false
                // Kick retry/escalation immediately rather than waiting out
                // the 5s watchdog — we already know the registration failed,
                // so there's no point stalling app startup.
                handleSetupFailed("service_add_status=$status")
            }
        }

        override fun onConnectionStateChange(device: BluetoothDevice, status: Int, newState: Int) {
            when (newState) {
                BluetoothProfile.STATE_CONNECTED -> listener.onCentralConnected(device)
                BluetoothProfile.STATE_DISCONNECTED -> {
                    subscribedCentrals.remove(device)
                    listener.onCentralDisconnected(device, status)
                }
            }
        }

        override fun onCharacteristicReadRequest(
            device: BluetoothDevice,
            requestId: Int,
            offset: Int,
            characteristic: BluetoothGattCharacteristic,
        ) {
            val server = gattServer ?: return
            try {
                val value: ByteArray? = when (characteristic.uuid) {
                    deviceIdCharacteristic?.uuid -> listener.provideDeviceIdBytes(device)
                    identityCharacteristic?.uuid -> listener.provideIdentityBytes(device)
                    else -> null
                }
                if (value != null) {
                    // Honour offset for long reads: the central will issue
                    // repeated reads with increasing offsets until it has
                    // reassembled a payload larger than the ATT MTU.
                    val slice = sliceForReadOffset(value, offset)
                    server.sendResponse(device, requestId, BluetoothGatt.GATT_SUCCESS, offset, slice)
                } else {
                    server.sendResponse(device, requestId, BluetoothGatt.GATT_FAILURE, offset, null)
                }
            } catch (e: SecurityException) {
                Log.w(TAG, "Permission denied in read request", e)
            }
        }

        override fun onCharacteristicWriteRequest(
            device: BluetoothDevice,
            requestId: Int,
            characteristic: BluetoothGattCharacteristic,
            preparedWrite: Boolean,
            responseNeeded: Boolean,
            offset: Int,
            value: ByteArray,
        ) {
            val server = gattServer ?: return
            try {
                if (characteristic.uuid == messageCharacteristic?.uuid) {
                    // Defence in depth: drop oversized writes before handing
                    // the payload to the upstream fragment assembler.
                    if (value.size > MAX_INBOUND_WRITE_BYTES) {
                        diagnosticEmitter(
                            "warn",
                            "gatt_inbound_write_oversize",
                            mapOf(
                                "address" to device.address,
                                "size" to value.size,
                                "max" to MAX_INBOUND_WRITE_BYTES,
                            ),
                        )
                        if (responseNeeded) {
                            server.sendResponse(
                                device, requestId, BluetoothGatt.GATT_FAILURE, offset, null,
                            )
                        }
                        return
                    }
                    listener.onInboundFragment(device, value)
                    if (responseNeeded) {
                        // Response value is semantically ignored by centrals;
                        // don't waste ATT bandwidth echoing the payload.
                        server.sendResponse(
                            device, requestId, BluetoothGatt.GATT_SUCCESS, offset, null,
                        )
                    }
                } else {
                    if (responseNeeded) {
                        server.sendResponse(
                            device, requestId, BluetoothGatt.GATT_FAILURE, offset, null,
                        )
                    }
                }
            } catch (e: SecurityException) {
                Log.w(TAG, "Permission denied in write request", e)
            }
        }

        override fun onDescriptorWriteRequest(
            device: BluetoothDevice,
            requestId: Int,
            descriptor: BluetoothGattDescriptor,
            preparedWrite: Boolean,
            responseNeeded: Boolean,
            offset: Int,
            value: ByteArray,
        ) {
            val server = gattServer ?: return
            try {
                if (descriptor.uuid == CCCD_UUID) {
                    val enable =
                        value.contentEquals(BluetoothGattDescriptor.ENABLE_NOTIFICATION_VALUE) ||
                            value.contentEquals(BluetoothGattDescriptor.ENABLE_INDICATION_VALUE)
                    if (enable) {
                        subscribedCentrals.add(device)
                        diagnosticEmitter(
                            "info",
                            "cccd_subscribe_received",
                            mapOf(
                                "address" to device.address,
                                "subscribedCount" to subscribedCentrals.size,
                            ),
                        )
                    } else {
                        subscribedCentrals.remove(device)
                        diagnosticEmitter(
                            "info",
                            "cccd_unsubscribe_received",
                            mapOf(
                                "address" to device.address,
                                "subscribedCount" to subscribedCentrals.size,
                            ),
                        )
                    }
                    if (responseNeeded) {
                        server.sendResponse(
                            device, requestId, BluetoothGatt.GATT_SUCCESS, offset, null,
                        )
                    }
                } else {
                    if (responseNeeded) {
                        server.sendResponse(
                            device, requestId, BluetoothGatt.GATT_FAILURE, offset, null,
                        )
                    }
                }
            } catch (e: SecurityException) {
                Log.w(TAG, "Permission denied in descriptor write request", e)
            }
        }
    }
}
