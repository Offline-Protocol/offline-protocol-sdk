//
// DevelopmentCleartextConfigTests.swift
//

import XCTest
@testable import OfflineProtocolSDK

final class DevelopmentCleartextConfigTests: XCTestCase {

    func testDevelopmentCleartextMatchesReactNativePreset() {
        let config = OfflineProtocolDevelopmentCleartextConfig.make(appId: "app", profile: "user")
        XCTAssertTrue(config.bleEnabled)
        XCTAssertFalse(config.wifiDirectEnabled)
        XCTAssertFalse(config.internetEnabled)
        XCTAssertFalse(config.encryptionEnabled)
        XCTAssertFalse(config.requireEncryption)
    }

    func testEncryptFailedDiagnosticsFireOnce() throws {
        OfflineProtocolEncryptFailedDiagnostics.resetForTests()
        var lines: [String] = []
        let error = ProtocolError.EncryptFailed(message: "boom")
        OfflineProtocolEncryptFailedDiagnostics.logFirstRemedyIfNeeded(
            context: "sendMessage",
            error: error,
            sink: { lines.append($0) }
        )
        OfflineProtocolEncryptFailedDiagnostics.logFirstRemedyIfNeeded(
            context: "sendMessage",
            error: error,
            sink: { lines.append($0) }
        )
        XCTAssertEqual(lines.count, 1)
        XCTAssertTrue(lines[0].contains("OfflineProtocolDevelopmentCleartextConfig"))
    }
}
