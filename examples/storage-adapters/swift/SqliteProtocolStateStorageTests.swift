import XCTest

/// The gate every storage adapter must pass.
///
/// One suite, defined once in Rust and reachable from every binding, so an
/// adapter written here is held to exactly the same contract as one written
/// in Kotlin or Python.
final class SqliteProtocolStateStorageTests: XCTestCase {

    func testPassesTheStorageConformanceSuite() throws {
        let path = FileManager.default.temporaryDirectory
            .appendingPathComponent("conformance-\(UUID().uuidString).db")
            .path
        defer { try? FileManager.default.removeItem(atPath: path) }

        let storage = try SqliteProtocolStateStorage(path: path)
        let json = runStorageConformance(storage: storage)

        struct Failure: Decodable { let check: String; let detail: String }
        struct Report: Decodable { let passed: [String]; let failures: [Failure] }

        let report = try JSONDecoder().decode(
            Report.self, from: Data(json.utf8))
        let detail = report.failures
            .map { "\($0.check): \($0.detail)" }
            .joined(separator: "\n")
        XCTAssertTrue(report.failures.isEmpty, "conformance failures:\n\(detail)")
        XCTAssertFalse(report.passed.isEmpty, "the suite reported no checks")
    }
}
