//
// OutboundFragmentQueueTests.swift
// Mirrors android/src/test/java/com/offlineprotocol/ble/OutboundFragmentQueueTest.kt
//

import XCTest
@testable import OfflineProtocol

final class OutboundFragmentQueueTests: XCTestCase {

    private func bytes(_ n: UInt8) -> Data { Data([n]) }

    /// Records what `flush` handed to the transport, and can be told to refuse
    /// from a given call onward (the flow-control case).
    private final class Sender {
        private(set) var sent: [(String, Data)] = []
        var refuseAfter: Int = .max

        func send(_ recipientId: String, _ data: Data) -> Bool {
            guard sent.count < refuseAfter else { return false }
            sent.append((recipientId, data))
            return true
        }
    }

    // MARK: - Ordering

    func testFlushSendsFragmentsInFifoOrder() {
        let queue = OutboundFragmentQueue()
        let sender = Sender()
        queue.enqueue("bob", bytes(1))
        queue.enqueue("bob", bytes(2))
        queue.enqueue("bob", bytes(3))

        XCTAssertFalse(queue.flush(send: sender.send))
        XCTAssertEqual(sender.sent.map(\.1), [bytes(1), bytes(2), bytes(3)])
        XCTAssertEqual(queue.totalCount(), 0)
    }

    // MARK: - Flow control

    func testFlushStopsAtFirstRefusalAndKeepsTheRemainder() {
        let queue = OutboundFragmentQueue()
        let sender = Sender()
        sender.refuseAfter = 1
        queue.enqueue("bob", bytes(1))
        queue.enqueue("bob", bytes(2))
        queue.enqueue("bob", bytes(3))

        XCTAssertTrue(queue.flush(send: sender.send), "unsent fragments must be reported")
        XCTAssertEqual(sender.sent.map(\.1), [bytes(1)])
        XCTAssertEqual(queue.totalCount(), 2)

        // The refused fragment is retried head-first on the next flush.
        let retry = Sender()
        XCTAssertFalse(queue.flush(send: retry.send))
        XCTAssertEqual(retry.sent.map(\.1), [bytes(2), bytes(3)])
    }

    func testOneStalledRecipientDoesNotBlockAnother() {
        let queue = OutboundFragmentQueue()
        queue.enqueue("bob", bytes(1))
        queue.enqueue("carol", bytes(2))

        var attempted: [String] = []
        let hasUnsent = queue.flush { recipientId, _ in
            attempted.append(recipientId)
            return recipientId != "bob"      // bob is flow-controlled
        }

        XCTAssertTrue(hasUnsent)
        XCTAssertEqual(Set(attempted), Set(["bob", "carol"]))
        XCTAssertTrue(queue.hasPending("bob"))
        XCTAssertFalse(queue.hasPending("carol"))
    }

    func testFlushPutsTheRemainderAheadOfFragmentsEnqueuedDuringTheSend() {
        let queue = OutboundFragmentQueue()
        queue.enqueue("bob", bytes(1))
        queue.enqueue("bob", bytes(2))

        // A fragment arriving mid-flush must land behind the ones being retried,
        // or the peer's stream reorders.
        var enqueuedDuringSend = false
        _ = queue.flush { _, _ in
            if !enqueuedDuringSend {
                enqueuedDuringSend = true
                queue.enqueue("bob", self.bytes(9))
            }
            return false
        }

        let drain = Sender()
        _ = queue.flush(send: drain.send)
        XCTAssertEqual(drain.sent.map(\.1), [bytes(1), bytes(2), bytes(9)])
    }

    // MARK: - Expiry

    func testFlushDropsExpiredFragmentsBeforeSending() {
        var now = Date(timeIntervalSince1970: 0)
        var dropped: [(String, OutboundFragmentQueue.DropReason, Int)] = []
        let queue = OutboundFragmentQueue(
            timeout: 30,
            clock: { now },
            onDropped: { dropped.append(($0, $1, $2)) }
        )
        queue.enqueue("bob", bytes(1))
        queue.enqueue("bob", bytes(2))
        now = now.addingTimeInterval(30)

        let sender = Sender()
        XCTAssertFalse(queue.flush(send: sender.send))
        XCTAssertTrue(sender.sent.isEmpty, "expired fragments must never reach the transport")
        XCTAssertEqual(queue.totalCount(), 0)
        XCTAssertEqual(dropped.count, 1)
        XCTAssertEqual(dropped.first?.2, 2)
        if case .expired = dropped.first?.1 {} else {
            XCTFail("expected .expired, got \(String(describing: dropped.first?.1))")
        }
    }

    /// A partial expiry is the case that actually tears a message — the early
    /// fragments go, the later ones ship — so it must be reported, not just the
    /// whole-queue case. Reporting only the latter is what made this class of
    /// drop invisible in telemetry.
    func testFlushSendsSurvivorsWhenOnlySomeFragmentsExpired() {
        var now = Date(timeIntervalSince1970: 0)
        var dropped: [(String, OutboundFragmentQueue.DropReason, Int)] = []
        let queue = OutboundFragmentQueue(
            timeout: 30,
            clock: { now },
            onDropped: { dropped.append(($0, $1, $2)) }
        )
        queue.enqueue("bob", bytes(1))
        now = now.addingTimeInterval(20)
        queue.enqueue("bob", bytes(2))
        now = now.addingTimeInterval(15)     // fragment 1 is 35s old, fragment 2 is 15s

        let sender = Sender()
        XCTAssertFalse(queue.flush(send: sender.send))
        XCTAssertEqual(sender.sent.map(\.1), [bytes(2)])

        // The expired count is what was dropped, not the queue depth.
        XCTAssertEqual(dropped.count, 1)
        XCTAssertEqual(dropped.first?.2, 1)
        if case .expired = dropped.first?.1 {} else {
            XCTFail("expected .expired, got \(String(describing: dropped.first?.1))")
        }
    }

    func testFlushReportsNothingWhenNothingExpired() {
        var dropped: [(String, OutboundFragmentQueue.DropReason, Int)] = []
        let queue = OutboundFragmentQueue(onDropped: { dropped.append(($0, $1, $2)) })
        queue.enqueue("bob", bytes(1))

        let sender = Sender()
        XCTAssertFalse(queue.flush(send: sender.send))
        XCTAssertTrue(dropped.isEmpty)
    }

    // MARK: - Overflow

    /// The overflow policy is whole-queue drop, not oldest-first, matching
    /// Android and the inbound side: evicting slice 0 of a multi-fragment
    /// message leaves orphan slices that reassemble into garbage at the
    /// receiver, so the only safe cut is the whole queue.
    func testOverflowDiscardsTheWholeQueue() {
        var dropped: [(String, OutboundFragmentQueue.DropReason, Int)] = []
        let queue = OutboundFragmentQueue(
            maxPerPeer: 3,
            onDropped: { dropped.append(($0, $1, $2)) }
        )
        queue.enqueue("bob", bytes(1))
        queue.enqueue("bob", bytes(2))
        queue.enqueue("bob", bytes(3))
        queue.enqueue("carol", bytes(7))
        queue.enqueue("bob", bytes(4))

        let sender = Sender()
        _ = queue.flush(send: sender.send)
        XCTAssertEqual(sender.sent.filter { $0.0 == "bob" }.map(\.1), [bytes(4)])
        XCTAssertEqual(sender.sent.filter { $0.0 == "carol" }.map(\.1), [bytes(7)],
                       "another recipient's queue must be untouched")
        XCTAssertEqual(dropped.count, 1)
        XCTAssertEqual(dropped.first?.2, 3)
        if case .capped = dropped.first?.1 {} else {
            XCTFail("expected .capped, got \(String(describing: dropped.first?.1))")
        }
    }

    /// The drain loop's signal to stop pulling from the Rust core must trip
    /// before the cap, or the loop overflows the queue it is protecting.
    func testIsBackedUpTripsAtThreeQuartersOfTheCap() {
        let queue = OutboundFragmentQueue(maxPerPeer: 8)
        for i in 0..<5 { queue.enqueue("bob", bytes(UInt8(i))) }
        XCTAssertFalse(queue.isBackedUp("bob"))

        queue.enqueue("bob", bytes(5))
        XCTAssertTrue(queue.isBackedUp("bob"))
        XCTAssertFalse(queue.isBackedUp("carol"))

        // Draining below the mark releases it, so the next drain pulls again.
        let sender = Sender()
        sender.refuseAfter = 1
        _ = queue.flush(send: sender.send)
        XCTAssertFalse(queue.isBackedUp("bob"))
    }

    // MARK: - Removal

    func testRemoveAllReturnsDroppedCount() {
        let queue = OutboundFragmentQueue()
        queue.enqueue("bob", bytes(1))
        queue.enqueue("bob", bytes(2))

        XCTAssertEqual(queue.removeAll("bob"), 2)
        XCTAssertEqual(queue.removeAll("bob"), 0)
    }

    func testClearDropsEveryQueue() {
        let queue = OutboundFragmentQueue()
        queue.enqueue("bob", bytes(1))
        queue.enqueue("carol", bytes(2))

        queue.clear()
        XCTAssertEqual(queue.totalCount(), 0)
        XCTAssertTrue(queue.recipientIds().isEmpty)
    }

    // MARK: - Cross-thread reads

    func testSnapshotReadsReflectQueuedState() {
        let queue = OutboundFragmentQueue()
        XCTAssertEqual(queue.totalCount(), 0)

        queue.enqueue("bob", bytes(1))
        queue.enqueue("bob", bytes(2))
        queue.enqueue("carol", bytes(3))

        XCTAssertEqual(queue.totalCount(), 3)
        XCTAssertEqual(Set(queue.recipientIds()), Set(["bob", "carol"]))
        XCTAssertTrue(queue.hasPending("bob"))
        XCTAssertFalse(queue.hasPending("dave"))
    }

    /// The reason this class exists — see the matching inbound test. Notably
    /// the lock must not be held across `send`, or a reader would block behind
    /// a CoreBluetooth write.
    func testSnapshotReadsDoNotBlockOnAFlushInProgress() {
        let queue = OutboundFragmentQueue()
        queue.enqueue("bob", bytes(1))

        let sending = DispatchSemaphore(value: 0)
        let release = DispatchSemaphore(value: 0)
        let owningQueue = DispatchQueue(label: "test.owning")

        owningQueue.async {
            _ = queue.flush { _, _ in
                sending.signal()
                release.wait()      // a slow BLE write
                return true
            }
        }
        XCTAssertEqual(sending.wait(timeout: .now() + 5), .success)

        // Read from a third thread so a regression that holds the lock across
        // `send` fails this test instead of deadlocking the runner.
        let read = DispatchSemaphore(value: 0)
        DispatchQueue.global().async {
            _ = queue.totalCount()
            _ = queue.recipientIds()
            read.signal()
        }

        XCTAssertEqual(read.wait(timeout: .now() + 5), .success,
                       "snapshot reads must not wait on an in-progress send")
        release.signal()
    }
}
