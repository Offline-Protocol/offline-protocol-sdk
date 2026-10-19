//
// PeerLinkResolution.swift
// OfflineProtocol
//
// Which of a peer's peripheral identifiers the bridge sends on, retires and
// keeps dialling, once the peer has been seen under more than one.
//
// A peer that rotates its advertising address reappears to CoreBluetooth as a
// new CBPeripheral with a new identifier, and the bridge verifies it there
// while the identifier it held before still maps to the same device id. In
// the third 2026-10-19 volume run that left two identifiers mapped to the
// Android peer. The send path took whichever a dictionary walk met first,
// which was the stale one, so every fragment for the peer found "no connected
// peripheral" and re-dialled an address nobody advertised any more, for 25
// minutes, while the verified link to the same peer sat idle. The connection
// monitor meanwhile kept a dozen connects pending to identifiers that had
// stopped advertising, each cancelled and re-armed by the 10 s connect
// timeout.
//
// Three rules, each closing one of those:
// - Send on a connected link when the peer has one, whatever else maps to it.
// - Verifying the peer on one identifier retires every other identifier that
//   maps to it and is not connected: it is an address the peer has left.
// - The monitor re-dials a peripheral it never identified only while it is
//   still advertising. One that was identified is retired by the rule above
//   instead, so the monitor still reconnects a known peer that went quiet.
//
// Pure functions over snapshots, so the rules are testable without
// CoreBluetooth.
//

import Foundation

enum PeerLinkResolution {
    /// How long a never-identified peripheral stays worth dialling after its
    /// last advertisement. Above the 30 s an idle scan can go between
    /// sightings of a peer that is still there, and far below the hours a
    /// stale identifier otherwise lingers.
    static let unidentifiedRedialWindow: TimeInterval = 60

    /// The identifier to send `deviceId`'s fragments on: a connected one if
    /// the peer has one, else any peripheral-role identifier, else a
    /// central-role one. Ties break on the identifier's string so the answer
    /// does not depend on dictionary order.
    static func preferredIdentifier(
        for deviceId: String,
        peripheralDeviceIds: [UUID: String],
        centralDeviceIds: [UUID: String],
        isConnected: (UUID) -> Bool
    ) -> UUID? {
        let peripheralMatches = matches(deviceId, in: peripheralDeviceIds)
        if let connected = peripheralMatches.first(where: isConnected) {
            return connected
        }
        if let any = peripheralMatches.first {
            return any
        }
        return matches(deviceId, in: centralDeviceIds).first
    }

    /// The peripheral-role identifiers that verifying `deviceId` on
    /// `verified` retires: every other one mapped to the same device that is
    /// not connected. A connected one is a live second link and stays.
    static func superseded(
        by verified: UUID,
        deviceId: String,
        peripheralDeviceIds: [UUID: String],
        isConnected: (UUID) -> Bool
    ) -> [UUID] {
        matches(deviceId, in: peripheralDeviceIds).filter { $0 != verified && !isConnected($0) }
    }

    /// Whether the connection monitor should dial a peripheral it holds and
    /// is not connected to.
    static func monitorShouldRedial(
        identified: Bool,
        lastAdvertisedAt: Date?,
        now: Date,
        window: TimeInterval = PeerLinkResolution.unidentifiedRedialWindow
    ) -> Bool {
        if identified { return true }
        guard let seen = lastAdvertisedAt else { return false }
        return now.timeIntervalSince(seen) < window
    }

    private static func matches(_ deviceId: String, in map: [UUID: String]) -> [UUID] {
        map.compactMap { $0.value == deviceId ? $0.key : nil }
            .sorted { $0.uuidString < $1.uuidString }
    }
}
