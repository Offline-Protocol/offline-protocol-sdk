package com.offlineprotocol.ble.nordic

import android.bluetooth.BluetoothDevice
import android.bluetooth.BluetoothGatt
import android.bluetooth.BluetoothGattCharacteristic
import android.content.Context
import android.util.Log
import no.nordicsemi.android.ble.BleManager as NordicBleManagerBase
import no.nordicsemi.android.ble.data.Data
import java.util.UUID

/**
 * Per-peer central-role client that extends Nordic's `BleManager` base class.
 *
 * One instance per remote peer. What Nordic gives us over the hand-rolled
 * `gattClientCallback` in the legacy BleManager:
 *
 *   - Op queue serialization — the library enqueues reads/writes/MTU/enable
 *     notifications and drains them one at a time with per-request callbacks,
 *     eliminating the class of bugs where a second BLE op clobbers an
 *     in-flight first op.
 *
 *   - Automatic CCCD writes on `enableNotifications(char)`. The library
 *     writes 0x2902 ENABLE_NOTIFICATION_VALUE to the descriptor for us
 *     instead of requiring a separate manual `writeDescriptor` after
 *     `setCharacteristicNotification` (which the legacy path missed and
 *     had to be fixed with a targeted commit earlier in this branch).
 *
 *   - MTU negotiation and notification subscription in a single `initialize`
 *     flow with explicit ordering via `.enqueue()`.
 *
 *   - `connect(...).retry(count, delayMs)` for automatic GATT 133 retry,
 *     which the legacy path rolled by hand with a reconnect scheduler.
 *
 * Note on wiring: this class exists as the migration target for the
 * central-role path. The legacy `gattClientCallback` in BleManager is
 * still the production code path until a follow-up commit swaps call
 * sites over to `NordicPeerClient` and validates on-device. Keeping the
 * two paths separated in this commit lets the structural change land
 * in isolation while the legacy path keeps working.
 */
class NordicPeerClient(
    context: Context,
    private val serviceUuid: UUID,
    private val messageCharUuid: UUID,
    private val deviceIdCharUuid: UUID,
    private val identityCharUuid: UUID,
    private val listener: Listener,
) : NordicBleManagerBase(context) {

    /** Callbacks surfaced back to the orchestrating transport layer. */
    interface Listener {
        /** Stable device-id bytes read from the peer's DEVICE_ID characteristic. */
        fun onDeviceIdRead(client: NordicPeerClient, deviceIdBytes: ByteArray)

        /** Signed identity bytes read from the peer's IDENTITY characteristic. */
        fun onIdentityRead(client: NordicPeerClient, identityBytes: ByteArray)

        /** Message fragment received via the NOTIFY subscription on the peer. */
        fun onFragmentReceived(client: NordicPeerClient, bytes: ByteArray)

        /** Forwarded diagnostic telemetry. */
        fun onDiagnostic(level: String, message: String, ctx: Map<String, Any?>)
    }

    companion object {
        private const val TAG = "NordicPeerClient"
        private const val DESIRED_MTU = 517
    }

    private var messageCharacteristic: BluetoothGattCharacteristic? = null
    private var deviceIdCharacteristic: BluetoothGattCharacteristic? = null
    private var identityCharacteristic: BluetoothGattCharacteristic? = null

    /**
     * Derived peer user id. The orchestrating layer populates this after
     * verifying the signed identity bytes returned via [Listener.onIdentityRead].
     */
    @Volatile
    var peerUserId: String? = null

    override fun log(priority: Int, message: String) {
        Log.println(priority, TAG, message)
    }

    override fun getMinLogPriority(): Int = Log.DEBUG

    override fun getGattCallback(): BleManagerGattCallback = PeerGattCallback()

    /**
     * Write an outbound fragment via WRITE_TYPE_NO_RESPONSE. Nordic's
     * `.split()` handles chunking when the payload exceeds the negotiated
     * MTU; we keep an additional hard cap elsewhere as a defence in depth.
     */
    fun sendFragment(bytes: ByteArray) {
        val char = messageCharacteristic ?: return
        writeCharacteristic(
            char,
            bytes,
            BluetoothGattCharacteristic.WRITE_TYPE_NO_RESPONSE,
        )
            .split()
            .fail { device, status ->
                listener.onDiagnostic(
                    "warn",
                    "nordic_write_failed",
                    mapOf("address" to device.address, "status" to status),
                )
            }
            .enqueue()
    }

    private inner class PeerGattCallback : BleManagerGattCallback() {
        override fun isRequiredServiceSupported(gatt: BluetoothGatt): Boolean {
            val service = gatt.getService(serviceUuid) ?: return false
            messageCharacteristic = service.getCharacteristic(messageCharUuid)
            deviceIdCharacteristic = service.getCharacteristic(deviceIdCharUuid)
            identityCharacteristic = service.getCharacteristic(identityCharUuid)
            return messageCharacteristic != null &&
                deviceIdCharacteristic != null &&
                identityCharacteristic != null
        }

        override fun initialize() {
            // Request a larger MTU first so subsequent reads/writes see the
            // negotiated size rather than the default 23 bytes.
            requestMtu(DESIRED_MTU)
                .fail { device, status ->
                    listener.onDiagnostic(
                        "warn",
                        "nordic_request_mtu_failed",
                        mapOf("address" to device.address, "status" to status),
                    )
                }
                .enqueue()

            // Stream inbound notifications directly to the transport.
            setNotificationCallback(messageCharacteristic)
                .with { _: BluetoothDevice, data: Data ->
                    val bytes = data.value
                    if (bytes != null) {
                        listener.onFragmentReceived(this@NordicPeerClient, bytes)
                    }
                }

            // Subscribe to notifications. Nordic writes the CCCD 0x2902
            // descriptor as part of this request — no manual writeDescriptor
            // needed.
            enableNotifications(messageCharacteristic)
                .fail { device, status ->
                    listener.onDiagnostic(
                        "warn",
                        "nordic_enable_notifications_failed",
                        mapOf("address" to device.address, "status" to status),
                    )
                }
                .enqueue()

            // Pull the stable device-id bytes and the signed identity bytes.
            // The transport layer will key its routing tables off these.
            readCharacteristic(deviceIdCharacteristic)
                .with { _: BluetoothDevice, data: Data ->
                    val bytes = data.value
                    if (bytes != null) {
                        listener.onDeviceIdRead(this@NordicPeerClient, bytes)
                    }
                }
                .enqueue()

            readCharacteristic(identityCharacteristic)
                .with { _: BluetoothDevice, data: Data ->
                    val bytes = data.value
                    if (bytes != null) {
                        listener.onIdentityRead(this@NordicPeerClient, bytes)
                    }
                }
                .enqueue()
        }

        override fun onServicesInvalidated() {
            messageCharacteristic = null
            deviceIdCharacteristic = null
            identityCharacteristic = null
        }
    }
}
