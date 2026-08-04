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

    // MARK: - Wipe policy

    /// The leftover a logout is asked to erase. Every account that has completed
    /// a post-split launch records a claim, so an unclaimed store is what the
    /// *previous* install left behind — and on a platform whose credential store
    /// outlives the app container, what a reinstall would otherwise re-adopt.
    func testUnclaimedLegacyStoreMayBeWiped() {
        XCTAssertTrue(LegacyStoreAdoption.shouldWipeLegacy(.absent, namespace: namespace))
        XCTAssertTrue(
            LegacyStoreAdoption.shouldWipeLegacy(.of(nil), namespace: namespace)
        )
        // Empty reads as absent here exactly as it does in `decide`.
        XCTAssertTrue(
            LegacyStoreAdoption.shouldWipeLegacy(.of(""), namespace: namespace)
        )
    }

    /// The ordinary logout: this account inherited the legacy store, so erasing
    /// it is erasing its own material.
    func testOurOwnClaimMayBeWiped() {
        XCTAssertTrue(
            LegacyStoreAdoption.shouldWipeLegacy(.of(namespace), namespace: namespace)
        )
        XCTAssertTrue(
            LegacyStoreAdoption.shouldWipeLegacy(
                .owned(by: namespace),
                namespace: namespace
            )
        )
    }

    /// The legacy store was shared by every account on a pre-split install, so
    /// another account's claim makes it theirs. Wiping it would destroy an MLS
    /// identity, sessions, and a block list that have nothing to do with this
    /// logout.
    func testForeignClaimIsNotWiped() {
        XCTAssertFalse(
            LegacyStoreAdoption.shouldWipeLegacy(.of(other), namespace: namespace)
        )
    }

    /// Unreadable is not "unclaimed". The two are indistinguishable at the
    /// store, and the mistakes are not symmetric: refusing costs a leftover the
    /// next wipe removes, while proceeding can destroy another account. So it
    /// fails closed — which is the whole reason `LegacyClaim` keeps a third case
    /// that `decide` does not.
    func testUnreadableClaimIsNotWiped() {
        XCTAssertFalse(
            LegacyStoreAdoption.shouldWipeLegacy(.unreadable, namespace: namespace)
        )
    }

    /// Bytes that are present but not UTF-8 are still evidence that something
    /// claimed the store. Reading them as *absent* would authorise a wipe of
    /// the shared pre-namespace store — another account's MLS identity,
    /// sessions, and block list — on the strength of a claim this reader merely
    /// failed to interpret. So the lossy decode keeps it `owned`, the mismatch
    /// refuses the wipe, and adoption conflicts rather than inheriting an
    /// identity whose owner it could not establish.
    func testNonUtf8ClaimIsOwnedNotAbsent() {
        let garbage: [UInt8] = [0xFF, 0xFE, 0xFD]
        let claim = LegacyStoreAdoption.LegacyClaim.of(bytes: garbage)

        XCTAssertNotEqual(claim, .absent)
        XCTAssertFalse(LegacyStoreAdoption.shouldWipeLegacy(claim, namespace: namespace))

        guard case .owned(let owner) = claim else {
            return XCTFail("expected a garbled claim to read as owned, got \(claim)")
        }
        XCTAssertNotEqual(owner, namespace)
        XCTAssertEqual(
            LegacyStoreAdoption.decide(existingClaim: owner, namespace: namespace),
            .conflict(claimedBy: owner)
        )
    }

    /// The byte and string classifications must agree, so routing a read
    /// through either one cannot change who may destroy the store.
    func testByteAndStringClaimsAgree() {
        XCTAssertEqual(
            LegacyStoreAdoption.LegacyClaim.of(bytes: Array(namespace.utf8)),
            .owned(by: namespace)
        )
        XCTAssertEqual(LegacyStoreAdoption.LegacyClaim.of(bytes: []), .absent)
        XCTAssertTrue(
            LegacyStoreAdoption.shouldWipeLegacy(
                .of(bytes: Array(namespace.utf8)),
                namespace: namespace
            )
        )
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

    // MARK: - Tombstones

    /// Both reserved entries are the provider's own bookkeeping. Neither may be
    /// promoted, listed, or handed back as key material — a tombstone escaping
    /// as a loadable key would be the provider advertising a corpse.
    func testReservedEntriesCoverTheClaimAndTombstones() {
        XCTAssertTrue(
            LegacyStoreAdoption.isReservedEntry(keyType: LegacyStoreAdoption.claimKeyType)
        )
        XCTAssertTrue(
            LegacyStoreAdoption.isReservedEntry(
                keyType: LegacyStoreAdoption.tombstoneKeyType
            )
        )
        XCTAssertFalse(LegacyStoreAdoption.isReservedEntry(keyType: "identity"))
        XCTAssertFalse(LegacyStoreAdoption.isReservedEntry(keyType: "key_package"))
    }

    /// The two reserved key types must stay distinct: collapsing them would let
    /// a tombstone answer a claim read, and the claim decides which account
    /// inherits the legacy identity.
    func testReservedKeyTypesAreDistinct() {
        XCTAssertNotEqual(
            LegacyStoreAdoption.claimKeyType,
            LegacyStoreAdoption.tombstoneKeyType
        )
    }

    /// The asymmetry the three-way exists for. Both non-`absent` answers stop
    /// read-through, because a read that failed cannot prove read-through is
    /// safe. Only a confirmed tombstone also authorises deleting the legacy
    /// copy: acting on a failed read would destroy the last copy of a key that
    /// was legitimately inheritable — on a first post-upgrade launch, possibly
    /// the signing identity — and unlike suppression that cannot be walked
    /// back.
    func testAnUnreadableTombstoneSuppressesWithoutAuthorisingDeletion() {
        let unreadable = LegacyStoreAdoption.TombstoneState.unreadable

        XCTAssertTrue(unreadable.suppressesReadThrough)
        XCTAssertFalse(unreadable.allowsRemovalRetry)
    }

    func testARecordedTombstoneSuppressesAndAuthorisesDeletion() {
        let recorded = LegacyStoreAdoption.TombstoneState.recorded

        XCTAssertTrue(recorded.suppressesReadThrough)
        XCTAssertTrue(recorded.allowsRemovalRetry)
    }

    /// The ordinary path: nothing was ever tombstoned, so an inherited entry is
    /// still promotable.
    func testAnAbsentTombstonePermitsReadThrough() {
        let absent = LegacyStoreAdoption.TombstoneState.absent

        XCTAssertFalse(absent.suppressesReadThrough)
        XCTAssertFalse(absent.allowsRemovalRetry)
    }

    /// One tombstone names exactly one legacy key. Keyed like the stores' own
    /// account keys, so it inherits their existing ambiguity rather than adding
    /// a new one.
    func testTombstoneIdNamesOneKey() {
        XCTAssertEqual(
            LegacyStoreAdoption.tombstoneKeyId(keyType: "key_package", keyId: "peer-1"),
            "key_package:peer-1"
        )
        XCTAssertNotEqual(
            LegacyStoreAdoption.tombstoneKeyId(keyType: "key_package", keyId: "peer-1"),
            LegacyStoreAdoption.tombstoneKeyId(keyType: "key_package", keyId: "peer-2")
        )
    }
}
