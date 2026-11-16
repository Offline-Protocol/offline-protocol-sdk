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
        XCTAssertFalse(config.autoKeyExchange)
        XCTAssertFalse(config.storePending)
        XCTAssertFalse(config.requireEncryption)
        XCTAssertEqual(config.maxPendingPerPeer, 64)
        XCTAssertEqual(config.maxPendingGlobal, 4096)
    }

    func testEncryptFailedDiagnosticsFireOnce() throws {
        OfflineProtocolEncryptFailedDiagnostics.resetForTests()
        var lines: [String] = []
        let error = ProtocolError.EncryptFailed(message: "boom")
        // MLS initialized: the error has another cause, so nothing is logged
        // and the once-guard is not spent.
        OfflineProtocolEncryptFailedDiagnostics.logFirstRemedyIfNeeded(
            context: "sendMessage",
            error: error,
            mlsInitialized: true,
            sink: { lines.append($0) }
        )
        XCTAssertTrue(lines.isEmpty)
        for _ in 0..<2 {
            OfflineProtocolEncryptFailedDiagnostics.logFirstRemedyIfNeeded(
                context: "sendMessage",
                error: error,
                mlsInitialized: false,
                sink: { lines.append($0) }
            )
        }
        XCTAssertEqual(lines.count, 1)
        XCTAssertTrue(lines[0].contains("OfflineProtocolDevelopmentCleartextConfig"))
        XCTAssertTrue(lines[0].contains("OfflineProtocolReferenceStorage.initializeMls"))
    }

    func testOnlyEncryptFailedIsClassifiedAsEncryptFailed() {
        XCTAssertTrue(OfflineProtocolEncryptFailedDiagnostics.isEncryptFailed(ProtocolError.EncryptFailed(message: "x")))
        XCTAssertFalse(OfflineProtocolEncryptFailedDiagnostics.isEncryptFailed(ProtocolError.InvalidArgument(message: "x")))
    }
}
