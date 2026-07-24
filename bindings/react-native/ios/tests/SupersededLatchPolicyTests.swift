//
// SupersededLatchPolicyTests.swift
// Mirrors android/src/test/java/com/offlineprotocol/SupersededLatchPolicyTest.kt
//

import XCTest
@testable import OfflineProtocol

final class SupersededLatchPolicyTests: XCTestCase {

    func testClose4000LatchesWhenSocketIsCurrent() {
        let policy = SupersededLatchPolicy()
        // handleConnectionClosed reaches the decision only after the task was
        // detached, so hasNewerSuccessor is false there.
        XCTAssertTrue(policy.shouldMark(closeCode: 4000, hasNewerSuccessor: false))
    }

    func testNonSupersedeCloseDoesNotLatch() {
        let policy = SupersededLatchPolicy()
        XCTAssertFalse(policy.shouldMark(closeCode: 1000, hasNewerSuccessor: false))
        XCTAssertFalse(policy.shouldMark(closeCode: -1, hasNewerSuccessor: false))
        XCTAssertFalse(policy.shouldMark(closeCode: nil, hasNewerSuccessor: false))
    }

    func testNewerSuccessorSocketIsNeverLatchedByAStale4000() {
        // The cd9fa39 regression: old socket displaced → app re-enabled via
        // start() → new socket B up → a LATE 4000 for the bygone generation
        // must not re-latch and stop B.
        let policy = SupersededLatchPolicy()
        XCTAssertFalse(policy.shouldMark(closeCode: 4000, hasNewerSuccessor: true))
    }

    func testSuccessorGuardWinsEvenWhenAlreadyLatched() {
        let policy = SupersededLatchPolicy()
        policy.mark()
        // A stale latch bit must still never stop a live successor socket.
        XCTAssertFalse(policy.shouldMark(closeCode: 1000, hasNewerSuccessor: true))
    }

    func testOnceLatchedAnyCloseKeepsLatching() {
        // A non-4000 close arriving after a SessionSuperseded notice already
        // latched (on the live socket) must still stop, not reconnect.
        let policy = SupersededLatchPolicy()
        policy.mark()
        XCTAssertTrue(policy.shouldMark(closeCode: 1000, hasNewerSuccessor: false))
        XCTAssertTrue(policy.shouldMark(closeCode: nil, hasNewerSuccessor: false))
    }

    func testMarkIsIdempotentAndReportsOnlyTheFirstTransition() {
        // The relay emits a notice AND close 4000 (each fanning into several
        // terminal signals); the one-shot event must fire exactly once.
        let policy = SupersededLatchPolicy()
        XCTAssertTrue(policy.mark())   // false -> true: fire the event
        XCTAssertFalse(policy.mark())  // already latched: no re-fire
        XCTAssertFalse(policy.mark())
        XCTAssertTrue(policy.isSuperseded)
    }

    func testStartClearsTheLatchAndReArmsMark() {
        let policy = SupersededLatchPolicy()
        policy.mark()
        XCTAssertTrue(policy.isSuperseded)

        // A fresh start() clears it; a subsequent displacement fires again.
        policy.clear()
        XCTAssertFalse(policy.isSuperseded)
        XCTAssertFalse(policy.shouldMark(closeCode: 1000, hasNewerSuccessor: false))
        XCTAssertTrue(policy.mark())
    }

    func testCloseCodeConstantMatchesRelayContract() {
        XCTAssertEqual(SupersededLatchPolicy.SUPERSEDED_CLOSE_CODE, 4000)
    }
}
