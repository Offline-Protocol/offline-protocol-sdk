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
     * client link to, with that peer and the elapsed-realtime moment the
     * refusal lapses.
     * A peripheral that rotates its address (macOS does, on its own clock)
     * advertises under both for a while, and without this the scanner would
     * redial the second address on every advertisement, verify it, and be
     * refused again, each round holding a connection slot.
     */
    private val duplicateUntil = ConcurrentHashMap<String, Pair<String, Long>>()

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
     * True when we hold a usable link to [deviceId] at an address other than
     * [excluding]: a client link of ours, or a central connected to our server
     * and subscribed to it ([isSubscribed]), which is the path the peer's
     * writes and our notifications already take.
     *
     * A client link beside such a server link is a second Link Layer
     * connection between the same two radios, and two devices hold one
     * (Core 5.4, Vol 6 Part B, 1.1.1). It forms only when one side's address
     * has rotated, and once it exists neither side can reconnect after the
     * other link drops: every dial is from an address the peer already has a
     * link with. In run 5 of the 2026-10-19 volume test that kept the phones
     * apart for ten minutes.
     *
     * A server link is seen only through [addressToDevice], which the hello
     * handler sets for a central's address. A subscribed central that writes
     * no hello (a Mac, a leaf, a service without it) is invisible here, so the
     * second client link beside it is kept (ADR 0030 names that case). A dial
     * still in flight counts as the kept link: [gattClients] holds it from
     * registration, and the duplicate is the one that verified second.
     */
    fun hasOtherUsableLink(
        deviceId: String,
        excluding: String,
        isSubscribed: (String) -> Boolean,
    ): Boolean =
        addressToDevice.any { (address, id) ->
            id == deviceId && address != excluding &&
                (gattClients.containsKey(address) ||
                    (serverConnections.contains(address) && isSubscribed(address)))
        }

    /**
     * Refuse client links to [address], a duplicate of [deviceId], until
     * [untilMs] (elapsed realtime).
     */
    fun suppressDuplicate(address: String, deviceId: String, untilMs: Long) {
        duplicateUntil[address] = deviceId to untilMs
    }

    /**
     * Whether a dial to [address] is still inside a duplicate refusal.
     *
     * Lapses early once the link it duplicated is gone: otherwise a
     * peer whose kept link died just after the refusal stayed undialed at the
     * address it still advertises for the rest of the window.
     */
    fun isSuppressedDuplicate(
        address: String,
        nowMs: Long,
        isSubscribed: (String) -> Boolean = { false },
    ): Boolean {
        val entry = duplicateUntil[address] ?: return false
        val (deviceId, until) = entry
        if (nowMs < until && hasOtherUsableLink(deviceId, address, isSubscribed)) return true
        duplicateUntil.remove(address, entry)
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

    /**
     * Maps [address] to [deviceId], as a hello would, when a central at it has
     * written to our server ([writerPending]: fragments wait there for a
     * sender) and nothing maps it yet. Returns whether it did. A mapped
     * address is never rebound.
     *
     * For a dial that verified the peer at an address it also writes to our
     * server from: an iPhone whose app restarted keeps that link (state
     * restoration) and never writes its hello again, so the dial is the only
     * proof of who is writing, even when the client link itself is then
     * refused as a duplicate. A server connection alone is no proof: Android
     * reports our own dial to the server too, so every refused dial has one,
     * and binding on it repointed the peer at an address being closed.
     */
    fun bindUnmappedServerLink(address: String, deviceId: String, writerPending: Boolean): Boolean {
        if (!writerPending || !serverConnections.contains(address) || addressToDevice.containsKey(address)) return false
        setDeviceIdentifier(address, deviceId)
        return true
    }

    fun untrackServerConnection(address: String) {
        serverConnections.remove(address)
    }

    fun serverConnectionCount(): Int = serverConnections.size

    /** True when the client link to [address] finished connecting. */
    fun isClientEstablished(address: String): Boolean = establishedClients.contains(address)

    /**
     * Peer addresses holding a link in either direction or a dial still in
     * flight, each counted once.
     *
     * Once, because Android reports a client link of ours to the GATT server
     * as well: the dial to a peer arrives there as a central connecting. A
     * sum of the two maps counted that one link twice, so two dials filled a
     * cap of four, and the cap check then refused the server-side half of our
     * own link, cancelling the connection the dial was waiting on.
     */
    fun connectionCount(): Int = (gattClients.keys + serverConnections).size

    /**
     * Peer addresses holding a completed link in either direction, each
     * counted once. A dial still in flight does not count: it may never
     * complete, and a peer connecting to us must not be refused for it.
     */
    fun establishedLinkCount(): Int = (establishedClients + serverConnections).size

    /**
     * [establishedLinkCount] restricted to links whose peer has proved who it
     * is (an address mapped to a device id). An unverified inbound central, or
     * the server's echo of a dial still being verified, may be closed in a
     * moment; the forced refresh must not keep the advertisement for it.
     */
    fun identifiedLinkCount(): Int =
        (establishedClients + serverConnections).count { addressToDevice.containsKey(it) }

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
