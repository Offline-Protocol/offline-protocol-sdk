import XCTest
@testable import OfflineProtocolSDK

final class ReferenceStorageTests: XCTestCase {
  func testReferenceProtocolStateStoragePassesConformance() throws {
    let profile = UUID().uuidString
    let report = try StorageConformance.verifyReferenceProtocolStateStorage(
      appId: "reference-storage-tests",
      profile: profile
    )
    XCTAssertTrue(report.isGreen, "failures: \(report.failures)")
  }
}
