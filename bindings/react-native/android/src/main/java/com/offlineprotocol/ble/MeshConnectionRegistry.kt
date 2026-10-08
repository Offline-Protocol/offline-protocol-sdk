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
    // Client links whose connect completed. [gattClients] also holds dials
    // still in flight, and a dial to a peer whose Bluetooth is off lasts as
    // long as the stack's connect timeout, so it cannot count as a link.
    private val establishedClients = Collections.newSetFromMap(ConcurrentHashMap<String, Boolean>())
    /**
     * Addresses refused as a second client link to a peer we already hold a
     * client link to, with the elapsed-realtime moment the refusal lapses.
     * A peripheral that rotates its address (macOS does, on its own clock)
     * advertises under both for a while, and without this the scanner would
     * redial the second address on every advertisement, verify it, and be
     * refused again, each round holding a connection slot.
     */
    private val duplicateUntil = ConcurrentHashMap<String, Long>()

    fun registerGatt(address: String, gatt: BluetoothGatt) {
        gattClients[address] = gatt
    }

    fun getGatt(address: String): BluetoothGatt? = gattClients[address]

    fun removeGatt(address: String): BluetoothGatt? {
        establishedClients.remove(address)
        return gattClients.remove(address)
    }

    /** Records that the client link to [address] finished connecting. */
    fun markClientEstablished(address: String) {
        if (gattClients.containsKey(address)) establishedClients.add(address)
    }

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
            // Only if it still points here: a peer that came back from a new
            // address (iOS rotates its random address across a Bluetooth
            // power-cycle) has already re-pointed it to the live link.
            if (deviceToAddress.remove(deviceId, address)) {
                // A peer can hold one link per direction at two addresses. When
                // the one dropped here was the last to resolve, the other still
                // maps to the peer and becomes its address, or the peer would
                // have none while a link to it is up.
                addressToDevice.entries
                    .firstOrNull { it.value == deviceId }
                    ?.let { deviceToAddress.putIfAbsent(deviceId, it.key) }
            }
        }
    }

    /**
     * True when [deviceId] is reachable over a live link at an address other
     * than [excluding].
     *
     * Walks every address mapped to the peer, like [hasEstablishedLink]:
     * [deviceToAddress] keeps only the address that resolved last, and a hello
     * makes that the peer's central-role address. Read through it, a client
     * link giving up at that address would miss the peer's live link at its
     * other address and report a live peer lost.
     */
    fun hasOtherLiveLink(deviceId: String, excluding: String): Boolean =
        addressToDevice.any { (address, id) ->
            id == deviceId && address != excluding &&
                (gattClients.containsKey(address) || serverConnections.contains(address))
        }

    /**
     * True when some link to [deviceId] is up: a central that connected to our
     * server, or a client link of ours whose connect completed, at ANY address
     * mapped to the peer. A dial still in flight does not count.
     *
     * Like [hasOtherLiveLink] this walks every address, because
     * [deviceToAddress] keeps only the address that resolved last, and a peer
     * can hold links from two addresses at once (one per direction).
     */
    fun hasEstablishedLink(deviceId: String): Boolean =
        addressToDevice.any { (address, id) ->
            id == deviceId && (establishedClients.contains(address) || serverConnections.contains(address))
        }

    /**
     * True when we hold a client link to [deviceId] at an address other than
     * [excluding]. Server links do not count: a peer may hold one link per
     * direction, and only a second link in the SAME direction is a duplicate.
     */
    fun hasOtherClientLink(deviceId: String, excluding: String): Boolean =
        addressToDevice.any { (address, id) ->
            id == deviceId && address != excluding && gattClients.containsKey(address)
        }

    /** Refuse client links to [address] until [untilMs] (elapsed realtime). */
    fun suppressDuplicate(address: String, untilMs: Long) {
        duplicateUntil[address] = untilMs
    }

    /** Whether a dial to [address] is still inside a duplicate refusal. */
    fun isSuppressedDuplicate(address: String, nowMs: Long): Boolean {
        val until = duplicateUntil[address] ?: return false
        if (nowMs < until) return true
        duplicateUntil.remove(address, until)
        return false
    }

    /** Every address currently mapped to [deviceId]. */
    fun addressesForDevice(deviceId: String): List<String> =
        addressToDevice.filterValues { it == deviceId }.keys.toList()

    fun hasDeviceForAddress(address: String): Boolean = addressToDevice.containsKey(address)

    /** Every peer identified on some link, once each. */
    fun deviceIds(): Set<String> = addressToDevice.values.toSet()

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

    fun hasServerConnection(address: String): Boolean = serverConnections.contains(address)

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
        establishedClients.clear()
        duplicateUntil.clear()
    }
}
