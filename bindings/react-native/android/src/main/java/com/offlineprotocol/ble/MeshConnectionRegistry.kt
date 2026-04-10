package com.offlineprotocol.ble

import android.bluetooth.BluetoothDevice
import android.bluetooth.BluetoothGatt
import com.offlineprotocol.mesh.MeshController.MeshRole
import java.util.Collections
import java.util.concurrent.ConcurrentHashMap

/**
 * Centralised registry for client and server-side BLE connections together with
 * auxiliary metadata (desired roles, resolved identifiers).
 *
 * Extracted from BleManager to keep the manager focused on orchestration.
 */
class MeshConnectionRegistry {
    private val gattClients = ConcurrentHashMap<String, BluetoothGatt>()
    private val addressToDevice = ConcurrentHashMap<String, String>()
    private val deviceToAddress = ConcurrentHashMap<String, String>()
    private val pendingRoles = ConcurrentHashMap<String, MeshRole>()
    private val connectionRoles = ConcurrentHashMap<String, MeshRole>()
    private val serverConnections = Collections.newSetFromMap(ConcurrentHashMap<String, Boolean>())

    /**
     * Maps live inbound `BluetoothDevice` handles (server-side connections from
     * remote centrals) to stable peer device IDs. This is RPA-safe: the handle
     * reference is stable for the lifetime of a single connection even when the
     * peer's advertised MAC rotates (iOS Random Resolvable Private Addresses).
     * Keying pending inbound fragments by the handle avoids the lookup miss
     * that the old `addressToDevice` path hit for iOS centrals.
     */
    private val handleToStableId = ConcurrentHashMap<BluetoothDevice, String>()

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

    fun trackServerConnection(deviceId: String) {
        serverConnections.add(deviceId)
    }

    fun untrackServerConnection(deviceId: String) {
        serverConnections.remove(deviceId)
    }

    fun connectionCount(): Int = gattClients.size + serverConnections.size

    // Server-connection handle tracking (RPA-safe for iOS centrals). See the
    // field-level comment on [handleToStableId] for rationale.

    fun setServerHandleIdentity(handle: BluetoothDevice, stableId: String) {
        handleToStableId[handle] = stableId
    }

    fun serverHandleIdentity(handle: BluetoothDevice): String? = handleToStableId[handle]

    fun removeServerHandle(handle: BluetoothDevice) {
        handleToStableId.remove(handle)
    }

    /**
     * Returns all server-side handles whose current address matches [address].
     * Used to drain the handle-keyed pending fragment queue once the reverse
     * identity resolution completes (the client-side code that learns the
     * stable ID keys its lookup by address, not by handle).
     */
    fun handlesForAddress(address: String): List<BluetoothDevice> =
        handleToStableId.keys.filter { it.address == address }

    fun clear() {
        gattClients.clear()
        addressToDevice.clear()
        deviceToAddress.clear()
        pendingRoles.clear()
        connectionRoles.clear()
        serverConnections.clear()
        handleToStableId.clear()
    }
}


