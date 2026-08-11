//
// InboundFragmentBufferTests.swift
// Mirrors android/src/test/java/com/offlineprotocol/ble/InboundFragmentBufferTest.kt
//

import XCTest
@testable import OfflineProtocol

final class InboundFragmentBufferTests: XCTestCase {

    private let peer = UUID()
    private let other = UUID()

    private func bytes(_ n: UInt8) -> Data { Data([n]) }

    // MARK: - Ordering

    func testDrainReturnsFragmentsInFifoOrder() {
        let buffer = InboundFragmentBuffer()
        buffer.enqueue(peer, bytes(1))
        buffer.enqueue(peer, bytes(2))
        buffer.enqueue(peer, bytes(3))

        XCTAssertEqual(buffer.drain(peer), [bytes(1), bytes(2), bytes(3)])
        // Drained buckets are removed, not emptied.
        XCTAssertTrue(buffer.drain(peer).isEmpty)
        XCTAssertFalse(buffer.hasPending(peer))
    }

    func testDrainIsolatesPeers() {
        let buffer = InboundFragmentBuffer()
        buffer.enqueue(peer, bytes(1))
        buffer.enqueue(other, bytes(9))

        XCTAssertEqual(buffer.drain(peer), [bytes(1)])
        XCTAssertEqual(buffer.drain(other), [bytes(9)])
    }

    // MARK: - Overflow

    func testOverflowDropsTheWholeBufferNotJustTheOldest() {
        var dropped: [(UUID, InboundFragmentBuffer.DropReason, Int)] = []
        let buffer = InboundFragmentBuffer(
            maxPerPeer: 3,
            onDropped: { dropped.append(($0, $1, $2)) }
        )
        buffer.enqueue(peer, bytes(1))
        buffer.enqueue(peer, bytes(2))
        buffer.enqueue(peer, bytes(3))
        // Fourth fragment hits the cap: the buffer is cleared, then appended to.
        buffer.enqueue(peer, bytes(4))

        // Dropping only the oldest would leave [2, 3, 4] — orphan slices the
        // reassembler would stitch into garbage. Whole-buffer drop is the point.
        XCTAssertEqual(buffer.drain(peer), [bytes(4)])
        XCTAssertEqual(dropped.count, 1)
        XCTAssertEqual(dropped.first?.2, 3)
        if case .cappedPerPeer = dropped.first?.1 {} else {
            XCTFail("expected .cappedPerPeer, got \(String(describing: dropped.first?.1))")
        }
    }

    // MARK: - enqueueIfPending

    func testEnqueueIfPendingDeclinesWhenNothingBuffered() {
        let buffer = InboundFragmentBuffer()
        XCTAssertFalse(buffer.enqueueIfPending(peer, bytes(1)))
        XCTAssertEqual(buffer.totalCount(), 0)
    }

    func testEnqueueIfPendingAppendsBehindExistingFragments() {
        let buffer = InboundFragmentBuffer()
        buffer.enqueue(peer, bytes(1))

        XCTAssertTrue(buffer.enqueueIfPending(peer, bytes(2)))
        XCTAssertEqual(buffer.drain(peer), [bytes(1), bytes(2)])
    }

    // MARK: - Expiry

    func testEvictExpiredKeysOnNewestFragmentSoArrivingMessagesSurvive() {
        var now = Date(timeIntervalSince1970: 0)
        let buffer = InboundFragmentBuffer(timeout: 15, clock: { now })

        buffer.enqueue(peer, bytes(1))          // t=0, would be stale on its own
        now = now.addingTimeInterval(14)
        buffer.enqueue(peer, bytes(2))          // t=14, keeps the buffer alive
        now = now.addingTimeInterval(14)        // t=28: oldest is 28s old, newest 14s

        XCTAssertEqual(buffer.evictExpired(), 0)
        // The whole message survives — a per-fragment sweep would have dropped
        // slice 0 here and handed the reassembler a permanent hole.
        XCTAssertEqual(buffer.drain(peer), [bytes(1), bytes(2)])
    }

    func testEvictExpiredDropsSilentPeerWholesale() {
        var now = Date(timeIntervalSince1970: 0)
        var dropped: [(UUID, InboundFragmentBuffer.DropReason, Int)] = []
        let buffer = InboundFragmentBuffer(
            timeout: 15,
            clock: { now },
            onDropped: { dropped.append(($0, $1, $2)) }
        )

        buffer.enqueue(peer, bytes(1))
        buffer.enqueue(peer, bytes(2))
        now = now.addingTimeInterval(15)

        XCTAssertEqual(buffer.evictExpired(), 2)
        XCTAssertTrue(buffer.drain(peer).isEmpty)
        XCTAssertEqual(dropped.count, 1)
        XCTAssertEqual(dropped.first?.2, 2)
        if case .expired = dropped.first?.1 {} else {
            XCTFail("expected .expired, got \(String(describing: dropped.first?.1))")
        }
    }

    func testEvictExpiredLeavesFreshPeersAlone() {
        var now = Date(timeIntervalSince1970: 0)
        let buffer = InboundFragmentBuffer(timeout: 15, clock: { now })

        buffer.enqueue(peer, bytes(1))
        now = now.addingTimeInterval(15)
        buffer.enqueue(other, bytes(2))

        XCTAssertEqual(buffer.evictExpired(), 1)
        XCTAssertTrue(buffer.drain(peer).isEmpty)
        XCTAssertEqual(buffer.drain(other), [bytes(2)])
    }

    // MARK: - Removal

    func testRemoveAllReturnsDroppedCount() {
        let buffer = InboundFragmentBuffer()
        buffer.enqueue(peer, bytes(1))
        buffer.enqueue(peer, bytes(2))

        XCTAssertEqual(buffer.removeAll(peer), 2)
        XCTAssertEqual(buffer.removeAll(peer), 0)
    }

    func testClearDropsEveryBuffer() {
        let buffer = InboundFragmentBuffer()
        buffer.enqueue(peer, bytes(1))
        buffer.enqueue(other, bytes(2))

        buffer.clear()
        XCTAssertEqual(buffer.totalCount(), 0)
        XCTAssertTrue(buffer.pendingIds().isEmpty)
    }

    // MARK: - Cross-thread reads

    func testSnapshotReadsReflectBufferedState() {
        let buffer = InboundFragmentBuffer()
        XCTAssertEqual(buffer.totalCount(), 0)
        XCTAssertTrue(buffer.pendingIds().isEmpty)

        buffer.enqueue(peer, bytes(1))
        buffer.enqueue(peer, bytes(2))
        buffer.enqueue(other, bytes(3))

        XCTAssertEqual(buffer.totalCount(), 3)
        XCTAssertEqual(Set(buffer.pendingIds()), Set([peer, other]))
        XCTAssertTrue(buffer.hasPending(peer))
        XCTAssertFalse(buffer.hasPending(UUID()))
    }

    /// The reason this class exists: readers on other threads must not have to
    /// hop onto the owning queue, because that queue performs UniFFI calls that
    /// can block for seconds (OFF-2123). Here the "owning queue" is a real
    /// serial queue and the reader is the caller's thread.
    func testSnapshotReadsDoNotBlockOnTheOwningQueue() {
        let owningQueue = DispatchQueue(label: "test.owning")
        let buffer = InboundFragmentBuffer(
            queueCheck: { dispatchPrecondition(condition: .onQueue(owningQueue)) }
        )
        let occupied = DispatchSemaphore(value: 0)
        let release = DispatchSemaphore(value: 0)

        owningQueue.async {
            buffer.enqueue(self.peer, self.bytes(1))
            occupied.signal()
            release.wait()          // hold the queue, as a slow UniFFI call would
        }
        XCTAssertEqual(occupied.wait(timeout: .now() + 5), .success)

        // Read from a third thread so a regression that reintroduces a hop onto
        // the owning queue fails this test instead of deadlocking the runner.
        let read = DispatchSemaphore(value: 0)
        var total = -1
        var ids: [UUID] = []
        DispatchQueue.global().async {
            total = buffer.totalCount()
            ids = buffer.pendingIds()
            read.signal()
        }

        XCTAssertEqual(read.wait(timeout: .now() + 5), .success,
                       "snapshot reads must not wait on the owning queue")
        XCTAssertEqual(total, 1)
        XCTAssertEqual(ids, [peer])

        release.signal()
    }
}
