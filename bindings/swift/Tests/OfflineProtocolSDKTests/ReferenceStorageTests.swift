import XCTest
@testable import OfflineProtocolSDK

final class ReferenceStorageTests: XCTestCase {
  func testReferenceProtocolStateStoragePassesConformance() throws {
    let profile = UUID().uuidString
    let storage = try OfflineProtocolReferenceStorage.makeProtocolStateStorage(
      appId: "reference-storage-tests",
      profile: profile
    )

    let report = runStorageConformance(storage: storage)

    let data = try XCTUnwrap(report.data(using: .utf8))
    let json = try XCTUnwrap(
      JSONSerialization.jsonObject(with: data) as? [String: Any], "not an object: \(report)")
    let failures = try XCTUnwrap(json["failures"] as? [Any], "no failures key: \(report)")
    let passed = try XCTUnwrap(json["passed"] as? [Any], "no passed key: \(report)")
    XCTAssertTrue(failures.isEmpty, "conformance failures: \(failures)")
    XCTAssertFalse(passed.isEmpty, "the suite ran no checks: \(report)")
  }
}
