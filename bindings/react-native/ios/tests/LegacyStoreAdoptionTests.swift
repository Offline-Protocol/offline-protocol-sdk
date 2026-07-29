import XCTest
@testable import OfflineProtocol

final class LegacyStoreAdoptionTests: XCTestCase {
    private let namespace = "account-" + String(repeating: "a", count: 64)
    private let other = "account-" + String(repeating: "b", count: 64)

    /// The upgrade case: an install that predates namespacing has an unclaimed
    /// legacy store, and the account that launches first inherits it. Without
    /// this the account would look brand new and mint a fresh MLS identity.
    func testUnclaimedLegacyStoreIsAdopted() {
        XCTAssertEqual(
            LegacyStoreAdoption.decide(existingClaim: nil, namespace: namespace),
            .adopt
        )
        XCTAssertEqual(
            LegacyStoreAdoption.decide(existingClaim: "", namespace: namespace),
            .adopt
        )
    }

    /// Adoption must be resumable: a launch after the claim was written but
    /// before every entry was promoted still reads through.
    func testOurOwnClaimResumesReadThrough() {
        XCTAssertEqual(
            LegacyStoreAdoption.decide(existingClaim: namespace, namespace: namespace),
            .resume
        )
    }

    /// The legacy store was shared by every account on the install, so only one
    /// can inherit it. A second account is genuinely new — but that must be
    /// reported, not silently rotated into.
    func testForeignClaimBlocksReadThrough() {
        let decision = LegacyStoreAdoption.decide(
            existingClaim: other,
            namespace: namespace
        )
        XCTAssertEqual(decision, .conflict(claimedBy: other))
        XCTAssertFalse(LegacyStoreAdoption.allowsReadThrough(decision))
    }

    /// A claim we can read back as our own is what actually makes inheritance
    /// exclusive; the pre-write probe only says the store *looked* unclaimed.
    func testClaimReadBackAsOursCompletesTheAdoption() {
        XCTAssertEqual(
            LegacyStoreAdoption.confirmClaim(readBack: namespace, namespace: namespace),
            .adopt
        )
    }

    /// The write reported success (or threw, which the caller spells as nil)
    /// but the store does not hold our claim. Adopting anyway is what lets a
    /// second account find the store still unclaimed, adopt it too, and end up
    /// sharing this account's MLS identity — so an unproven claim fails closed.
    func testUnrecordedClaimDoesNotAdopt() {
        let decision = LegacyStoreAdoption.confirmClaim(
            readBack: nil,
            namespace: namespace
        )
        XCTAssertEqual(decision, .claimUnverified)
        XCTAssertFalse(LegacyStoreAdoption.allowsReadThrough(decision))
        XCTAssertEqual(
            LegacyStoreAdoption.confirmClaim(readBack: "", namespace: namespace),
            .claimUnverified
        )
    }

    /// The read back also catches a racing claim, which the pre-write probe
    /// cannot see.
    func testClaimReadBackAsSomeoneElsesIsAConflict() {
        XCTAssertEqual(
            LegacyStoreAdoption.confirmClaim(readBack: other, namespace: namespace),
            .conflict(claimedBy: other)
        )
    }

    func testAdoptAndResumeAllowReadThrough() {
        XCTAssertTrue(LegacyStoreAdoption.allowsReadThrough(.adopt))
        XCTAssertTrue(LegacyStoreAdoption.allowsReadThrough(.resume))
        XCTAssertFalse(LegacyStoreAdoption.allowsReadThrough(.claimUnverified))
    }

    /// The claim entry is bookkeeping, not key material: promoting it into the
    /// new store would make a later account read its own namespace back as an
    /// inherited value.
    func testClaimEntryIsNeverReadThrough() {
        XCTAssertTrue(
            LegacyStoreAdoption.isClaimEntry(keyType: LegacyStoreAdoption.claimKeyType)
        )
        XCTAssertFalse(LegacyStoreAdoption.isClaimEntry(keyType: "identity"))
    }
}
