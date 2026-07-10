//
// RecipientInFlightTrackerTests.swift
// Mirrors android/src/test/java/com/offlineprotocol/RecipientInFlightTrackerTest.kt
//

import XCTest
@testable import OfflineProtocol

final class RecipientInFlightTrackerTests: XCTestCase {

    func testDrainReturnsLiveIdsAndClearsRecipient() {
        let tracker = RecipientInFlightTracker(ttlMs: 60_000, maxPerRecipient: 32)
        tracker.recordSent(recipient: "bob", messageId: "m1", nowMs: 1_000)
        tracker.recordSent(recipient: "bob", messageId: "m2", nowMs: 2_000)
        tracker.recordSent(recipient: "carol", messageId: "m3", nowMs: 2_000)

        XCTAssertEqual(tracker.drainRecipient("bob", nowMs: 3_000), ["m1", "m2"])
        // Drained — a second DeliveryError must not double-fail the same ids.
        XCTAssertTrue(tracker.drainRecipient("bob", nowMs: 3_000).isEmpty)
        // Other recipients untouched.
        XCTAssertEqual(tracker.drainRecipient("carol", nowMs: 3_000), ["m3"])
    }

    func testDrainSkipsExpiredEntries() {
        let tracker = RecipientInFlightTracker(ttlMs: 1_000, maxPerRecipient: 32)
        tracker.recordSent(recipient: "bob", messageId: "old", nowMs: 0)
        tracker.recordSent(recipient: "bob", messageId: "fresh", nowMs: 1_500)

        XCTAssertEqual(tracker.drainRecipient("bob", nowMs: 2_000), ["fresh"])
    }

    func testCapDropsOldestFirst() {
        let tracker = RecipientInFlightTracker(ttlMs: 60_000, maxPerRecipient: 2)
        tracker.recordSent(recipient: "bob", messageId: "m1", nowMs: 1)
        tracker.recordSent(recipient: "bob", messageId: "m2", nowMs: 2)
        tracker.recordSent(recipient: "bob", messageId: "m3", nowMs: 3)

        XCTAssertEqual(tracker.drainRecipient("bob", nowMs: 4), ["m2", "m3"])
    }

    func testPruneEvictsExpiredEntriesAndEmptyRecipients() {
        let tracker = RecipientInFlightTracker(ttlMs: 1_000, maxPerRecipient: 32)
        tracker.recordSent(recipient: "bob", messageId: "old", nowMs: 0)
        tracker.recordSent(recipient: "carol", messageId: "live", nowMs: 1_800)

        tracker.prune(nowMs: 2_000)

        XCTAssertTrue(tracker.drainRecipient("bob", nowMs: 2_000).isEmpty)
        XCTAssertEqual(tracker.drainRecipient("carol", nowMs: 2_000), ["live"])
    }

    func testIgnoresEmptyInputsAndClearForgetsEverything() {
        let tracker = RecipientInFlightTracker()
        tracker.recordSent(recipient: "", messageId: "m1", nowMs: 1)
        tracker.recordSent(recipient: "bob", messageId: "", nowMs: 1)
        XCTAssertTrue(tracker.drainRecipient("", nowMs: 2).isEmpty)
        XCTAssertTrue(tracker.drainRecipient("bob", nowMs: 2).isEmpty)

        tracker.recordSent(recipient: "bob", messageId: "m1", nowMs: 1)
        tracker.clear()
        XCTAssertTrue(tracker.drainRecipient("bob", nowMs: 2).isEmpty)
    }
}
