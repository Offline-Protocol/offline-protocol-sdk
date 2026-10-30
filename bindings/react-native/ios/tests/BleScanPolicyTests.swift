//
// BleScanPolicyTests.swift
//
// Pins what a scan names and when its silence may be read as a stall, by app
// state (see BleScanPolicy.swift). Android pins its screen-state rules in
// BleScanPolicyTest.kt: the same filter, and a slow restart that never
// escalates instead of none.
//

import XCTest
@testable import OfflineProtocol

final class BleScanPolicyTests: XCTestCase {

    func testForegroundScanNamesNoService() {
        XCTAssertNil(BleScanPolicy.services("mesh", inBackground: false))
    }

    func testBackgroundScanNamesTheMeshService() {
        // A filterless scan in the background delivers nothing at all.
        XCTAssertEqual(BleScanPolicy.services("mesh", inBackground: true), ["mesh"])
    }

    func testQuietForegroundScanMayBeRestarted() {
        XCTAssertTrue(BleScanPolicy.mayRestartQuietScan(inBackground: false))
    }

    func testQuietBackgroundScanIsNeverRestarted() {
        // Reading this silence as a stall is what escalated to the central
        // manager replacement that left Bluetooth `.unsupported` for good.
        XCTAssertFalse(BleScanPolicy.mayRestartQuietScan(inBackground: true))
    }
}
