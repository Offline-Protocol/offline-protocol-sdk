//
// ForegroundReconnectPolicyTests.swift
//
// Pins the shared foreground-reconnect gate (see ForegroundReconnectPolicy.swift).
// The Android side has a paired ForegroundReconnectPolicyTest.kt asserting the
// same rule. Times are millisecond ints from the caller's monotonic clock; the
// tests drive them directly.
//

import XCTest
@testable import OfflineProtocol

final class ForegroundReconnectPolicyTests: XCTestCase {

    func testColdLaunchForegroundDoesNotReconnect() {
        // No background was ever recorded → foreground must not reconnect.
        let policy = ForegroundReconnectPolicy(minBackgroundIntervalMs: 4_000)
        XCTAssertFalse(policy.shouldReconnectOnForeground(nowMs: 1_000_000))
    }

    func testBackgroundBelowThresholdDoesNotReconnect() {
        // Quick app-switch: 3.999s away → keep the live socket.
        let policy = ForegroundReconnectPolicy(minBackgroundIntervalMs: 4_000)
        policy.didEnterBackground(nowMs: 1_000)
        XCTAssertFalse(policy.shouldReconnectOnForeground(nowMs: 1_000 + 3_999))
    }

    func testBackgroundExactlyAtThresholdReconnects() {
        // Boundary: at-or-above the window reconnects (>= threshold).
        let policy = ForegroundReconnectPolicy(minBackgroundIntervalMs: 4_000)
        policy.didEnterBackground(nowMs: 0)
        XCTAssertTrue(policy.shouldReconnectOnForeground(nowMs: 4_000))
    }

    func testBackgroundPastThresholdReconnects() {
        let policy = ForegroundReconnectPolicy(minBackgroundIntervalMs: 4_000)
        policy.didEnterBackground(nowMs: 0)
        XCTAssertTrue(policy.shouldReconnectOnForeground(nowMs: 60_000))
    }

    func testForegroundConsumesTheTimestamp() {
        // A second foreground with no intervening background must not re-fire,
        // even if wall time has moved well past the threshold.
        let policy = ForegroundReconnectPolicy(minBackgroundIntervalMs: 4_000)
        policy.didEnterBackground(nowMs: 0)
        XCTAssertTrue(policy.shouldReconnectOnForeground(nowMs: 10_000))
        XCTAssertFalse(policy.shouldReconnectOnForeground(nowMs: 20_000))
    }

    func testReArmingAfterConsumeReconnectsAgain() {
        // background → foreground (fires) → background → foreground (fires again).
        let policy = ForegroundReconnectPolicy(minBackgroundIntervalMs: 4_000)
        policy.didEnterBackground(nowMs: 0)
        XCTAssertTrue(policy.shouldReconnectOnForeground(nowMs: 5_000))
        policy.didEnterBackground(nowMs: 6_000)
        XCTAssertTrue(policy.shouldReconnectOnForeground(nowMs: 6_000 + 4_000))
    }

    func testSleepInclusiveElapsedIsHonoured() {
        // The caller supplies sleep-inclusive time; a large jump (device slept
        // in the background) counts toward the window like any other elapsed ms.
        let policy = ForegroundReconnectPolicy(minBackgroundIntervalMs: 4_000)
        policy.didEnterBackground(nowMs: 100)
        XCTAssertTrue(policy.shouldReconnectOnForeground(nowMs: 100 + 3_600_000))
    }

    func testDefaultThresholdIsFourSeconds() {
        XCTAssertEqual(ForegroundReconnectPolicy.defaultMinBackgroundIntervalMs, 4_000)
        let policy = ForegroundReconnectPolicy()
        policy.didEnterBackground(nowMs: 0)
        XCTAssertFalse(policy.shouldReconnectOnForeground(nowMs: 3_999))
        policy.didEnterBackground(nowMs: 0)
        XCTAssertTrue(policy.shouldReconnectOnForeground(nowMs: 4_000))
    }
}
