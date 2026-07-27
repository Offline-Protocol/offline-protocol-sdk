//
// WriteStallWatchdogTests.swift
//
// No Android counterpart: OkHttp's writeTimeout owns this on that side. These
// suites pin the iOS write-stall watchdog that replaces it (see
// WriteStallWatchdog.swift). Times are millisecond ints from the caller's
// monotonic clock; the tests drive them directly.
//

import XCTest
@testable import OfflineProtocol

final class WriteStallWatchdogTests: XCTestCase {

    func testEmptyWatchdogNeverStalls() {
        let wd = WriteStallWatchdog(timeoutMs: 10_000)
        XCTAssertNil(wd.stalledAgeMs(nowMs: 1_000_000))
        XCTAssertEqual(wd.outstandingCount, 0)
    }

    func testWriteWithinTimeoutDoesNotStall() {
        let wd = WriteStallWatchdog(timeoutMs: 10_000)
        wd.arm(nowMs: 1_000)
        // 9.999s later — still inside the budget.
        XCTAssertNil(wd.stalledAgeMs(nowMs: 1_000 + 9_999))
        XCTAssertEqual(wd.outstandingCount, 1)
    }

    func testWriteExactlyAtTimeoutDoesNotStall() {
        // Boundary: strictly-greater-than, so age == timeout is still OK.
        let wd = WriteStallWatchdog(timeoutMs: 10_000)
        wd.arm(nowMs: 0)
        XCTAssertNil(wd.stalledAgeMs(nowMs: 10_000))
    }

    func testWritePastTimeoutStallsAndReportsAge() {
        let wd = WriteStallWatchdog(timeoutMs: 10_000)
        wd.arm(nowMs: 0)
        XCTAssertEqual(wd.stalledAgeMs(nowMs: 10_001), 10_001)
    }

    func testDisarmClearsTheStall() {
        let wd = WriteStallWatchdog(timeoutMs: 10_000)
        wd.arm(nowMs: 0)
        XCTAssertNotNil(wd.stalledAgeMs(nowMs: 20_000))
        wd.disarm()
        XCTAssertNil(wd.stalledAgeMs(nowMs: 20_000))
        XCTAssertEqual(wd.outstandingCount, 0)
    }

    func testOldestOutstandingWriteDrivesTheStall() {
        // A steady stream: the OLDEST still-outstanding write is what ages out,
        // not the most recent. Arm three, retire one, and the second becomes
        // the age reference.
        let wd = WriteStallWatchdog(timeoutMs: 10_000)
        wd.arm(nowMs: 0)      // #1
        wd.arm(nowMs: 5_000)  // #2
        wd.arm(nowMs: 9_000)  // #3
        // #1 is 10.5s old → stalled on #1's age.
        XCTAssertEqual(wd.stalledAgeMs(nowMs: 10_500), 10_500)
        // Retire #1; now #2 (5.5s old at 10.5s) is the reference → not stalled.
        wd.disarm()
        XCTAssertNil(wd.stalledAgeMs(nowMs: 10_500))
        XCTAssertEqual(wd.outstandingCount, 2)
    }

    func testHealthyStreamNeverStalls() {
        // Each write completes ~1ms after it is issued: arm then immediately
        // disarm, thousands of times. The watchdog must stay empty and quiet.
        let wd = WriteStallWatchdog(timeoutMs: 10_000)
        var now: Int64 = 0
        for _ in 0..<5_000 {
            wd.arm(nowMs: now)
            now += 1
            wd.disarm()
            XCTAssertNil(wd.stalledAgeMs(nowMs: now))
        }
        XCTAssertEqual(wd.outstandingCount, 0)
    }

    func testDisarmOnEmptyIsANoOp() {
        // A late cancelled completion after a reset must not underflow.
        let wd = WriteStallWatchdog(timeoutMs: 10_000)
        wd.disarm()
        wd.disarm()
        XCTAssertEqual(wd.outstandingCount, 0)
        wd.arm(nowMs: 0)
        XCTAssertEqual(wd.outstandingCount, 1)
    }

    func testResetDropsAllOutstanding() {
        let wd = WriteStallWatchdog(timeoutMs: 10_000)
        wd.arm(nowMs: 0)
        wd.arm(nowMs: 100)
        wd.arm(nowMs: 200)
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
        wd.arm(nowMs: 0)
        wd.arm(nowMs: 100)
        wd.reset()
        // Two late cancelled completions from the dead socket:
        wd.disarm()
        wd.disarm()
        // New connection arms fresh; the stale disarms above did no damage.
        wd.arm(nowMs: 50_000)
        XCTAssertEqual(wd.outstandingCount, 1)
        XCTAssertNil(wd.stalledAgeMs(nowMs: 50_000 + 9_999))
        XCTAssertEqual(wd.stalledAgeMs(nowMs: 50_000 + 10_001), 10_001)
    }

    func testDefaultTimeoutMatchesKotlinWriteTimeout() {
        // Parity anchor: the default equals the Kotlin bridge's OkHttp
        // writeTimeout / CONNECTION_TIMEOUT_MS (10s).
        XCTAssertEqual(WriteStallWatchdog.defaultTimeoutMs, 10_000)
        let wd = WriteStallWatchdog()
        wd.arm(nowMs: 0)
        XCTAssertNil(wd.stalledAgeMs(nowMs: 10_000))
        XCTAssertNotNil(wd.stalledAgeMs(nowMs: 10_001))
    }
}
