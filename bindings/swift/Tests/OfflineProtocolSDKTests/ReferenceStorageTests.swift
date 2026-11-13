//
// ReferenceStorageTests.swift
//
// The protocol-state half of `OfflineProtocolReferenceStorage`, linked.
//
// The Keychain half cannot run here: a package test has no host application,
// so it has no Keychain entitlement and every Keychain call fails with
// errSecMissingEntitlement. What runs is the half a package test can reach,
// at the path an application gets rather than a temporary root.
//

import XCTest
@testable import OfflineProtocolSDK

final class ReferenceStorageTests: XCTestCase {

    private let appId = "reference-storage-tests"
    private var profile = ""

    override func setUpWithError() throws {
        profile = UUID().uuidString
    }

    override func tearDownWithError() throws {
        try AppContainerProtocolStateStorage.wipeAccount(
            accountNamespace: StorageNamespace.account(appId: appId, profile: profile))
    }

    /// The store an application opens passes the suite the core defines for a
    /// supported backend (C11), at the account's own root.
    func testTheReferenceStateStorePassesConformance() throws {
        let storage = try OfflineProtocolReferenceStorage.makeProtocolStateStorage(
            appId: appId, profile: profile)

        let report = runStorageConformance(storage: storage)

        let data = try XCTUnwrap(report.data(using: .utf8))
        let json = try XCTUnwrap(
            JSONSerialization.jsonObject(with: data) as? [String: Any], "not an object: \(report)")
        let failures = try XCTUnwrap(json["failures"] as? [Any], "no failures key: \(report)")
        let passed = try XCTUnwrap(json["passed"] as? [Any], "no passed key: \(report)")
        XCTAssertTrue(failures.isEmpty, "conformance failures: \(failures)")
        XCTAssertFalse(passed.isEmpty, "the suite ran no checks: \(report)")
    }

    /// `wipe` erases the store `makeProtocolStateStorage` opened. A wipe that
    /// derived the account's path on its own would return and erase nothing.
    /// It reaches the Keychain too, which throws here; the state store must
    /// still be wiped, because the wipe attempts both halves.
    func testWipeErasesTheStoreTheFactoryOpened() throws {
        let written = try OfflineProtocolReferenceStorage.makeProtocolStateStorage(
            appId: appId, profile: profile)
        try written.store(keyType: "pending/messages", keyId: "peer", data: Data([1, 2, 3]))
        XCTAssertNotNil(try written.load(keyType: "pending/messages", keyId: "peer"))

        _ = try? OfflineProtocolReferenceStorage.wipe(appId: appId, profile: profile)

        let reopened = try OfflineProtocolReferenceStorage.makeProtocolStateStorage(
            appId: appId, profile: profile)
        XCTAssertNil(try reopened.load(keyType: "pending/messages", keyId: "peer"))
    }

    /// Only the two outcomes that start a fresh identity carry a warning, and
    /// every decision the secure store can reach maps to one outcome.
    func testOnlyAFreshIdentityCarriesAWarning() {
        let outcomes: [(LegacyStoreAdoption.Decision?, OfflineProtocolReferenceStorage.LegacyAdoption)] = [
            (nil, .none),
            (.adopt, .adopted),
            (.resume, .resumed),
            (.conflict(claimedBy: "account-other"), .conflict),
            (.claimUnverified, .claimUnverified),
        ]
        for (decision, expected) in outcomes {
            let adoption = OfflineProtocolReferenceStorage.LegacyAdoption(decision)
            XCTAssertEqual(adoption, expected)
            XCTAssertEqual(adoption.warning != nil, adoption.startsFreshIdentity, "\(adoption)")
        }
        XCTAssertTrue(OfflineProtocolReferenceStorage.LegacyAdoption.conflict.startsFreshIdentity)
        XCTAssertTrue(OfflineProtocolReferenceStorage.LegacyAdoption.claimUnverified.startsFreshIdentity)
    }
}
