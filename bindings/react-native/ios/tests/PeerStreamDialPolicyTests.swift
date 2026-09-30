//
// PeerStreamDialPolicyTests.swift
//
// Pins when the iOS peer-stream manager dials an advertised address and when
// it dials again (ADR 0027): the lower address at once, the higher after a
// grace delay, a redial ladder that climbs only on a redial actually
// scheduled, and no dial toward an address a stream already holds. The
// manager owns the timers and connections, which need a device; this is the
// decision it makes with them.
//

import XCTest
@testable import OfflineProtocol

final class PeerStreamDialPolicyTests: XCTestCase {

    private let peer = "off1qysluvwl5922yctzd0u9gpr06gn3k7ldfvgtwgvn"

    func testTheLowerAddressDialsAtOnceAndTheHigherWaits() {
        var lower = PeerStreamDialPolicy()
        XCTAssertEqual(lower.discovered(peer, weAreLower: true, held: false), 0)
        var higher = PeerStreamDialPolicy()
        XCTAssertEqual(higher.discovered(peer, weAreLower: false, held: false),
                       PeerStreamDialPolicy.higherAddressDelay)
    }

    func testNoDialWhileOneIsUnderWayOrTheAddressIsHeld() {
        var policy = PeerStreamDialPolicy()
        XCTAssertNotNil(policy.discovered(peer, weAreLower: true, held: false))
        XCTAssertNil(policy.discovered(peer, weAreLower: true, held: false), "one dial at a time")

        var held = PeerStreamDialPolicy()
        XCTAssertNil(held.discovered(peer, weAreLower: true, held: true))
    }

    func testAnAbandonedDialLetsTheNextAdvertDial() {
        var policy = PeerStreamDialPolicy()
        _ = policy.discovered(peer, weAreLower: true, held: false)
        policy.abandoned(peer)
        XCTAssertEqual(policy.discovered(peer, weAreLower: true, held: false), 0)
    }

    func testTheLadderDoublesToItsCap() {
        var policy = PeerStreamDialPolicy()
        _ = policy.discovered(peer, weAreLower: true, held: false)
        var delays: [TimeInterval] = []
        for _ in 0..<9 {
            delays.append(policy.ended(peer, advertised: true, held: false)!)
        }
        XCTAssertEqual(delays, [1, 2, 4, 8, 16, 32, 60, 60, 60])
    }

    func testALostRaceDoesNotClimbTheLadder() {
        // The higher address's dial loses to the lower's stream: its stream
        // ends while the address is held. That is no redial, so no step.
        var policy = PeerStreamDialPolicy()
        for _ in 0..<5 {
            _ = policy.discovered(peer, weAreLower: false, held: false)
            XCTAssertNil(policy.ended(peer, advertised: true, held: true))
        }
        // When the held stream later ends, the first redial is the first step.
        XCTAssertEqual(policy.discovered(peer, weAreLower: false, held: false),
                       PeerStreamDialPolicy.higherAddressDelay)
        XCTAssertEqual(policy.ended(peer, advertised: true, held: false), 1)
    }

    func testNoRedialOnceTheAdvertIsGone() {
        var policy = PeerStreamDialPolicy()
        _ = policy.discovered(peer, weAreLower: true, held: false)
        XCTAssertNil(policy.ended(peer, advertised: false, held: false))
        XCTAssertEqual(policy.discovered(peer, weAreLower: true, held: false), 0,
                       "the ended stream no longer counts as a dial under way")
    }

    func testAProvedStreamStartsTheLadderOver() {
        var policy = PeerStreamDialPolicy()
        _ = policy.discovered(peer, weAreLower: true, held: false)
        _ = policy.ended(peer, advertised: true, held: false)
        _ = policy.ended(peer, advertised: true, held: false)
        policy.proved(peer)
        XCTAssertEqual(policy.ended(peer, advertised: true, held: false), 1)
    }

    func testResetForgetsEverything() {
        var policy = PeerStreamDialPolicy()
        _ = policy.discovered(peer, weAreLower: true, held: false)
        _ = policy.ended(peer, advertised: true, held: false)
        policy.reset()
        XCTAssertEqual(policy.discovered(peer, weAreLower: true, held: false), 0)
        XCTAssertEqual(policy.ended(peer, advertised: true, held: false), 1)
    }
}
