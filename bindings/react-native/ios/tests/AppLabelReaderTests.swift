import XCTest
@testable import OfflineProtocol

/// Mirrors android/ `ProtocolConfigParserTest`'s app label cases, and keeps
/// the two suites in sync.
final class AppLabelReaderTests: XCTestCase {

    private func read(_ json: String) throws -> String {
        let data = try XCTUnwrap(json.data(using: .utf8))
        let raw = try XCTUnwrap(
            JSONSerialization.jsonObject(with: data) as? [String: Any]
        )
        return try AppLabelReader.read(raw)
    }

    func testTheLabelIsReadUnderItsNewAndOldNames() throws {
        XCTAssertEqual(try read(#"{"appLabel":"app.muster"}"#), "app.muster")
        XCTAssertEqual(try read(#"{"appId":"app.muster"}"#), "app.muster")
        XCTAssertEqual(try read(#"{"app_id":"app.muster"}"#), "app.muster")
        XCTAssertEqual(try read(#"{"appLabel":"same","appId":"same"}"#), "same")
        XCTAssertEqual(try read(#"{}"#), "")
    }

    func testNamesThatDisagreeAreRefused() {
        XCTAssertThrowsError(try read(#"{"appLabel":"new","appId":"old"}"#)) { error in
            XCTAssertTrue(error.localizedDescription.contains("disagree"), "\(error)")
        }
    }
}
