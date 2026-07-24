//
// SocketGenerationTrackerTests.swift
//
// iOS-only: there is no Kotlin mirror for SocketGenerationTracker (the Android
// close funnel drops non-current sockets before the supersede decision, so it
// needs no generation tracking — see SocketGenerationTracker.swift).
//

import XCTest
@testable import OfflineProtocol

final class SocketGenerationTrackerTests: XCTestCase {

    func testFreshTrackerHasNoGeneration() {
        let tracker = SocketGenerationTracker()
        XCTAssertEqual(tracker.latest, 0)
    }

    func testMintIsMonotonicAndAdvancesLatest() {
        var tracker = SocketGenerationTracker()
        XCTAssertEqual(tracker.mint(), 1)
        XCTAssertEqual(tracker.latest, 1)
        XCTAssertEqual(tracker.mint(), 2)
        XCTAssertEqual(tracker.mint(), 3)
        XCTAssertEqual(tracker.latest, 3)
    }

    func testCurrentGenerationIsNotBygone() {
        // A 4000 for the current generation must still latch: the live socket
        // was genuinely displaced (case a).
        var tracker = SocketGenerationTracker()
        let gen = tracker.mint()
        XCTAssertFalse(tracker.isBygone(gen))
    }

    func testOlderGenerationIsBygone() {
        // A 4000 for a socket minted before the newest one is stale and must
        // NOT re-latch (case b — the nil reconnect window).
        var tracker = SocketGenerationTracker()
        let first = tracker.mint()
        _ = tracker.mint()
        XCTAssertTrue(tracker.isBygone(first))
    }

    func testBygoneHoldsAcrossManyGenerations() {
        var tracker = SocketGenerationTracker()
        let first = tracker.mint()
        for _ in 0..<5 { _ = tracker.mint() }
        XCTAssertTrue(tracker.isBygone(first))
        XCTAssertFalse(tracker.isBygone(tracker.latest))
    }
}
