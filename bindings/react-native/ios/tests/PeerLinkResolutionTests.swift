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

    /// Run 5: Android advertised on `stale` (the link the stall teardown
    /// cancelled) and was also connected and verified on `fresh`. The timed-out
    /// redial of `stale` must not lose the peer, and `stale` must not be
    /// dialled while `fresh` is up.
    func testAPeerConnectedOnAnotherIdentifierHasALiveLink() {
        let links = PeerLinkResolution.otherLiveLinks(
            of: peer,
            excluding: stale,
            peripheralDeviceIds: [stale: peer, fresh: peer, other: "off1other"],
            centralDeviceIds: [stale: peer, fresh: peer],
            isConnected: { $0 == self.fresh },
            isSubscribed: { $0 == self.fresh }
        )
        XCTAssertEqual(links, [fresh], "one link, counted once though it is in both roles")
    }

    func testWithNoOtherLinkThePeerIsLost() {
        XCTAssertEqual(
            PeerLinkResolution.otherLiveLinks(
                of: peer,
                excluding: stale,
                peripheralDeviceIds: [stale: peer, fresh: peer],
                centralDeviceIds: [:],
                isConnected: { _ in false },
                isSubscribed: { _ in false }
            ),
            [],
            "a mapped identifier that is not connected is not a link"
        )
        XCTAssertEqual(
            PeerLinkResolution.otherLiveLinks(
                of: peer,
                excluding: stale,
                peripheralDeviceIds: [stale: peer],
                centralDeviceIds: [stale: peer],
                isConnected: { $0 == self.stale },
                isSubscribed: { $0 == self.stale }
            ),
            [],
            "the identifier that dropped never counts for itself"
        )
    }

    func testAVerifiedSubscribedCentralIsALink() {
        XCTAssertEqual(
            PeerLinkResolution.otherLiveLinks(
                of: peer,
                excluding: stale,
                peripheralDeviceIds: [stale: peer],
                centralDeviceIds: [other: peer],
                isConnected: { _ in false },
                isSubscribed: { $0 == self.other }
            ),
            [other]
        )
        XCTAssertEqual(
            PeerLinkResolution.otherLiveLinks(
                of: peer,
                excluding: stale,
                peripheralDeviceIds: [stale: peer],
                centralDeviceIds: [other: peer],
                isConnected: { _ in false },
                isSubscribed: { _ in false }
            ),
            [],
            "a central that unsubscribed carries nothing"
        )
    }

    /// A subscribed central counts only while it is heard from. One whose
    /// `didUnsubscribeFrom` never arrived would otherwise hold a departed
    /// peer as a neighbour for good.
    func testASubscriptionCountsOnlyWhileItsCentralIsHeardFrom() {
        let now = Date(timeIntervalSince1970: 10_000)
        XCTAssertTrue(PeerLinkResolution.subscriptionIsLive(lastHeardAt: now.addingTimeInterval(-59), now: now))
        XCTAssertFalse(PeerLinkResolution.subscriptionIsLive(lastHeardAt: now.addingTimeInterval(-60), now: now))
        XCTAssertFalse(PeerLinkResolution.subscriptionIsLive(lastHeardAt: nil, now: now))
    }

    /// The monitor's window is exclusive at its edge, like the liveness one.
    func testTheMonitorStopsDiallingAnUnidentifiedPeripheralExactlyAtTheWindow() {
        let now = Date(timeIntervalSince1970: 10_000)
        let window = PeerLinkResolution.unidentifiedRedialWindow
        XCTAssertTrue(PeerLinkResolution.monitorShouldRedial(
            identified: false, lastAdvertisedAt: now.addingTimeInterval(-window + 1), now: now))
        XCTAssertFalse(PeerLinkResolution.monitorShouldRedial(
            identified: false, lastAdvertisedAt: now.addingTimeInterval(-window), now: now))
    }

    /// The disconnect path retires on the kept link even when that link is a
    /// subscribed central with no peripheral-role mapping of its own.
    func testRetiringOnACentralOnlyLinkRetiresEveryDisconnectedPeripheral() {
        let retired = PeerLinkResolution.superseded(
            by: other,
            deviceId: peer,
            peripheralDeviceIds: [stale: peer, fresh: peer],
            isConnected: { _ in false }
        )
        XCTAssertEqual(retired, [stale, fresh])
    }
}

