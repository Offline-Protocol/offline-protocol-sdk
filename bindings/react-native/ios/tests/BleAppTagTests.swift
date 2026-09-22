//
// BleAppTagTests.swift
//
// Pins the APP_TAG value byte for byte. Mirrors android's BleAppTagTest;
// keep in sync: a central on one platform matches a tag computed on the
// other, so the two must agree on every byte.
//

import XCTest
@testable import OfflineProtocol

final class BleAppTagTests: XCTestCase {

    /// `SHA-256("offline-protocol-ble-app-tag-v1" || 0x00 || appId)[0..8]`,
    /// computed independently with `shasum -a 256`.
    func testVectorsMatchTheIndependentDigest() {
        XCTAssertEqual(hex(BleAppTag.compute(appId: "offline-messenger")), "2e2e6107567ff67b")
        XCTAssertEqual(hex(BleAppTag.compute(appId: "offline-demo")), "3a88052657e598e1")
        XCTAssertEqual(hex(BleAppTag.compute(appId: "")), "d5c9ed34fbf6503f")
    }

    func testTagIsEightBytes() {
        XCTAssertEqual(BleAppTag.size, 8)
        XCTAssertEqual(BleAppTag.compute(appId: "offline-messenger").count, 8)
    }

    /// The domain separator is load-bearing: without it the tag would be the
    /// prefix of a plain digest of the app id (`ce3cfe35f7b6c523` for this one).
    func testTagIsDomainSeparated() {
        XCTAssertNotEqual(hex(BleAppTag.compute(appId: "offline-messenger")), "ce3cfe35f7b6c523")
    }

    func testDifferentAppsGetDifferentTags() {
        XCTAssertNotEqual(
            BleAppTag.compute(appId: "offline-messenger"),
            BleAppTag.compute(appId: "offline-demo")
        )
    }

    private func hex(_ data: Data) -> String {
        data.map { String(format: "%02x", $0) }.joined()
    }
}
