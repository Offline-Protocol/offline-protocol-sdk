//
// WriteStallWatchdogTests.swift
//
// No Android counterpart: OkHttp's writeTimeout owns this on that side. These
// suites pin the iOS write-stall watchdog that replaces it (see
// WriteStallWatchdog.swift). Times are millisecond ints from the caller's
// monotonic clock; the tests drive them directly. Each write is tagged with the
// caller's socket generation (InternetManager stamps it on task.taskDescription);
// unless a test is exercising the cross-generation guard it uses a single
// generation `g1`.
//

import XCTest
@testable import OfflineProtocol

final class WriteStallWatchdogTests: XCTestCase {

    private let g1 = 1
    private let g2 = 2

    func testEmptyWatchdogNeverStalls() {
        let wd = WriteStallWatchdog(timeoutMs: 10_000)
        XCTAssertNil(wd.stalledAgeMs(nowMs: 1_000_000))
        XCTAssertEqual(wd.outstandingCount, 0)
    }

    func testWriteWithinTimeoutDoesNotStall() {
        let wd = WriteStallWatchdog(timeoutMs: 10_000)
        wd.arm(nowMs: 1_000, generation: g1)
        // 9.999s later — still inside the budget.
        XCTAssertNil(wd.stalledAgeMs(nowMs: 1_000 + 9_999))
        XCTAssertEqual(wd.outstandingCount, 1)
    }

    func testWriteExactlyAtTimeoutDoesNotStall() {
        // Boundary: strictly-greater-than, so age == timeout is still OK.
        let wd = WriteStallWatchdog(timeoutMs: 10_000)
        wd.arm(nowMs: 0, generation: g1)
        XCTAssertNil(wd.stalledAgeMs(nowMs: 10_000))
    }

    func testWritePastTimeoutStallsAndReportsAge() {
        let wd = WriteStallWatchdog(timeoutMs: 10_000)
        wd.arm(nowMs: 0, generation: g1)
        XCTAssertEqual(wd.stalledAgeMs(nowMs: 10_001), 10_001)
    }

    func testDisarmClearsTheStall() {
        let wd = WriteStallWatchdog(timeoutMs: 10_000)
        wd.arm(nowMs: 0, generation: g1)
        XCTAssertNotNil(wd.stalledAgeMs(nowMs: 20_000))
        wd.disarm(generation: g1)
        XCTAssertNil(wd.stalledAgeMs(nowMs: 20_000))
        XCTAssertEqual(wd.outstandingCount, 0)
    }

    func testOldestOutstandingWriteDrivesTheStall() {
        // A steady stream: the OLDEST still-outstanding write is what ages out,
        // not the most recent. Arm three, retire one, and the second becomes
        // the age reference.
        let wd = WriteStallWatchdog(timeoutMs: 10_000)
        wd.arm(nowMs: 0, generation: g1)      // #1
        wd.arm(nowMs: 5_000, generation: g1)  // #2
        wd.arm(nowMs: 9_000, generation: g1)  // #3
        // #1 is 10.5s old → stalled on #1's age.
        XCTAssertEqual(wd.stalledAgeMs(nowMs: 10_500), 10_500)
        // Retire #1; now #2 (5.5s old at 10.5s) is the reference → not stalled.
        wd.disarm(generation: g1)
        XCTAssertNil(wd.stalledAgeMs(nowMs: 10_500))
        XCTAssertEqual(wd.outstandingCount, 2)
    }

    func testHealthyStreamNeverStalls() {
        // Each write completes ~1ms after it is issued: arm then immediately
        // disarm, thousands of times. The watchdog must stay empty and quiet.
        let wd = WriteStallWatchdog(timeoutMs: 10_000)
        var now: Int64 = 0
        for _ in 0..<5_000 {
            wd.arm(nowMs: now, generation: g1)
            now += 1
            wd.disarm(generation: g1)
            XCTAssertNil(wd.stalledAgeMs(nowMs: now))
        }
        XCTAssertEqual(wd.outstandingCount, 0)
    }

    func testDisarmOnEmptyIsANoOp() {
        // A late cancelled completion after a reset must not underflow.
        let wd = WriteStallWatchdog(timeoutMs: 10_000)
        wd.disarm(generation: g1)
        wd.disarm(generation: g1)
        XCTAssertEqual(wd.outstandingCount, 0)
        wd.arm(nowMs: 0, generation: g1)
        XCTAssertEqual(wd.outstandingCount, 1)
    }

    func testResetDropsAllOutstanding() {
        let wd = WriteStallWatchdog(timeoutMs: 10_000)
        wd.arm(nowMs: 0, generation: g1)
        wd.arm(nowMs: 100, generation: g1)
        wd.arm(nowMs: 200, generation: g1)
        XCTAssertEqual(wd.outstandingCount, 3)
        wd.reset()
        XCTAssertEqual(wd.outstandingCount, 0)
        XCTAssertNil(wd.stalledAgeMs(nowMs: 1_000_000))
    }

    func testResetThenLateDisarmsAreHarmless() {
        // teardown resets while sends are outstanding; their cancelled
        // completions then disarm. Those disarms must not drop a fresh
        // connection's freshly-armed writes below zero or wedge state.
        let wd = WriteStallWatchdog(timeoutMs: 10_000)
        wd.arm(nowMs: 0, generation: g1)
        wd.arm(nowMs: 100, generation: g1)
        wd.reset()
        // Two late cancelled completions from the dead socket:
        wd.disarm(generation: g1)
        wd.disarm(generation: g1)
        // New connection arms fresh; the stale disarms above did no damage.
        wd.arm(nowMs: 50_000, generation: g1)
        XCTAssertEqual(wd.outstandingCount, 1)
        XCTAssertNil(wd.stalledAgeMs(nowMs: 50_000 + 9_999))
        XCTAssertEqual(wd.stalledAgeMs(nowMs: 50_000 + 10_001), 10_001)
    }

    func testStaleDisarmCannotPopLiveSuccessorSlot() {
        // The cross-generation guard. A torn-down socket (g1) left writes
        // outstanding; teardown reset the FIFO, a fresh socket (g2) connected
        // and armed a write — and ONLY THEN the old g1 completions arrive (a
        // cancelled completion the OS delivered late). Those g1 disarms must
        // find no g1 entry and leave g2's live write intact, or the watchdog
        // would under-count and miss a genuine g2 stall.
        let wd = WriteStallWatchdog(timeoutMs: 10_000)
        wd.arm(nowMs: 0, generation: g1)
        wd.reset()                              // teardown of g1
        wd.arm(nowMs: 1_000, generation: g2)    // fresh socket's first write
        // Late cancelled completions from the dead g1 socket:
        wd.disarm(generation: g1)
        wd.disarm(generation: g1)
        // g2's write survived and still ages out on its own clock.
        XCTAssertEqual(wd.outstandingCount, 1)
        XCTAssertNil(wd.stalledAgeMs(nowMs: 1_000 + 10_000))
        XCTAssertEqual(wd.stalledAgeMs(nowMs: 1_000 + 10_001), 10_001)
    }

    func testDisarmRetiresOnlyItsOwnGeneration() {
        // Even without a reset between them (defense in depth): if two
        // generations coexist, a disarm retires the oldest of its OWN
        // generation, never another's.
        let wd = WriteStallWatchdog(timeoutMs: 10_000)
        wd.arm(nowMs: 0, generation: g1)
        wd.arm(nowMs: 100, generation: g2)
        wd.disarm(generation: g2)               // retires the g2 entry only
        XCTAssertEqual(wd.outstandingCount, 1)
        // The surviving g1 entry (the head) still drives the stall.
        XCTAssertEqual(wd.stalledAgeMs(nowMs: 10_001), 10_001)
    }

    func testDefaultTimeoutMatchesKotlinWriteTimeout() {
        // Parity anchor: the default equals the Kotlin bridge's OkHttp
        // writeTimeout / CONNECTION_TIMEOUT_MS (10s).
        XCTAssertEqual(WriteStallWatchdog.defaultTimeoutMs, 10_000)
        let wd = WriteStallWatchdog()
        wd.arm(nowMs: 0, generation: g1)
        XCTAssertNil(wd.stalledAgeMs(nowMs: 10_000))
        XCTAssertNotNil(wd.stalledAgeMs(nowMs: 10_001))
    }
}
