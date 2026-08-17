import XCTest
@testable import OfflineProtocol

/// Mirrors the Android module's ProtocolErrorBridgeTest.kt — the two
/// bridge mappers are hand-maintained twins and must expose the same
/// code for every typed variant.
final class ProtocolErrorBridgeTests: XCTestCase {

    func testTypedProtocolErrorsMapToStableBridgeCodes() {
        let cases: [(ProtocolError, String)] = [
            (.NoKeyPackage(message: "bob"), "NoKeyPackage"),
            (.SessionNotReady(message: "pending"), "SessionNotReady"),
            (.EncryptFailed(message: "boom"), "EncryptFailed"),
            (.MediaTransferLimit(message: "bob"), "MediaTransferLimit"),
            (.SendFailed(message: "all transports failed"), "SendFailed"),
            (.InvalidState(message: "cannot demote the last admin"), "InvalidState"),
            // resolveUsername raises this for "discovery is off", where a retry
            // can never succeed, beside InvalidState for "retry shortly". One
            // code for both is the difference between an app that stops and one
            // that spins.
            (.InvalidConfiguration(message: "username discovery is disabled"), "InvalidConfiguration"),
            (.MlsNotInitialized(message: "MLS not initialized"), "MlsNotInitialized"),
            (.TransportError(message: "ble unavailable"), "TransportError"),
            (.SerializationError(message: "bad json"), "SerializationError"),
            (.ServiceError(message: "no provider"), "ServiceError"),
            (.GroupNotFound(message: "group:missing"), "GroupNotFound"),
            (.PermissionDenied(message: "only admins can invite"), "PermissionDenied"),
            (.InvalidArgument(message: "group name cannot be empty"), "InvalidArgument"),
        ]
        for (error, expectedCode) in cases {
            let mapped = mapProtocolBridgeError(error)
            XCTAssertNotNil(mapped, "expected a mapping for \(expectedCode)")
            XCTAssertEqual(mapped?.code, expectedCode)
        }
    }

    func testMessageCarryingVariantsPassTheEngineMessageThrough() {
        let mapped = mapProtocolBridgeError(
            ProtocolError.GroupNotFound(message: "Group not found: group:x")
        )
        XCTAssertEqual(mapped?.message, "Group not found: group:x")
    }

    func testUnmappedErrorsReturnNilSoCallersKeepTheirLegacyCode() {
        XCTAssertNil(mapProtocolBridgeError(ProtocolError.Other(message: "misc")))
        XCTAssertNil(mapProtocolBridgeError(NSError(domain: "test", code: 1)))
    }
}
