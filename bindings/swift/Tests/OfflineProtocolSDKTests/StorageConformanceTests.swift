//
// StorageConformanceTests.swift
//
// The typed report `StorageConformance` decodes from the suite's JSON. A
// green report is covered by ReferenceStorageTests; what is covered here is
// everything a green run never reaches.
//

import XCTest
@testable import OfflineProtocolSDK

final class StorageConformanceTests: XCTestCase {

    /// A failure keeps its check and its detail, and makes the report not
    /// green even though other checks passed.
    func testAFailingReportDecodesWithItsChecks() throws {
        let report = try StorageConformance.decode(
            #"{"passed":["store_then_load"],"failures":[{"check":"values_are_bytes","detail":"loaded 3 bytes, expected 256 (contents differ)"}]}"#)

        XCTAssertEqual(report.passed, ["store_then_load"])
        XCTAssertEqual(
            report.failures,
            [StorageConformanceFailure(
                check: "values_are_bytes", detail: "loaded 3 bytes, expected 256 (contents differ)")])
        XCTAssertFalse(report.isGreen)
    }

    /// A report with no failures and no passes ran nothing, so it is not green.
    func testAnEmptyReportIsNotGreen() throws {
        let report = try StorageConformance.decode(#"{"passed":[],"failures":[]}"#)

        XCTAssertFalse(report.isGreen)
    }

    /// A key the suite renamed is an error, never a report with no failures.
    func testARenamedKeyIsAnError() {
        XCTAssertThrowsError(try StorageConformance.decode(#"{"passed":["store_then_load"],"failed":[]}"#))
    }

    /// A backend that ignores the key type fails the suite end to end, through
    /// the callback interface, and the run stops at the isolation check
    /// rather than deleting what a merged listing names.
    func testABackendThatMergesKeyTypesIsNotSupported() throws {
        let report = try StorageConformance.run(on: KeyTypeBlindStorage())

        XCTAssertFalse(report.isGreen)
        XCTAssertEqual(report.failures.map(\.check), ["key_types_are_separate_namespaces"])
        XCTAssertEqual(report.passed, [])
    }
}

/// Stores every record under its id alone, so two key types share one
/// namespace: the defect that would make the suite's cleanup destructive.
private final class KeyTypeBlindStorage: ProtocolStateStorageProvider, @unchecked Sendable {
    private var records: [String: Data] = [:]
    private let lock = NSLock()

    func store(keyType: String, keyId: String, data: Data) throws {
        lock.lock()
        defer { lock.unlock() }
        records[keyId] = data
    }

    func load(keyType: String, keyId: String) throws -> Data? {
        lock.lock()
        defer { lock.unlock() }
        return records[keyId]
    }

    func delete(keyType: String, keyId: String) throws {
        lock.lock()
        defer { lock.unlock() }
        records[keyId] = nil
    }

    func listKeys(keyType: String) throws -> [String] {
        lock.lock()
        defer { lock.unlock() }
        return Array(records.keys)
    }
}
