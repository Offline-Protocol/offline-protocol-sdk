//
// PresenceWatchPolicyTests.swift
// Mirrors android/src/test/java/com/offlineprotocol/PresenceWatchPolicyTest.kt
//

import XCTest
@testable import OfflineProtocol

final class PresenceWatchPolicyTests: XCTestCase {

    func testMergesCoreWatchlistWithLocalSignals() {
        let policy = PresenceWatchPolicy()
        policy.watch("delivery-error-peer", nowMs: 1_000)

        let queried = policy.peersToQuery(coreWatchlist: ["welcome-pending-peer"], nowMs: 1_000)

        XCTAssertEqual(Set(queried), ["delivery-error-peer", "welcome-pending-peer"])
    }

    func testCapsQueriesPerTickAndRotatesAcrossTicks() {
        let policy = PresenceWatchPolicy(maxQueriesPerTick: 2)
        for i in 1...5 {
            policy.watch("peer\(i)", nowMs: 0)
        }

        let tick1 = policy.peersToQuery(coreWatchlist: [], nowMs: 1)
        let tick2 = policy.peersToQuery(coreWatchlist: [], nowMs: 2)
        let tick3 = policy.peersToQuery(coreWatchlist: [], nowMs: 3)

        XCTAssertEqual(tick1.count, 2)
        XCTAssertEqual(tick2.count, 2)
        // All five peers covered within ceil(5/2) = 3 ticks.
        XCTAssertEqual(
            Set(tick1 + tick2 + tick3),
            ["peer1", "peer2", "peer3", "peer4", "peer5"]
        )
    }

    func testUnwatchRemovesPeer() {
        let policy = PresenceWatchPolicy()
        policy.watch("bob", nowMs: 0)
        policy.unwatch("bob")

        XCTAssertTrue(policy.peersToQuery(coreWatchlist: [], nowMs: 1).isEmpty)
        XCTAssertTrue(policy.watchedPeers().isEmpty)
    }

    func testIdlePeersAreEvictedButCoreListedPeersStayFresh() {
        let policy = PresenceWatchPolicy(idleTtlMs: 1_000)
        policy.watch("stale", nowMs: 0)
        policy.watch("pending", nowMs: 0)

        // "pending" keeps being reported by the core watchlist, refreshing
        // its idle clock; "stale" was a one-off DeliveryError signal.
        let queried = policy.peersToQuery(coreWatchlist: ["pending"], nowMs: 2_000)

        XCTAssertEqual(queried, ["pending"])
        XCTAssertEqual(policy.watchedPeers(), ["pending"])
    }

    func testIgnoresEmptyPeerIdsAndClearResets() {
        let policy = PresenceWatchPolicy()
        policy.watch("", nowMs: 0)
        XCTAssertTrue(policy.peersToQuery(coreWatchlist: [""], nowMs: 1).isEmpty)

        policy.watch("bob", nowMs: 0)
        policy.clear()
        XCTAssertTrue(policy.watchedPeers().isEmpty)
        XCTAssertTrue(policy.peersToQuery(coreWatchlist: [], nowMs: 1).isEmpty)
    }
}
