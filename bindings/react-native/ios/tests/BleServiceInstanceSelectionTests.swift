//
// BleServiceInstanceSelectionTests.swift
//
// Pins which service instance a central binds to when a phone runs several
// SDK apps. Mirrors android's BleServiceInstanceSelectionTest. Keep in sync.
//
// Regression pin: before this rule, iOS read DEVICE_ID and IDENTITY from every
// instance into one per-peripheral slot and cross-checked whichever pair
// landed, so a phone running two SDK apps was refused or announced under the
// wrong app's identity at random. Android bound to the first instance always.
//

import XCTest
@testable import OfflineProtocol

final class BleServiceInstanceSelectionTests: XCTestCase {

    private let ours = Data([0x01, 0x02, 0x03, 0x04, 0x05, 0x06, 0x07, 0x08])
    private let chat = Data([0x11, 0x12, 0x13, 0x14, 0x15, 0x16, 0x17, 0x18])
    private let game = Data([0x21, 0x22, 0x23, 0x24, 0x25, 0x26, 0x27, 0x28])

    private func candidate(_ tag: Data?, canHandshake: Bool = true) -> BleServiceInstanceSelection.Candidate {
        BleServiceInstanceSelection.Candidate(tag: tag, canHandshake: canHandshake)
    }

    private func select(_ candidates: [BleServiceInstanceSelection.Candidate]) -> BleServiceInstanceSelection.Selection {
        BleServiceInstanceSelection.select(candidates, ownTag: ours)
    }

    /// The case this type exists for: our app's instance is not the first.
    func testPicksOurAppsInstanceWhereverItSits() {
        XCTAssertEqual(
            select([candidate(chat), candidate(game), candidate(ours)]),
            .init(index: 2, reason: BleServiceInstanceSelection.Reason.sameApp)
        )
    }

    /// Two builds of one app on one phone: the first wins.
    func testTwoInstancesOfOurAppPickTheFirst() {
        XCTAssertEqual(
            select([candidate(chat), candidate(ours), candidate(ours)]),
            .init(index: 1, reason: BleServiceInstanceSelection.Reason.sameApp)
        )
    }

    /// Our tag beats an untagged instance: the untagged rule is a fallback.
    func testOurTagBeatsAnUntaggedInstance() {
        XCTAssertEqual(
            select([candidate(nil), candidate(ours)]),
            .init(index: 1, reason: BleServiceInstanceSelection.Reason.sameApp)
        )
    }

    /// One build predating the tag among tagged foreign apps: most plausibly
    /// ours, and certainly not one of the apps that named themselves.
    func testSoleUntaggedInstanceAmongForeignAppsIsChosen() {
        XCTAssertEqual(
            select([candidate(chat), candidate(nil), candidate(game)]),
            .init(index: 1, reason: BleServiceInstanceSelection.Reason.soleUntagged)
        )
    }

    /// Only foreign apps: the phone is still a relay neighbour, bound as
    /// before the tag existed.
    func testAllForeignFallsBackToTheFirstInstance() {
        XCTAssertEqual(
            select([candidate(chat), candidate(game)]),
            .init(index: 0, reason: BleServiceInstanceSelection.Reason.firstInstance)
        )
    }

    /// Several builds predating the tag: nothing to go on, so the first.
    func testSeveralUntaggedFallBackToTheFirstInstance() {
        XCTAssertEqual(
            select([candidate(nil), candidate(nil)]),
            .init(index: 0, reason: BleServiceInstanceSelection.Reason.firstInstance)
        )
    }

    /// An instance that cannot finish the handshake is never chosen over one
    /// that can, even when it serves our tag.
    func testInstanceThatCannotHandshakeIsSkipped() {
        XCTAssertEqual(
            select([candidate(ours, canHandshake: false), candidate(chat)]),
            .init(index: 1, reason: BleServiceInstanceSelection.Reason.firstInstance)
        )
        XCTAssertEqual(
            select([candidate(nil, canHandshake: false), candidate(nil), candidate(chat)]),
            .init(index: 1, reason: BleServiceInstanceSelection.Reason.soleUntagged)
        )
    }

    /// Nothing usable: index 0, so the handshake refuses it for the
    /// characteristic it lacks, exactly as before the tag.
    func testNoInstanceCanHandshakeReturnsTheFirst() {
        XCTAssertEqual(
            select([candidate(ours, canHandshake: false), candidate(chat, canHandshake: false)]),
            .init(index: 0, reason: BleServiceInstanceSelection.Reason.noneCanHandshake)
        )
        XCTAssertEqual(
            select([]),
            .init(index: 0, reason: BleServiceInstanceSelection.Reason.noneCanHandshake)
        )
    }

    /// The reason strings are shared with Kotlin; a rename on one side only
    /// would split the diagnostics.
    func testReasonStringsAreStable() {
        XCTAssertEqual(BleServiceInstanceSelection.Reason.sameApp, "same_app")
        XCTAssertEqual(BleServiceInstanceSelection.Reason.soleUntagged, "sole_untagged")
        XCTAssertEqual(BleServiceInstanceSelection.Reason.firstInstance, "first_instance")
        XCTAssertEqual(BleServiceInstanceSelection.Reason.noneCanHandshake, "none_can_handshake")
    }
}
