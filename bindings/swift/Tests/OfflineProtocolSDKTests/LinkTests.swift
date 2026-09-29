//
// LinkTests.swift
//
// What only a linked build can show: that the package's Swift, its binary and
// its resources are one artifact.
//
// The bridge's own suites run against stand-ins for the generated types, in a
// harness that cannot link the Rust library. They prove the bridge's logic.
// They cannot prove that the generated Swift in this package was generated
// from the library in this package, that the header reached the compiler, or
// that a store written in Swift satisfies the contract the core holds it to.
// Each test here fails on one of those.
//

import XCTest
@testable import OfflineProtocolSDK

final class LinkTests: XCTestCase {

    /// The first call into the library checks the contract version and every
    /// function's checksum, and traps on a mismatch. So this passing is the
    /// statement that the Swift and the binary were generated together.
    func testAGeneratedCallCrossesIntoTheLibrary() throws {
        let key = [UInt8](repeating: 7, count: 32)

        let address = try deriveAddress(publicKey: key)

        // The derivation is pinned by the conformance vectors in Rust. What
        // is checked here is that an answer came back through the boundary
        // and is the same answer twice.
        XCTAssertTrue(address.hasPrefix("off1"), "not an address: \(address)")
        XCTAssertEqual(address, try deriveAddress(publicKey: key))
        XCTAssertNotEqual(
            address, try deriveAddress(publicKey: [UInt8](repeating: 8, count: 32)))
    }

    /// An error raised in Rust arrives as the generated Swift error, not as a
    /// trap. A key of the wrong length is the smallest input that raises one.
    func testAnErrorCrossesBackAsASwiftError() {
        XCTAssertThrowsError(try deriveAddress(publicKey: [1, 2, 3])) { error in
            XCTAssertTrue(error is ProtocolError, "unexpected error type: \(error)")
        }
    }

    /// The built-in protocol-state store passes the suite the core defines
    /// for a supported backend (C11), called from Rust through the callback
    /// interface, which is the direction the production path uses.
    func testTheBuiltInStateStorePassesConformance() throws {
        let root = FileManager.default.temporaryDirectory
            .appendingPathComponent("link-tests-" + UUID().uuidString, isDirectory: true)
        defer { try? FileManager.default.removeItem(at: root) }
        let storage = try AppContainerProtocolStateStorage(root: root)

        let report = runStorageConformance(storage: storage)

        let data = try XCTUnwrap(report.data(using: .utf8))
        let json = try XCTUnwrap(
            JSONSerialization.jsonObject(with: data) as? [String: Any], "not an object: \(report)")
        let failures = try XCTUnwrap(json["failures"] as? [Any], "no failures key: \(report)")
        let passed = try XCTUnwrap(json["passed"] as? [Any], "no passed key: \(report)")
        XCTAssertTrue(failures.isEmpty, "conformance failures: \(failures)")
        // An empty report would satisfy the line above.
        XCTAssertFalse(passed.isEmpty, "the suite ran no checks: \(report)")
    }

    /// The privacy manifest ships inside the package. A static library cannot
    /// carry one, so it rides the Swift target's resources, and a manifest
    /// that stops listing it still builds.
    func testThePrivacyManifestIsInTheBundle() throws {
        let url = try XCTUnwrap(
            Bundle.module.url(forResource: "PrivacyInfo", withExtension: "xcprivacy"),
            "PrivacyInfo.xcprivacy is not among the package's resources")

        let plist = try PropertyListSerialization.propertyList(
            from: Data(contentsOf: url), format: nil)

        let manifest = try XCTUnwrap(plist as? [String: Any])
        XCTAssertNotNil(manifest["NSPrivacyAccessedAPITypes"])
    }
}
