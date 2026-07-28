//
// WriteStallWatchdogTests.swift
//
// No Android counterpart: OkHttp's writeTimeout owns this on that side. These
// suites pin the iOS write-stall watchdog that replaces it (see
// WriteStallWatchdog.swift). Times are millisecond ints from the caller's
// monotonic clock; the tests drive them directly. Each `arm` returns the token
// that write's own completion must disarm with — the suites hold the tokens
// exactly as `sendWatched` holds one per in-flight send.
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
        _ = wd.arm(nowMs: 1_000)
        // 9.999s later — still inside the budget.
        XCTAssertNil(wd.stalledAgeMs(nowMs: 1_000 + 9_999))
        XCTAssertEqual(wd.outstandingCount, 1)
    }

    func testWriteExactlyAtTimeoutDoesNotStall() {
        // Boundary: strictly-greater-than, so age == timeout is still OK.
        let wd = WriteStallWatchdog(timeoutMs: 10_000)
        _ = wd.arm(nowMs: 0)
        XCTAssertNil(wd.stalledAgeMs(nowMs: 10_000))
    }

    func testWritePastTimeoutStallsAndReportsAge() {
        let wd = WriteStallWatchdog(timeoutMs: 10_000)
        _ = wd.arm(nowMs: 0)
        XCTAssertEqual(wd.stalledAgeMs(nowMs: 10_001), 10_001)
    }

    func testDisarmClearsTheStall() {
        let wd = WriteStallWatchdog(timeoutMs: 10_000)
        let t = wd.arm(nowMs: 0)
        XCTAssertNotNil(wd.stalledAgeMs(nowMs: 20_000))
        wd.disarm(t)
        XCTAssertNil(wd.stalledAgeMs(nowMs: 20_000))
        XCTAssertEqual(wd.outstandingCount, 0)
    }

    func testOldestOutstandingWriteDrivesTheStall() {
        // A steady stream: the OLDEST still-outstanding write is what ages out,
        // not the most recent. Arm three, retire the first, and the second
        // becomes the age reference.
        let wd = WriteStallWatchdog(timeoutMs: 10_000)
        let t1 = wd.arm(nowMs: 0)
        _ = wd.arm(nowMs: 5_000)
        _ = wd.arm(nowMs: 9_000)
        // #1 is 10.5s old → stalled on #1's age.
        XCTAssertEqual(wd.stalledAgeMs(nowMs: 10_500), 10_500)
        // Retire #1; now #2 (5.5s old at 10.5s) is the reference → not stalled.
        wd.disarm(t1)
        XCTAssertNil(wd.stalledAgeMs(nowMs: 10_500))
        XCTAssertEqual(wd.outstandingCount, 2)
    }

    func testOutOfOrderCompletionDoesNotReKeyTheStallOffAYoungerWrite() {
        // THE precision contract. A hung write (#1) is still outstanding when a
        // later write (#2) completes — URLSession makes no promise that send
        // completions fire in send order. #2's completion must retire #2's OWN
        // slot; retiring the oldest instead would discard #1's timestamp and
        // re-key the stall off #2's, delaying the teardown by the gap between
        // their send times (here 9s — nearly a second timeout).
        let wd = WriteStallWatchdog(timeoutMs: 10_000)
        _ = wd.arm(nowMs: 0)             // #1 — hangs, never completes
        let t2 = wd.arm(nowMs: 9_000)    // #2 — completes fast, out of order
        wd.disarm(t2)

        XCTAssertEqual(wd.outstandingCount, 1)
        // #1 still drives the stall, on its own clock: fires at 10s past 0,
        // not at 10s past 9_000.
        XCTAssertNil(wd.stalledAgeMs(nowMs: 10_000))
        XCTAssertEqual(wd.stalledAgeMs(nowMs: 10_001), 10_001)
    }

    func testHealthyStreamNeverStalls() {
        // Each write completes ~1ms after it is issued: arm then immediately
        // disarm, thousands of times. The watchdog must stay empty and quiet.
        let wd = WriteStallWatchdog(timeoutMs: 10_000)
        var now: Int64 = 0
        for _ in 0..<5_000 {
            let t = wd.arm(nowMs: now)
            now += 1
            wd.disarm(t)
            XCTAssertNil(wd.stalledAgeMs(nowMs: now))
        }
        XCTAssertEqual(wd.outstandingCount, 0)
    }

    func testDoubleDisarmOfTheSameTokenIsANoOp() {
        // Tokens are never reused, so a completion that somehow ran twice can
        // only fail to match — it must never pop a different write's slot.
        let wd = WriteStallWatchdog(timeoutMs: 10_000)
        let t1 = wd.arm(nowMs: 0)
        _ = wd.arm(nowMs: 100)
        wd.disarm(t1)
        wd.disarm(t1)
        XCTAssertEqual(wd.outstandingCount, 1)
        // The survivor is #2 (armed at 100), not #1.
        XCTAssertNil(wd.stalledAgeMs(nowMs: 10_100))
        XCTAssertEqual(wd.stalledAgeMs(nowMs: 10_101), 10_001)
    }

    func testResetDropsAllOutstanding() {
        let wd = WriteStallWatchdog(timeoutMs: 10_000)
        _ = wd.arm(nowMs: 0)
        _ = wd.arm(nowMs: 100)
        _ = wd.arm(nowMs: 200)
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
        let t1 = wd.arm(nowMs: 0)
        let t2 = wd.arm(nowMs: 100)
        wd.reset()
        // Two late cancelled completions from the dead socket:
        wd.disarm(t1)
        wd.disarm(t2)
        // New connection arms fresh; the stale disarms above did no damage.
        _ = wd.arm(nowMs: 50_000)
        XCTAssertEqual(wd.outstandingCount, 1)
        XCTAssertNil(wd.stalledAgeMs(nowMs: 50_000 + 9_999))
        XCTAssertEqual(wd.stalledAgeMs(nowMs: 50_000 + 10_001), 10_001)
    }

    func testStaleDisarmCannotPopLiveSuccessorSlot() {
        // The cross-connection guard, now carried by token identity alone. A
        // torn-down socket left a write outstanding; teardown reset the FIFO, a
        // fresh socket connected and armed a write — and ONLY THEN the old
        // completion arrives (a cancelled completion the OS delivered late).
        // Its token names an entry that no longer exists, so the live write
        // survives; otherwise the watchdog would under-count and miss a genuine
        // stall on the new socket.
        let wd = WriteStallWatchdog(timeoutMs: 10_000)
        let dead = wd.arm(nowMs: 0)
        wd.reset()                       // teardown
        _ = wd.arm(nowMs: 1_000)         // fresh socket's first write
        // Late cancelled completion from the dead socket:
        wd.disarm(dead)
        wd.disarm(dead)
        // The live write survived and still ages out on its own clock.
        XCTAssertEqual(wd.outstandingCount, 1)
        XCTAssertNil(wd.stalledAgeMs(nowMs: 1_000 + 10_000))
        XCTAssertEqual(wd.stalledAgeMs(nowMs: 1_000 + 10_001), 10_001)
    }

    func testTokensAreNeverReusedAcrossResets() {
        // The token counter deliberately survives reset(): a fresh connection's
        // first write must not be disarmable by the previous connection's
        // first-write completion.
        let wd = WriteStallWatchdog(timeoutMs: 10_000)
        let firstOfOldSocket = wd.arm(nowMs: 0)
        wd.reset()
        let firstOfNewSocket = wd.arm(nowMs: 1_000)
        XCTAssertNotEqual(firstOfOldSocket, firstOfNewSocket)
        wd.disarm(firstOfOldSocket)
        XCTAssertEqual(wd.outstandingCount, 1)
    }

    func testDefaultTimeoutMatchesKotlinWriteTimeout() {
        // Parity anchor: the default equals the Kotlin bridge's OkHttp
        // writeTimeout / CONNECTION_TIMEOUT_MS (10s).
        XCTAssertEqual(WriteStallWatchdog.defaultTimeoutMs, 10_000)
        let wd = WriteStallWatchdog()
        _ = wd.arm(nowMs: 0)
        XCTAssertNil(wd.stalledAgeMs(nowMs: 10_000))
        XCTAssertNotNil(wd.stalledAgeMs(nowMs: 10_001))
    }
}
