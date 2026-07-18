//
// ForcedPresenceCheckQueueTests.swift
// Mirrors android/src/test/java/com/offlineprotocol/ForcedPresenceCheckQueueTest.kt — keep in sync.
//
// The queue only ever sees checks that could not be sent right now; a
// sendable check completes in the manager without touching it. These
// tests pin the decision policy: park before the deadline, fail fast on a
// stopping/stopped transport, expire at/past the deadline, reject at
// capacity, and resolve every completion exactly once.
//

import XCTest
@testable import OfflineProtocol

final class ForcedPresenceCheckQueueTests: XCTestCase {

    private final class Recorder {
        var invocations = 0
        var lastResult: Bool?
        func completion(_ result: Bool) {
            invocations += 1
            lastResult = result
        }
    }

    private func entry(_ recorder: Recorder, deadlineMs: Int64, userId: String = "peer") -> ForcedPresenceCheckQueue.Entry {
        ForcedPresenceCheckQueue.Entry(userId: userId, deadlineMs: deadlineMs, completion: recorder.completion)
    }

    func testParksBeforeTheDeadlineWithoutResolving() {
        let queue = ForcedPresenceCheckQueue()
        let recorder = Recorder()
        XCTAssertTrue(queue.parkOrExpire(entry(recorder, deadlineMs: 8_000), transportStopped: false, nowMs: 1_000))
        XCTAssertEqual(recorder.invocations, 0)
        XCTAssertFalse(queue.isEmpty)
    }

    func testFailsFastOnAStoppedTransport() {
        let queue = ForcedPresenceCheckQueue()
        let recorder = Recorder()
        // Even far from the deadline: no reconnect is coming.
        XCTAssertFalse(queue.parkOrExpire(entry(recorder, deadlineMs: 8_000), transportStopped: true, nowMs: 1_000))
        XCTAssertEqual(recorder.invocations, 1)
        XCTAssertEqual(recorder.lastResult, false)
        XCTAssertTrue(queue.isEmpty)
    }

    func testExpiresExactlyAtTheDeadline() {
        let queue = ForcedPresenceCheckQueue()
        let recorder = Recorder()
        XCTAssertFalse(queue.parkOrExpire(entry(recorder, deadlineMs: 8_000), transportStopped: false, nowMs: 8_000))
        XCTAssertEqual(recorder.invocations, 1)
        XCTAssertEqual(recorder.lastResult, false)
        XCTAssertTrue(queue.isEmpty)
    }

    func testExpiresPastTheDeadline() {
        let queue = ForcedPresenceCheckQueue()
        let recorder = Recorder()
        XCTAssertFalse(queue.parkOrExpire(entry(recorder, deadlineMs: 8_000), transportStopped: false, nowMs: 9_500))
        XCTAssertEqual(recorder.invocations, 1)
        XCTAssertEqual(recorder.lastResult, false)
    }

    func testRejectsNewEntriesAtCapacityWithoutEvictingParkedOnes() {
        let queue = ForcedPresenceCheckQueue(capacity: 2)
        let first = Recorder()
        let second = Recorder()
        let third = Recorder()
        XCTAssertTrue(queue.parkOrExpire(entry(first, deadlineMs: 8_000, userId: "a"), transportStopped: false, nowMs: 0))
        XCTAssertTrue(queue.parkOrExpire(entry(second, deadlineMs: 8_000, userId: "b"), transportStopped: false, nowMs: 0))
        XCTAssertFalse(queue.parkOrExpire(entry(third, deadlineMs: 8_000, userId: "c"), transportStopped: false, nowMs: 0))
        XCTAssertEqual(third.invocations, 1)
        XCTAssertEqual(third.lastResult, false)
        // The parked entries survive, in arrival order.
        XCTAssertEqual(queue.takeAll().map { $0.userId }, ["a", "b"])
        XCTAssertEqual(first.invocations, 0)
        XCTAssertEqual(second.invocations, 0)
    }

    func testTakeAllEmptiesTheQueueAndFreesCapacity() {
        let queue = ForcedPresenceCheckQueue(capacity: 1)
        let first = Recorder()
        let second = Recorder()
        XCTAssertTrue(queue.parkOrExpire(entry(first, deadlineMs: 8_000, userId: "a"), transportStopped: false, nowMs: 0))
        XCTAssertEqual(queue.takeAll().count, 1)
        XCTAssertTrue(queue.isEmpty)
        // A serviced (taken) entry no longer occupies its slot.
        XCTAssertTrue(queue.parkOrExpire(entry(second, deadlineMs: 8_000, userId: "b"), transportStopped: false, nowMs: 0))
    }

    func testDrainAllResolvesEveryEntryFalseExactlyOnce() {
        let queue = ForcedPresenceCheckQueue()
        let recorders = (0..<3).map { _ in Recorder() }
        for (i, recorder) in recorders.enumerated() {
            XCTAssertTrue(queue.parkOrExpire(entry(recorder, deadlineMs: 8_000, userId: "peer\(i)"), transportStopped: false, nowMs: 0))
        }
        queue.drainAll()
        XCTAssertTrue(queue.isEmpty)
        for recorder in recorders {
            XCTAssertEqual(recorder.invocations, 1)
            XCTAssertEqual(recorder.lastResult, false)
        }
        // Idempotent on an empty queue.
        queue.drainAll()
        for recorder in recorders {
            XCTAssertEqual(recorder.invocations, 1)
        }
    }

    func testCompletionFiresExactlyOnceAcrossParkTakeReparkAndDrain() {
        let queue = ForcedPresenceCheckQueue()
        let recorder = Recorder()
        let check = entry(recorder, deadlineMs: 8_000)
        // Park, get serviced, still unsendable, re-park (the retry-tick
        // lifecycle), then the transport stops.
        XCTAssertTrue(queue.parkOrExpire(check, transportStopped: false, nowMs: 0))
        let taken = queue.takeAll()
        XCTAssertEqual(taken.count, 1)
        XCTAssertEqual(recorder.invocations, 0)
        XCTAssertTrue(queue.parkOrExpire(taken[0], transportStopped: false, nowMs: 4_000))
        queue.drainAll()
        XCTAssertEqual(recorder.invocations, 1)
        XCTAssertEqual(recorder.lastResult, false)
    }
}
