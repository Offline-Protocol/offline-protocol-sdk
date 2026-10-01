//
// PeerStreamDialPolicyTests.swift
//
// Pins when the iOS peer-stream manager dials an advertised address and when
// it dials again (ADR 0027): the lower address at once, the higher after a
// grace delay, a redial ladder that climbs only on a redial actually
// scheduled, no dial toward an address a stream already holds, an address
// that stays advertised while any record for it is live, and how many
// inbound streams the listener takes. The
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

    /// A peer back from the background before its old record expired is
    /// advertised by two records; the old one going must leave the address.
    func testRemovingAStaleRecordKeepsTheLiveOnesAddress() {
        var adverts = PeerStreamDialPolicy.adverts(
            [(address: peer, endpoint: "old")], fresh: ["old"], current: [:])
        adverts = PeerStreamDialPolicy.adverts(
            [(address: peer, endpoint: "old"), (address: peer, endpoint: "new")],
            fresh: ["new"], current: adverts)
        XCTAssertEqual(adverts[peer], "new", "the record a change added is the newest")
        adverts = PeerStreamDialPolicy.adverts(
            [(address: peer, endpoint: "new")], fresh: [], current: adverts)
        XCTAssertEqual(adverts[peer], "new")
        XCTAssertNil(PeerStreamDialPolicy.adverts(
            [(address: String, endpoint: String)](), fresh: [], current: adverts)[peer],
            "the last record going takes the address")
    }

    func testTheRecordInUseStaysOverAnOlderOne() {
        let records = [(address: peer, endpoint: "a"), (address: peer, endpoint: "b")]
        for current in ["a", "b"] {
            XCTAssertEqual(PeerStreamDialPolicy.adverts(
                records, fresh: [], current: [peer: current])[peer], current)
        }
    }

    /// One machine on the LAN cannot fill the listener, and a full listener
    /// leaves room to dial.
    func testInboundIsBoundedPerHostAndLeavesRoomToDial() {
        let one = Array(repeating: "10.0.0.9", count: PeerStreamDialPolicy.maxInboundPerHost)
        XCTAssertFalse(PeerStreamDialPolicy.admitsInbound(from: "10.0.0.9", inboundFrom: one, open: one.count))
        XCTAssertTrue(PeerStreamDialPolicy.admitsInbound(from: "10.0.0.7", inboundFrom: one, open: one.count))

        let many = (0..<PeerStreamDialPolicy.maxInbound).map { "10.0.1.\($0)" }
        XCTAssertFalse(PeerStreamDialPolicy.admitsInbound(from: "10.0.2.1", inboundFrom: many, open: many.count))
        XCTAssertLessThan(PeerStreamDialPolicy.maxInbound, PeerStreamDialPolicy.maxStreams,
                          "dials keep a share of the budget")
        XCTAssertFalse(PeerStreamDialPolicy.admitsInbound(
            from: "10.0.2.1", inboundFrom: [String](), open: PeerStreamDialPolicy.maxStreams))
    }

    /// A dial that found no free slot is tried again, later each time, and
    /// stays the one dial under way for its address.
    func testADialWithNoSlotIsTriedAgainOnTheLadder() {
        var policy = PeerStreamDialPolicy()
        _ = policy.discovered(peer, weAreLower: true, held: false)
        XCTAssertEqual(policy.noSlot(peer), PeerStreamDialPolicy.redialInitialDelay)
        XCTAssertNil(policy.discovered(peer, weAreLower: true, held: false), "one dial at a time")
        XCTAssertEqual(policy.noSlot(peer), 2 * PeerStreamDialPolicy.redialInitialDelay)
    }
}
