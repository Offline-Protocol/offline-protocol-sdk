//
// RelayTimestampsTests.swift
// OfflineProtocolTests
//
// Mirrors android/src/test/.../RelayTimestampsTest.kt — keep in sync.
//

import XCTest
@testable import OfflineProtocol

final class RelayTimestampsTests: XCTestCase {

    func testParsesEpochMilliseconds() {
        XCTAssertEqual(RelayTimestamps.parseToMsOrNull("1720000000000"), 1_720_000_000_000)
    }

    func testParsesIso8601WithFractionalSeconds() {
        XCTAssertEqual(
            RelayTimestamps.parseToMsOrNull("2024-01-01T00:00:00.500Z"),
            1_704_067_200_500
        )
    }

    func testParsesIso8601WithoutFractionalSeconds() {
        XCTAssertEqual(
            RelayTimestamps.parseToMsOrNull("2024-01-01T00:00:00Z"),
            1_704_067_200_000
        )
    }

    func testAbsentOrUnparseableReturnsNilInsteadOfInventingNow() {
        XCTAssertNil(RelayTimestamps.parseToMsOrNull(""))
        XCTAssertNil(RelayTimestamps.parseToMsOrNull("not-a-timestamp"))
        XCTAssertNil(RelayTimestamps.parseToMsOrNull("2024-13-45T99:99:99Z"))
    }
}
