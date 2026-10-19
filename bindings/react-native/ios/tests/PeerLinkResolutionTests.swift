//
// PeerLinkResolutionTests.swift
// OfflineProtocol
//

import XCTest
@testable import OfflineProtocol

final class PeerLinkResolutionTests: XCTestCase {
    private let peer = "off1peer"
    // Ordered so the stale identifier sorts first: a lookup that took the
    // first match it met, as the registry's dictionary walk could, picks it.
    private let stale = UUID(uuidString: "00000000-0000-0000-0000-000000000001")!
    private let fresh = UUID(uuidString: "00000000-0000-0000-0000-000000000002")!
    private let other = UUID(uuidString: "00000000-0000-0000-0000-000000000003")!

    /// The third volume run: the peer rotated its address, was verified on
    /// the new identifier, and the old one still mapped to it. Sending on the
    /// old one found no connection for 25 minutes.
    func testASendGoesToTheConnectedLinkWhenAStaleIdentifierAlsoMapsToThePeer() {
        let chosen = PeerLinkResolution.preferredIdentifier(
            for: peer,
            peripheralDeviceIds: [stale: peer, fresh: peer, other: "off1other"],
            centralDeviceIds: [:],
            isConnected: { $0 == self.fresh }
        )
        XCTAssertEqual(chosen, fresh)
    }

    func testWithNoConnectedLinkTheChoiceIsStableAndPrefersThePeripheralRole() {
        let chosen = PeerLinkResolution.preferredIdentifier(
            for: peer,
            peripheralDeviceIds: [fresh: peer, stale: peer],
            centralDeviceIds: [other: peer],
            isConnected: { _ in false }
        )
        XCTAssertEqual(chosen, stale)
        XCTAssertEqual(
            PeerLinkResolution.preferredIdentifier(
                for: peer,
                peripheralDeviceIds: [:],
                centralDeviceIds: [other: peer],
                isConnected: { _ in false }
            ),
            other
        )
        XCTAssertNil(
            PeerLinkResolution.preferredIdentifier(
                for: peer,
                peripheralDeviceIds: [other: "off1other"],
                centralDeviceIds: [:],
                isConnected: { _ in true }
            )
        )
    }

    func testVerifyingOnANewIdentifierRetiresTheDisconnectedOnesOnly() {
        let third = UUID()
        let retired = PeerLinkResolution.superseded(
            by: fresh,
            deviceId: peer,
            peripheralDeviceIds: [stale: peer, fresh: peer, third: peer, other: "off1other"],
            isConnected: { $0 == third }
        )
        XCTAssertEqual(retired, [stale], "a connected second link stays; another peer's never goes")
    }

    func testTheMonitorDialsAnUnidentifiedPeripheralOnlyWhileItAdvertises() {
        let now = Date()
        XCTAssertTrue(PeerLinkResolution.monitorShouldRedial(identified: false, lastAdvertisedAt: now.addingTimeInterval(-5), now: now))
        XCTAssertFalse(PeerLinkResolution.monitorShouldRedial(identified: false, lastAdvertisedAt: now.addingTimeInterval(-61), now: now))
        XCTAssertFalse(PeerLinkResolution.monitorShouldRedial(identified: false, lastAdvertisedAt: nil, now: now))
        // A known peer that went quiet is still reconnected: rotation is
        // handled by retiring the identifier, not by the monitor giving up.
        XCTAssertTrue(PeerLinkResolution.monitorShouldRedial(identified: true, lastAdvertisedAt: nil, now: now))
    }
}
