package com.offlineprotocol.ble

import android.bluetooth.BluetoothGatt
import com.offlineprotocol.mesh.MeshController.MeshRole
import java.util.Collections
import java.util.concurrent.ConcurrentHashMap

/**
 * Centralised registry for client and server-side BLE connections together with
 * auxiliary metadata (desired roles, resolved identifiers).
 *
 * Extracted from [BleTransportFacade] to keep the facade focused on
 * orchestration.
 *
 * All connection-indexed state is keyed by the peer's BLE address string as
 * observed at the time of that specific connection. For the GATT-server
 * (peripheral) side that is the remote central's address from
 * `BluetoothGattServerCallback`; for the GATT-client (central) side it's the
 * address we passed into `connectGatt`. Both are stable for the lifetime of
 * a single LL connection, which is the only window these maps need to cover.
 */
class MeshConnectionRegistry {
    private val gattClients = ConcurrentHashMap<String, BluetoothGatt>()
    private val addressToDevice = ConcurrentHashMap<String, String>()
    private val deviceToAddress = ConcurrentHashMap<String, String>()
    private val pendingRoles = ConcurrentHashMap<String, MeshRole>()
    private val connectionRoles = ConcurrentHashMap<String, MeshRole>()
    private val serverConnections = Collections.newSetFromMap(ConcurrentHashMap<String, Boolean>())

    fun registerGatt(address: String, gatt: BluetoothGatt) {
        gattClients[address] = gatt
    }

    fun getGatt(address: String): BluetoothGatt? = gattClients[address]

    fun removeGatt(address: String): BluetoothGatt? = gattClients.remove(address)

    fun forEachGatt(action: (BluetoothGatt) -> Unit) {
        gattClients.values.forEach(action)
    }

    fun setDeviceIdentifier(address: String, deviceId: String) {
        addressToDevice[address] = deviceId
        deviceToAddress[deviceId] = address
    }

    fun deviceIdForAddress(address: String): String? = addressToDevice[address]

    fun addressForDevice(deviceId: String): String? = deviceToAddress[deviceId]

    fun removeIdentifiersForAddress(address: String) {
        val deviceId = addressToDevice.remove(address)
        if (deviceId != null) {
            deviceToAddress.remove(deviceId)
        }
    }

    fun removeIdentifiersForDevice(deviceId: String) {
        val address = deviceToAddress.remove(deviceId)
        if (address != null) {
            addressToDevice.remove(address)
        }
    }

    fun hasDeviceForAddress(address: String): Boolean = addressToDevice.containsKey(address)

    fun discoveredPeerCount(): Int = addressToDevice.size

    fun setPendingRole(address: String, role: MeshRole) {
        pendingRoles[address] = role
    }

    fun consumePendingRole(address: String): MeshRole? = pendingRoles.remove(address)

    fun clearPendingRoles() {
        pendingRoles.clear()
    }

    fun setConnectionRole(deviceId: String, role: MeshRole) {
        connectionRoles[deviceId] = role
    }

    fun removeConnectionRole(deviceId: String) {
        connectionRoles.remove(deviceId)
    }

    fun connectionRoleEntries(): List<Map.Entry<String, MeshRole>> = connectionRoles.entries.toList()

    fun trackServerConnection(address: String) {
        serverConnections.add(address)
    }

    fun untrackServerConnection(address: String) {
        serverConnections.remove(address)
    }

    fun serverConnectionCount(): Int = serverConnections.size

    fun connectionCount(): Int = gattClients.size + serverConnections.size

    fun clear() {
        gattClients.clear()
        addressToDevice.clear()
        deviceToAddress.clear()
        pendingRoles.clear()
        connectionRoles.clear()
        serverConnections.clear()
    }
}
