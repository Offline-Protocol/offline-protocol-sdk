//
// PeerIdentityBindingTests.swift
//
// Pins the BLE discovery gate: a peer is announced only under an address it
// proved. Mirrors android's PeerIdentityBindingTest — keep in sync.
//
// Regression pin: before this rule, `DEVICE_ID` was announced as-is. It
// carried the app-chosen `profile`, which is not the peer's identity and is
// commonly a shared constant like "default" — so peers collided on one id,
// and every control frame they sent was dropped by the core's
// `validate_transport_sender` because the `Message.sender` they stamp is
// their derived address, not their profile.
//

import XCTest
@testable import OfflineProtocol

final class PeerIdentityBindingTests: XCTestCase {

    /// A real derivation, from `crates/offline-protocol-core/src/address.rs`
    /// golden vectors — 44 characters, canonical bech32m.
    private let addressA = "off1qysluvwl5922yctzd0u9gpr06gn3k7ldfvgtwgvn"
    private let addressB = "off1qyqqqqqqqqqqqqqqqqqqqqqqqqqqqqqqqqn8antf"

    /// The happy path: what the peer advertises is what its key derives to.
    func testMatchingAdvertisementIsVerified() {
        XCTAssertEqual(
            PeerIdentityBinding.resolve(advertisedDeviceId: addressA, derivedAddress: addressA),
            .verified(peerId: addressA)
        )
    }

    /// The announced id comes from the proof, not the claim. They are equal
    /// here by construction, so this pins *which* value is returned — the one
    /// a future relaxation of the comparison must not be able to bypass.
    func testVerifiedPeerIdIsTheDerivedAddress() {
        guard case let .verified(peerId) = PeerIdentityBinding.resolve(
            advertisedDeviceId: addressA,
            derivedAddress: addressA
        ) else {
            return XCTFail("expected a verified outcome")
        }
        XCTAssertEqual(peerId, addressA)
    }

    /// Two different addresses: the peer is claiming an id its key does not
    /// derive to. This is the impersonation attempt the gate exists for.
    func testMismatchedAddressIsRejected() {
        XCTAssertEqual(
            PeerIdentityBinding.resolve(advertisedDeviceId: addressA, derivedAddress: addressB),
            .rejected(reason: PeerIdentityBinding.Reason.addressMismatch)
        )
    }

    /// The cutover case: a build that still advertises its profile. It must
    /// stay invisible rather than being surfaced under an unproven name.
    func testProfileShapedAdvertisementIsRejected() {
        XCTAssertEqual(
            PeerIdentityBinding.resolve(advertisedDeviceId: "default", derivedAddress: addressA),
            .rejected(reason: PeerIdentityBinding.Reason.addressMismatch)
        )
    }

    /// No identity read, or one that failed to verify. The caller passes nil
    /// for anything short of a decoded blob with a good signature, so this one
    /// case covers absent, undecodable, and forged alike.
    func testUnverifiedIdentityIsRejected() {
        XCTAssertEqual(
            PeerIdentityBinding.resolve(advertisedDeviceId: addressA, derivedAddress: nil),
            .rejected(reason: PeerIdentityBinding.Reason.unverifiedIdentity)
        )
    }

    /// An identity alone is not enough. The peer must also advertise the
    /// address, because `DEVICE_ID` is what the MTU map and the connection
    /// registry key on — accepting identity-only would leave those unkeyed.
    func testMissingDeviceIdIsRejected() {
        XCTAssertEqual(
            PeerIdentityBinding.resolve(advertisedDeviceId: nil, derivedAddress: addressA),
            .rejected(reason: PeerIdentityBinding.Reason.missingDeviceId)
        )
    }

    /// An empty characteristic value is the same as an absent one. Android's
    /// central already closes the link on this (`empty_device_id`); the rule
    /// must not disagree.
    func testEmptyDeviceIdIsRejected() {
        XCTAssertEqual(
            PeerIdentityBinding.resolve(advertisedDeviceId: "", derivedAddress: addressA),
            .rejected(reason: PeerIdentityBinding.Reason.missingDeviceId)
        )
    }

    /// Neither side available — the missing device id is reported, so the
    /// diagnostic names the first thing the handshake failed to obtain.
    func testBothAbsentReportsTheDeviceId() {
        XCTAssertEqual(
            PeerIdentityBinding.resolve(advertisedDeviceId: nil, derivedAddress: nil),
            .rejected(reason: PeerIdentityBinding.Reason.missingDeviceId)
        )
    }

    /// Bech32m permits an uppercase encoding, but the core emits canonical
    /// lowercase from one shared `derive_address`. A peer advertising the
    /// other casing did not derive its id the way we do, so it is refused
    /// rather than normalised into agreement.
    func testCaseDifferingAdvertisementIsRejected() {
        XCTAssertEqual(
            PeerIdentityBinding.resolve(
                advertisedDeviceId: addressA.uppercased(),
                derivedAddress: addressA
            ),
            .rejected(reason: PeerIdentityBinding.Reason.addressMismatch)
        )
    }
}
