package com.offlineprotocol.ble.nordic

import android.content.Context
import com.offlineprotocol.BleManager as LegacyBleManager
import com.offlineprotocol.TransportManager
import com.offlineprotocol.TransportManagerListener
import com.offlineprotocol.TransportState
import uniffi.offline_protocol.OfflineProtocol

/**
 * Transport-level facade for the Android BLE implementation.
 *
 * Exists as the stable public name for the BLE transport that
 * OfflineProtocolModule constructs. All behaviour currently delegates to
 * the legacy [com.offlineprotocol.BleManager] while the migration off
 * raw BluetoothGatt/BluetoothGattServer proceeds piecewise:
 *
 *   - NordicGattServer already owns the peripheral-side GATT server with
 *     CCCD + onDescriptorWriteRequest handling.
 *   - NordicAdvertiser already owns the BluetoothLeAdvertiser lifecycle
 *     and restart scheduling.
 *   - NordicPeerClient is the migration target for the central-role path
 *     but is not yet wired in.
 *
 * Once the remaining orchestration (scanning, client callback, mesh
 * controller glue, fragment accounting) migrates out of the legacy class,
 * this facade becomes the sole owner of those pieces and the legacy
 * BleManager is deleted. Until then, every call goes through the
 * delegate so behaviour is unchanged.
 */
class BleTransportFacade(
    context: Context,
    protocol: OfflineProtocol,
    deviceId: String,
    diagnosticEmitter: ((String, String, Map<String, Any?>) -> Unit)? = null,
) : TransportManager {

    private val delegate: LegacyBleManager = LegacyBleManager(
        context,
        protocol,
        deviceId,
        diagnosticEmitter,
    )

    override val transportId: String get() = delegate.transportId
    override val transportName: String get() = delegate.transportName
    override val state: TransportState get() = delegate.state

    override var listener: TransportManagerListener?
        get() = delegate.listener
        set(value) {
            delegate.listener = value
        }

    override fun isAvailable(): Boolean = delegate.isAvailable()
    override fun start() = delegate.start()
    override fun stop() = delegate.stop()
    override fun pause() = delegate.pause()
    override fun resume() = delegate.resume()
    override fun getMetrics(): Map<String, Any> = delegate.getMetrics()

    /**
     * Event-driven send trigger invoked by the UniFFI
     * `BleTransportCallback.on_fragments_available` hook from the Rust core.
     * Forwarded to the legacy orchestrator; once the migration is complete
     * this class will drain fragments itself.
     */
    fun onFragmentsAvailable() = delegate.onFragmentsAvailable()
}
