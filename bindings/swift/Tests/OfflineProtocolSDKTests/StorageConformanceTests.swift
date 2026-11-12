import XCTest
@testable import OfflineProtocolSDK

final class StorageConformanceTests: XCTestCase {
  func testRunDecodesAGreenReportFromTheReferenceStore() throws {
    let profile = UUID().uuidString
    let report = try StorageConformance.verifyReferenceProtocolStateStorage(
      appId: "storage-conformance-tests",
      profile: profile
    )
    XCTAssertTrue(report.isGreen, "failures: \(report.failures)")
    XCTAssertFalse(report.passed.isEmpty)
  }
}
