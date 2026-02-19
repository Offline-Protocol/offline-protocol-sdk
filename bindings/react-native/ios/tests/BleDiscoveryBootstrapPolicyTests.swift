import XCTest
@testable import OfflineProtocol

final class BleDiscoveryBootstrapPolicyTests: XCTestCase {

    func testAllowsColdStartBootstrapWithStrongSignalAndMissingKeys() {
        let allowed = BleDiscoveryBootstrapPolicy.shouldAllowCandidate(
            isConnectable: true,
            currentConnectionCount: 0,
            maxConnectionsPerDevice: 4,
            estimatedVisiblePeerCount: 3,
            densePeerThreshold: 50,
            rssi: -64,
            hasAnyServiceKey: false,
            minRssiWithServiceKeys: -75,
            minRssiWithoutServiceKeys: -68,
            lastAttemptAt: nil,
            now: Date(),
            perDeviceCooldown: 12.0,
            recentBootstrapAttempts: 0,
            maxBootstrapAttemptsPerMinute: 4,
            recentConnectionAttempts: 0,
            maxConnectionAttemptsPerMinute: 6
        )

        XCTAssertTrue(allowed)
    }

    func testRejectsWeakSignalForUnknownCandidateWhenKeysMissing() {
        let allowed = BleDiscoveryBootstrapPolicy.shouldAllowCandidate(
            isConnectable: true,
            currentConnectionCount: 0,
            maxConnectionsPerDevice: 4,
            estimatedVisiblePeerCount: 4,
            densePeerThreshold: 50,
            rssi: -80,
            hasAnyServiceKey: false,
            minRssiWithServiceKeys: -75,
            minRssiWithoutServiceKeys: -68,
            lastAttemptAt: nil,
            now: Date(),
            perDeviceCooldown: 12.0,
            recentBootstrapAttempts: 0,
            maxBootstrapAttemptsPerMinute: 4,
            recentConnectionAttempts: 0,
            maxConnectionAttemptsPerMinute: 6
        )

        XCTAssertFalse(allowed)
    }

    func testRejectsWhenPerDeviceCooldownIsActive() {
        let now = Date()
        let allowed = BleDiscoveryBootstrapPolicy.shouldAllowCandidate(
            isConnectable: true,
            currentConnectionCount: 0,
            maxConnectionsPerDevice: 4,
            estimatedVisiblePeerCount: 2,
            densePeerThreshold: 50,
            rssi: -60,
            hasAnyServiceKey: true,
            minRssiWithServiceKeys: -75,
            minRssiWithoutServiceKeys: -68,
            lastAttemptAt: now.addingTimeInterval(-5.0),
            now: now,
            perDeviceCooldown: 12.0,
            recentBootstrapAttempts: 1,
            maxBootstrapAttemptsPerMinute: 4,
            recentConnectionAttempts: 1,
            maxConnectionAttemptsPerMinute: 6
        )

        XCTAssertFalse(allowed)
    }
}
