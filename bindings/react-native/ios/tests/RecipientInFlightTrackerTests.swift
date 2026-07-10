//
// RecipientInFlightTrackerTests.swift
// Mirrors android/src/test/java/com/offlineprotocol/RecipientInFlightTrackerTest.kt
//

import XCTest
@testable import OfflineProtocol

final class RecipientInFlightTrackerTests: XCTestCase {

    func testDrainReturnsLiveIdsAndClearsRecipient() {
        let tracker = RecipientInFlightTracker(ttlMs: 60_000, maxPerRecipient: 32)
        tracker.recordSent(recipient: "bob", messageId: "m1", plane: .data, nowMs: 1_000)
        tracker.recordSent(recipient: "bob", messageId: "m2", plane: .data, nowMs: 2_000)
        tracker.recordSent(recipient: "carol", messageId: "m3", plane: .data, nowMs: 2_000)

        XCTAssertEqual(tracker.drainRecipient("bob", plane: .data, nowMs: 3_000), ["m1", "m2"])
        // Drained — a second DeliveryError must not double-fail the same ids.
        XCTAssertTrue(tracker.drainRecipient("bob", plane: .data, nowMs: 3_000).isEmpty)
        // Other recipients untouched.
        XCTAssertEqual(tracker.drainRecipient("carol", plane: .data, nowMs: 3_000), ["m3"])
    }

    func testDrainSkipsExpiredEntries() {
        let tracker = RecipientInFlightTracker(ttlMs: 1_000, maxPerRecipient: 32)
        tracker.recordSent(recipient: "bob", messageId: "old", plane: .data, nowMs: 0)
        tracker.recordSent(recipient: "bob", messageId: "fresh", plane: .data, nowMs: 1_500)

        XCTAssertEqual(tracker.drainRecipient("bob", plane: .data, nowMs: 2_000), ["fresh"])
    }

    func testCapDropsOldestFirst() {
        let tracker = RecipientInFlightTracker(ttlMs: 60_000, maxPerRecipient: 2)
        tracker.recordSent(recipient: "bob", messageId: "m1", plane: .data, nowMs: 1)
        tracker.recordSent(recipient: "bob", messageId: "m2", plane: .data, nowMs: 2)
        tracker.recordSent(recipient: "bob", messageId: "m3", plane: .data, nowMs: 3)

        XCTAssertEqual(tracker.drainRecipient("bob", plane: .data, nowMs: 4), ["m2", "m3"])
    }

    func testPruneEvictsExpiredEntriesAndEmptyRecipients() {
        let tracker = RecipientInFlightTracker(ttlMs: 1_000, maxPerRecipient: 32)
        tracker.recordSent(recipient: "bob", messageId: "old", plane: .data, nowMs: 0)
        // TTL pruning is per entry regardless of plane.
        tracker.recordSent(recipient: "bob", messageId: "old-conn", plane: .connReq, nowMs: 0)
        tracker.recordSent(recipient: "carol", messageId: "live", plane: .data, nowMs: 1_800)

        tracker.prune(nowMs: 2_000)

        XCTAssertTrue(tracker.drainRecipient("bob", plane: .data, nowMs: 2_000).isEmpty)
        XCTAssertTrue(tracker.drainRecipient("bob", plane: .connReq, nowMs: 2_000).isEmpty)
        XCTAssertEqual(tracker.drainRecipient("carol", plane: .data, nowMs: 2_000), ["live"])
    }

    func testRelayAcceptedResolvesExactIdWhenItMatches() {
        let tracker = RecipientInFlightTracker(ttlMs: 60_000, maxPerRecipient: 32)
        tracker.recordSent(recipient: "bob", messageId: "m1", plane: .data, nowMs: 1_000)
        tracker.recordSent(recipient: "bob", messageId: "m2", plane: .data, nowMs: 2_000)

        tracker.resolveOnRelayAccepted(recipient: "bob", messageId: "m2", nowMs: 3_000)

        // Only the accepted frame left the tracker; a later DeliveryError
        // still fails the genuinely unresolved one.
        XCTAssertEqual(tracker.drainRecipient("bob", plane: .data, nowMs: 4_000), ["m1"])
    }

    func testRelayAcceptedFallsBackToOldestForUnknownOrMissingId() {
        let tracker = RecipientInFlightTracker(ttlMs: 60_000, maxPerRecipient: 32)
        tracker.recordSent(recipient: "bob", messageId: "m1", plane: .data, nowMs: 1_000)
        tracker.recordSent(recipient: "bob", messageId: "m2", plane: .data, nowMs: 2_000)
        tracker.recordSent(recipient: "bob", messageId: "m3", plane: .data, nowMs: 3_000)

        // The relay echoes a server-generated id: sends per recipient are
        // FIFO on one socket, so the answer belongs to the oldest in-flight.
        tracker.resolveOnRelayAccepted(recipient: "bob", messageId: "server-id", nowMs: 4_000)
        tracker.resolveOnRelayAccepted(recipient: "bob", messageId: nil, nowMs: 4_000)

        XCTAssertEqual(tracker.drainRecipient("bob", plane: .data, nowMs: 5_000), ["m3"])
    }

    func testRelayAcceptedIgnoresExpiredEntriesAndUnknownRecipients() {
        let tracker = RecipientInFlightTracker(ttlMs: 1_000, maxPerRecipient: 32)
        tracker.recordSent(recipient: "bob", messageId: "stale", plane: .data, nowMs: 0)
        tracker.recordSent(recipient: "bob", messageId: "fresh", plane: .data, nowMs: 2_500)

        // The stale entry is expired housekeeping, not the oldest live send:
        // the answer must resolve "fresh", not be eaten by "stale".
        tracker.resolveOnRelayAccepted(recipient: "bob", messageId: nil, nowMs: 3_000)
        XCTAssertTrue(tracker.drainRecipient("bob", plane: .data, nowMs: 3_000).isEmpty)

        // No-ops, never throws.
        tracker.resolveOnRelayAccepted(recipient: "nobody", messageId: "m1", nowMs: 3_000)
        tracker.resolveOnRelayAccepted(recipient: "", messageId: "m1", nowMs: 3_000)
    }

    func testRelayAcceptedSkipsConnReqAndResolvesOldestData() {
        let tracker = RecipientInFlightTracker(ttlMs: 60_000, maxPerRecipient: 32)
        // The CONN_REQ entry is OLDER than the data sends: MessageSent only
        // answers SendMessage frames, so the oldest-first fallback must skip
        // it — eating the conn_req would leave the actually-answered data
        // frame exposed to a later DeliveryError sweep.
        tracker.recordSent(recipient: "bob", messageId: "c1", plane: .connReq, nowMs: 1_000)
        tracker.recordSent(recipient: "bob", messageId: "d1", plane: .data, nowMs: 2_000)

        tracker.resolveOnRelayAccepted(recipient: "bob", messageId: "server-id", nowMs: 3_000)

        XCTAssertTrue(tracker.drainRecipient("bob", plane: .data, nowMs: 3_000).isEmpty)
        XCTAssertEqual(tracker.drainRecipient("bob", plane: .connReq, nowMs: 3_000), ["c1"])
    }

    func testDeliveryErrorDrainLeavesConnReqUntouched() {
        let tracker = RecipientInFlightTracker(ttlMs: 60_000, maxPerRecipient: 32)
        tracker.recordSent(recipient: "bob", messageId: "c1", plane: .connReq, nowMs: 1_000)
        tracker.recordSent(recipient: "bob", messageId: "d1", plane: .data, nowMs: 2_000)

        // DeliveryError answers data frames only: the pending conn_req has
        // its own failure channel (ConnectionRequestError) and must survive.
        XCTAssertEqual(tracker.drainRecipient("bob", plane: .data, nowMs: 3_000), ["d1"])
        XCTAssertEqual(tracker.drainRecipient("bob", plane: .connReq, nowMs: 3_000), ["c1"])
    }

    func testConnectionRequestErrorDrainLeavesDataUntouched() {
        let tracker = RecipientInFlightTracker(ttlMs: 60_000, maxPerRecipient: 32)
        tracker.recordSent(recipient: "bob", messageId: "d1", plane: .data, nowMs: 1_000)
        tracker.recordSent(recipient: "bob", messageId: "c1", plane: .connReq, nowMs: 2_000)

        // ConnectionRequestError answers conn ops only: in-flight data
        // frames keep waiting for MessageSent/DeliveryError.
        XCTAssertEqual(tracker.drainRecipient("bob", plane: .connReq, nowMs: 3_000), ["c1"])
        XCTAssertEqual(tracker.drainRecipient("bob", plane: .data, nowMs: 3_000), ["d1"])
    }

    func testPlaneIsolationRegression() {
        let tracker = RecipientInFlightTracker(ttlMs: 60_000, maxPerRecipient: 32)
        // The mixed-plane regression: a pending conn_req to bob, then data
        // sends to bob. MessageSent (unknown server id) must resolve the
        // oldest DATA entry — not the older conn_req — and the DeliveryError
        // that follows must fail only the remaining data frame.
        tracker.recordSent(recipient: "bob", messageId: "c1", plane: .connReq, nowMs: 1_000)
        tracker.recordSent(recipient: "bob", messageId: "d1", plane: .data, nowMs: 2_000)
        tracker.recordSent(recipient: "bob", messageId: "d2", plane: .data, nowMs: 3_000)

        tracker.resolveOnRelayAccepted(recipient: "bob", messageId: "server-id", nowMs: 4_000)

        XCTAssertEqual(tracker.drainRecipient("bob", plane: .data, nowMs: 4_000), ["d2"])
        XCTAssertEqual(tracker.drainRecipient("bob", plane: .connReq, nowMs: 4_000), ["c1"])
    }

    func testUnrecordRemovesTheExactEntry() {
        let tracker = RecipientInFlightTracker(ttlMs: 60_000, maxPerRecipient: 32)
        tracker.recordSent(recipient: "bob", messageId: "m1", plane: .data, nowMs: 1_000)
        tracker.recordSent(recipient: "bob", messageId: "m2", plane: .data, nowMs: 2_000)

        // A failed socket write takes back its optimistic pre-send entry;
        // the sibling send stays tracked.
        tracker.unrecord(recipient: "bob", messageId: "m1")
        XCTAssertEqual(tracker.drainRecipient("bob", plane: .data, nowMs: 3_000), ["m2"])

        // No-ops, never throws.
        tracker.unrecord(recipient: "bob", messageId: "gone")
        tracker.unrecord(recipient: "nobody", messageId: "m1")
        tracker.unrecord(recipient: "", messageId: "")
    }

    func testIgnoresEmptyInputsAndClearForgetsEverything() {
        let tracker = RecipientInFlightTracker()
        tracker.recordSent(recipient: "", messageId: "m1", plane: .data, nowMs: 1)
        tracker.recordSent(recipient: "bob", messageId: "", plane: .data, nowMs: 1)
        XCTAssertTrue(tracker.drainRecipient("", plane: .data, nowMs: 2).isEmpty)
        XCTAssertTrue(tracker.drainRecipient("bob", plane: .data, nowMs: 2).isEmpty)

        tracker.recordSent(recipient: "bob", messageId: "m1", plane: .data, nowMs: 1)
        tracker.recordSent(recipient: "bob", messageId: "c1", plane: .connReq, nowMs: 1)
        tracker.clear()
        XCTAssertTrue(tracker.drainRecipient("bob", plane: .data, nowMs: 2).isEmpty)
        XCTAssertTrue(tracker.drainRecipient("bob", plane: .connReq, nowMs: 2).isEmpty)
    }
}
