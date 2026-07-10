//
// RelayRateLimiterTests.swift
// Mirrors android/src/test/java/com/offlineprotocol/RelayRateLimiterTest.kt — keep in sync.
//

import XCTest
@testable import OfflineProtocol

final class RelayRateLimiterTests: XCTestCase {

    func testAllowsTheFullBurstThenDefers() {
        let limiter = RelayRateLimiter(capacity: 5, refillPerSecond: 1.0)
        let t0: Int64 = 1_000
        for _ in 0..<5 {
            XCTAssertTrue(limiter.tryAcquire(nowMs: t0))
        }
        XCTAssertFalse(limiter.tryAcquire(nowMs: t0))
    }

    func testRefillsAtTheConfiguredRate() {
        let limiter = RelayRateLimiter(capacity: 5, refillPerSecond: 2.0)
        let t0: Int64 = 1_000
        for _ in 0..<5 {
            XCTAssertTrue(limiter.tryAcquire(nowMs: t0))
        }
        XCTAssertFalse(limiter.tryAcquire(nowMs: t0))
        // 500ms at 2 tokens/s = exactly 1 token.
        XCTAssertTrue(limiter.tryAcquire(nowMs: t0 + 500))
        XCTAssertFalse(limiter.tryAcquire(nowMs: t0 + 500))
    }

    func testRefillNeverExceedsCapacity() {
        let limiter = RelayRateLimiter(capacity: 2, refillPerSecond: 1.0)
        let t0: Int64 = 1_000
        XCTAssertTrue(limiter.tryAcquire(nowMs: t0))
        // A long quiet period refills to capacity, not beyond.
        let later = t0 + 3_600_000
        for _ in 0..<2 {
            XCTAssertTrue(limiter.tryAcquire(nowMs: later))
        }
        XCTAssertFalse(limiter.tryAcquire(nowMs: later))
    }

    func testRefundReturnsAnUnusedToken() {
        let limiter = RelayRateLimiter(capacity: 1, refillPerSecond: 1.0)
        let t0: Int64 = 1_000
        XCTAssertTrue(limiter.tryAcquire(nowMs: t0))
        XCTAssertFalse(limiter.tryAcquire(nowMs: t0))
        limiter.refund()
        XCTAssertTrue(limiter.tryAcquire(nowMs: t0))
    }

    func testDrainEmptiesTheBucketAndRefillResumes() {
        let limiter = RelayRateLimiter(capacity: 5, refillPerSecond: 1.0)
        let t0: Int64 = 1_000
        limiter.drain(nowMs: t0)
        XCTAssertFalse(limiter.tryAcquire(nowMs: t0))
        XCTAssertTrue(limiter.tryAcquire(nowMs: t0 + 1_000))
    }

    func testClockGoingBackwardsDoesNotMintTokens() {
        let limiter = RelayRateLimiter(capacity: 1, refillPerSecond: 1000.0)
        let t0: Int64 = 10_000
        XCTAssertTrue(limiter.tryAcquire(nowMs: t0))
        XCTAssertFalse(limiter.tryAcquire(nowMs: t0 - 5_000))
    }

    func testDefaultsSitUnderTheRelayBudget() {
        // The relay's documented bucket is 30 burst / 10 per second; the
        // client mirror must leave headroom for unmetered frames (auth).
        XCTAssertLessThan(RelayRateLimiter.defaultCapacity, 30)
        XCTAssertLessThan(RelayRateLimiter.defaultRefillPerSecond, 10.0)
    }
}
