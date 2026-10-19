//
// MeshConnectionRegistry.swift
// OfflineProtocol
//

import Foundation
import CoreBluetooth

/// Thread-safe registry tracking active BLE connections, device identifiers, and role assignments.
final class MeshConnectionRegistry: @unchecked Sendable {
    private let lock = NSLock()
    private var connectedPeripherals: [UUID: CBPeripheral] = [:]
    private var peripheralDeviceIds: [UUID: String] = [:]
    private var centralDeviceIds: [UUID: String] = [:]
    private var pendingRoles: [UUID: MeshController.MeshRole] = [:]
    private var connectionRoles: [String: MeshController.MeshRole] = [:]

    // MARK: - Peripheral Tracking

    func registerPeripheral(_ peripheral: CBPeripheral) {
        lock.lock()
        connectedPeripherals[peripheral.identifier] = peripheral
        lock.unlock()
    }

    func connectedPeripheral(for identifier: UUID) -> CBPeripheral? {
        lock.lock()
        let peripheral = connectedPeripherals[identifier]
        lock.unlock()
        return peripheral
    }

    @discardableResult
    func removePeripheral(_ identifier: UUID) -> CBPeripheral? {
        lock.lock()
        let removed = connectedPeripherals.removeValue(forKey: identifier)
        lock.unlock()
        return removed
    }

    func allPeripherals() -> [CBPeripheral] {
        lock.lock()
        let peripherals = Array(connectedPeripherals.values)
        lock.unlock()
        return peripherals
    }

    func connectedPeripheralCount() -> Int {
        lock.lock()
        let count = connectedPeripherals.count
        lock.unlock()
        return count
    }

    // MARK: - Device ID Mapping

    func setPeripheralDeviceId(_ deviceId: String, for identifier: UUID) {
        lock.lock()
        peripheralDeviceIds[identifier] = deviceId
        lock.unlock()
    }

    func peripheralDeviceId(for identifier: UUID) -> String? {
        lock.lock()
        let value = peripheralDeviceIds[identifier]
        lock.unlock()
        return value
    }

    @discardableResult
    func removePeripheralDeviceId(for identifier: UUID) -> String? {
        lock.lock()
        let removed = peripheralDeviceIds.removeValue(forKey: identifier)
        lock.unlock()
        return removed
    }

    /// The identifier to send `deviceId`'s fragments on. A peer that rotated
    /// its address maps from more than one, and the connected one must win:
    /// see `PeerLinkResolution`.
    func peripheralIdentifier(for deviceId: String) -> UUID? {
        lock.lock()
        let connected = connectedPeripherals
        let match = PeerLinkResolution.preferredIdentifier(
            for: deviceId,
            peripheralDeviceIds: peripheralDeviceIds,
            centralDeviceIds: centralDeviceIds,
            isConnected: { connected[$0] != nil }
        )
        lock.unlock()
        return match
    }

    /// Unmaps every other disconnected peripheral identifier of `deviceId`,
    /// now verified on `verified`, and returns them so the caller can stop
    /// dialling them. Central-role mappings are left alone: they key inbound
    /// fragments from a link the peer opened, which this says nothing about.
    func retirePeripheralIdentifiers(supersededBy verified: UUID, deviceId: String) -> [UUID] {
        lock.lock()
        let connected = connectedPeripherals
        let retired = PeerLinkResolution.superseded(
            by: verified,
            deviceId: deviceId,
            peripheralDeviceIds: peripheralDeviceIds,
            isConnected: { connected[$0] != nil }
        )
        for identifier in retired {
            peripheralDeviceIds.removeValue(forKey: identifier)
        }
        lock.unlock()
        return retired
    }

    /// The other identifiers `deviceId` holds a usable link on: see
    /// `PeerLinkResolution.otherLiveLinks`. `isSubscribed` answers for the
    /// centrals subscribed to our server, which this registry does not track.
    func otherLiveLinks(of deviceId: String, excluding: UUID, isSubscribed: (UUID) -> Bool) -> [UUID] {
        lock.lock()
        let connected = connectedPeripherals
        let links = PeerLinkResolution.otherLiveLinks(
            of: deviceId,
            excluding: excluding,
            peripheralDeviceIds: peripheralDeviceIds,
            centralDeviceIds: centralDeviceIds,
            isConnected: { connected[$0] != nil },
            isSubscribed: isSubscribed
        )
        lock.unlock()
        return links
    }

    func setCentralDeviceId(_ deviceId: String, for identifier: UUID) {
        lock.lock()
        centralDeviceIds[identifier] = deviceId
        lock.unlock()
    }

    func centralDeviceId(for identifier: UUID) -> String? {
        lock.lock()
        let value = centralDeviceIds[identifier]
        lock.unlock()
        return value
    }

    @discardableResult
    func removeCentralDeviceId(for identifier: UUID) -> String? {
        lock.lock()
        let removed = centralDeviceIds.removeValue(forKey: identifier)
        lock.unlock()
        return removed
    }

    func hasAnyDevice(for identifier: UUID) -> Bool {
        lock.lock()
        let has = peripheralDeviceIds[identifier] != nil || centralDeviceIds[identifier] != nil
        lock.unlock()
        return has
    }

    func allPeripheralDeviceIds() -> [String] {
        lock.lock()
        let ids = Array(peripheralDeviceIds.values)
        lock.unlock()
        return ids
    }

    func discoveredPeerCount() -> Int {
        lock.lock()
        let count = peripheralDeviceIds.count + centralDeviceIds.count
        lock.unlock()
        return count
    }

    // MARK: - Role Tracking

    func setPendingRole(_ role: MeshController.MeshRole, for identifier: UUID) {
        lock.lock()
        pendingRoles[identifier] = role
        lock.unlock()
    }

    func pendingRole(for identifier: UUID) -> MeshController.MeshRole? {
        lock.lock()
        let role = pendingRoles[identifier]
        lock.unlock()
        return role
    }

    func consumePendingRole(for identifier: UUID) -> MeshController.MeshRole? {
        lock.lock()
        let role = pendingRoles.removeValue(forKey: identifier)
        lock.unlock()
        return role
    }

    func setConnectionRole(_ role: MeshController.MeshRole, for deviceId: String) {
        lock.lock()
        connectionRoles[deviceId] = role
        lock.unlock()
    }

    func connectionRole(for deviceId: String) -> MeshController.MeshRole? {
        lock.lock()
        let role = connectionRoles[deviceId]
        lock.unlock()
        return role
    }

    func removeConnectionRole(for deviceId: String) {
        lock.lock()
        connectionRoles.removeValue(forKey: deviceId)
        lock.unlock()
    }

    // MARK: - Reset

    func reset() {
        lock.lock()
        connectedPeripherals.removeAll()
        peripheralDeviceIds.removeAll()
        centralDeviceIds.removeAll()
        pendingRoles.removeAll()
        connectionRoles.removeAll()
        lock.unlock()
    }
}


