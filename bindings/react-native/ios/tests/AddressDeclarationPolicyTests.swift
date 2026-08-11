//
// AddressDeclarationPolicyTests.swift
//
// Pins the relay address declaration: the exact bytes signed, and the four
// conditions under which a connection stays in account-name space.
// Mirrors android's AddressDeclarationPolicyTest — keep in sync.
//
// The payload test is a cross-repo pin. Its expected value is copied from the
// relay's own `address_proof_payload_matches_the_pinned_vector`
// (relay-server src/address_binding.rs), so the two implementations cannot
// drift apart silently — which matters more than usual here, because a wrong
// payload is not a compile error or a parse error on either side. It is a
// signature that simply does not verify, reported by the relay as
// `AddressError`, and indistinguishable in its logs from an attack.
//

import XCTest
@testable import OfflineProtocol

final class AddressDeclarationPolicyTests: XCTestCase {

    /// A real derivation from the core's address golden vectors.
    private let address = "off1qysluvwl5922yctzd0u9gpr06gn3k7ldfvgtwgvn"

    /// The challenge from the relay's pinned vector: bytes 0x00…0x1f.
    private let vectorChallenge = Data((0..<32).map { UInt8($0) })

    private func hex(_ data: Data) -> String {
        data.map { String(format: "%02x", $0) }.joined()
    }

    // MARK: - The signed bytes

    /// Byte-for-byte against the relay's pinned vector. Any drift in the
    /// domain string, the length prefix's width or endianness, the UTF-8
    /// encoding, or the concatenation order fails here rather than in the
    /// field.
    func testProofPayloadMatchesThePinnedRelayVector() {
        let payload = AddressDeclarationPolicy.proofPayload(
            account: "alice",
            challenge: vectorChallenge
        )
        XCTAssertEqual(
            hex(payload),
            "6f66666c696e652d72656c61792d616464722d7631"
                + "00000005"
                + "616c696365"
                + "000102030405060708090a0b0c0d0e0f101112131415161718191a1b1c1d1e1f"
        )
    }

    /// The length prefix is big-endian. A little-endian bridge writes
    /// `05000000` here and produces a signature the relay refuses — the one
    /// error the pinned vector exists to catch, isolated so a failure names it.
    func testAccountLengthPrefixIsBigEndian() {
        let payload = AddressDeclarationPolicy.proofPayload(
            account: "alice",
            challenge: vectorChallenge
        )
        let domainLength = AddressDeclarationPolicy.PROOF_DOMAIN.utf8.count
        XCTAssertEqual(
            hex(payload.subdata(in: domainLength..<(domainLength + 4))),
            "00000005"
        )
    }

    /// The prefix counts UTF-8 bytes, not characters. The relay reads the same
    /// bytes back out, so a character count silently mis-frames every account
    /// name outside ASCII.
    func testAccountLengthCountsUtf8BytesNotCharacters() {
        let account = "zoë"  // 3 characters, 4 UTF-8 bytes
        let payload = AddressDeclarationPolicy.proofPayload(
            account: account,
            challenge: vectorChallenge
        )
        let domainLength = AddressDeclarationPolicy.PROOF_DOMAIN.utf8.count
        XCTAssertEqual(
            hex(payload.subdata(in: domainLength..<(domainLength + 4))),
            "00000004"
        )
        XCTAssertEqual(payload.count, domainLength + 4 + 4 + 32)
    }

    /// The domain must not prefix, nor be prefixed by, the core's
    /// control-frame domain. If either did, a relay-chosen challenge could
    /// steer this signature into the control-message domain and replay as a
    /// frame from this peer. The relay pins the same relation from its side.
    func testProofDomainCannotCollideWithControlFrameSigning() {
        let controlDomain = "offline-ctrl-v1"
        let proofDomain = AddressDeclarationPolicy.PROOF_DOMAIN
        XCTAssertFalse(proofDomain.hasPrefix(controlDomain))
        XCTAssertFalse(controlDomain.hasPrefix(proofDomain))
    }

    /// The payload starts with the domain, so a signature over the bare
    /// challenge — the naive implementation — can never equal it. The relay
    /// refuses that shape explicitly (`a_bare_challenge_signature_is_refused`).
    func testPayloadIsNeverTheBareChallenge() {
        let payload = AddressDeclarationPolicy.proofPayload(
            account: "alice",
            challenge: vectorChallenge
        )
        XCTAssertNotEqual(payload, vectorChallenge)
        XCTAssertTrue(hex(payload).hasPrefix(hex(Data(AddressDeclarationPolicy.PROOF_DOMAIN.utf8))))
    }

    // MARK: - The decision

    private func capabilities(_ extra: String...) -> [String] {
        ["group_delivery_v2"] + extra
    }

    /// The happy path: capability, a well-formed challenge, and an account
    /// name the relay itself supplied.
    func testDeclaresWhenCapabilityChallengeAndAccountArePresent() {
        let outcome = AddressDeclarationPolicy.decide(
            capabilities: capabilities(AddressDeclarationPolicy.CAPABILITY),
            addressChallenge: vectorChallenge.base64EncodedString(),
            username: "alice"
        )
        XCTAssertEqual(outcome, .declare(account: "alice", challenge: vectorChallenge))
    }

    /// An older relay. It omits the capability, would parse a `DeclareAddress`
    /// into nothing and answer nothing at all — so the token is what gates the
    /// send, and its absence is expected rather than exceptional.
    func testSkipsWhenRelayLacksTheCapability() {
        XCTAssertEqual(
            AddressDeclarationPolicy.decide(
                capabilities: capabilities(),
                addressChallenge: vectorChallenge.base64EncodedString(),
                username: "alice"
            ),
            .skip(reason: AddressDeclarationPolicy.Reason.capabilityAbsent)
        )
    }

    /// Capability without a challenge: nothing to sign.
    func testSkipsWhenChallengeIsAbsent() {
        XCTAssertEqual(
            AddressDeclarationPolicy.decide(
                capabilities: capabilities(AddressDeclarationPolicy.CAPABILITY),
                addressChallenge: nil,
                username: "alice"
            ),
            .skip(reason: AddressDeclarationPolicy.Reason.challengeAbsent)
        )
    }

    /// Not base64 at all.
    func testSkipsWhenChallengeIsNotBase64() {
        XCTAssertEqual(
            AddressDeclarationPolicy.decide(
                capabilities: capabilities(AddressDeclarationPolicy.CAPABILITY),
                addressChallenge: "not base64!!",
                username: "alice"
            ),
            .skip(reason: AddressDeclarationPolicy.Reason.challengeMalformed)
        )
    }

    /// Decodes, but not to 32 bytes. Signing it would produce a proof that
    /// cannot verify, so it is refused before the FFI is touched.
    func testSkipsWhenChallengeIsWrongLength() {
        XCTAssertEqual(
            AddressDeclarationPolicy.decide(
                capabilities: capabilities(AddressDeclarationPolicy.CAPABILITY),
                addressChallenge: Data(repeating: 7, count: 16).base64EncodedString(),
                username: "alice"
            ),
            .skip(reason: AddressDeclarationPolicy.Reason.challengeMalformed)
        )
    }

    /// The relay decodes with a strict standard-alphabet engine, so a
    /// base64url spelling of the same 32 bytes is not interchangeable.
    func testSkipsWhenChallengeIsBase64Url() {
        // A 32-byte value whose standard encoding contains both '+' and '/'.
        var raw = Data(repeating: 0, count: 32)
        raw[0] = 0xFB
        raw[1] = 0xF0
        raw[2] = 0x00
        let standard = raw.base64EncodedString()
        let urlSafe = standard.replacingOccurrences(of: "+", with: "-")
            .replacingOccurrences(of: "/", with: "_")
            .replacingOccurrences(of: "=", with: "")
        XCTAssertNotEqual(standard, urlSafe, "the fixture must actually differ between alphabets")
        XCTAssertEqual(
            AddressDeclarationPolicy.decide(
                capabilities: capabilities(AddressDeclarationPolicy.CAPABILITY),
                addressChallenge: urlSafe,
                username: "alice"
            ),
            .skip(reason: AddressDeclarationPolicy.Reason.challengeMalformed)
        )
    }

    /// No account name on the frame. The proof binds the name the relay
    /// resolved, so there is nothing local that could stand in: signing a
    /// substitute (the profile, a device id) yields a signature that cannot
    /// verify and reads as an attack in the relay's logs.
    func testSkipsWhenAccountIsAbsent() {
        XCTAssertEqual(
            AddressDeclarationPolicy.decide(
                capabilities: capabilities(AddressDeclarationPolicy.CAPABILITY),
                addressChallenge: vectorChallenge.base64EncodedString(),
                username: nil
            ),
            .skip(reason: AddressDeclarationPolicy.Reason.accountAbsent)
        )
    }

    func testSkipsWhenAccountIsEmpty() {
        XCTAssertEqual(
            AddressDeclarationPolicy.decide(
                capabilities: capabilities(AddressDeclarationPolicy.CAPABILITY),
                addressChallenge: vectorChallenge.base64EncodedString(),
                username: ""
            ),
            .skip(reason: AddressDeclarationPolicy.Reason.accountAbsent)
        )
    }

    // MARK: - The frame

    /// Field names and the frame tag are the relay's `ClientMessage` variant;
    /// all three values are base64 standard *with* padding, which is what the
    /// relay's decoder requires.
    func testDeclarationFrameShape() throws {
        let publicKey = Data(repeating: 0xAB, count: 32)
        let signature = Data(repeating: 0xCD, count: 64)
        let json = try XCTUnwrap(
            AddressDeclarationPolicy.declarationJson(
                address: address,
                publicKey: publicKey,
                signature: signature
            )
        )
        let parsed = try XCTUnwrap(
            try JSONSerialization.jsonObject(with: XCTUnwrap(json.data(using: .utf8)))
                as? [String: Any]
        )
        XCTAssertEqual(parsed["type"] as? String, "DeclareAddress")
        XCTAssertEqual(parsed["address"] as? String, address)
        XCTAssertEqual(parsed["public_key"] as? String, publicKey.base64EncodedString())
        XCTAssertEqual(parsed["signature"] as? String, signature.base64EncodedString())
        XCTAssertEqual(parsed.count, 4)
    }

    /// Length and padding of the encoded material, as the relay expects to
    /// find it: 32 bytes → 44 characters, 64 bytes → 88.
    func testEncodedMaterialIsPaddedStandardBase64() throws {
        let json = try XCTUnwrap(
            AddressDeclarationPolicy.declarationJson(
                address: address,
                publicKey: Data(repeating: 0xAB, count: 32),
                signature: Data(repeating: 0xCD, count: 64)
            )
        )
        let parsed = try XCTUnwrap(
            try JSONSerialization.jsonObject(with: XCTUnwrap(json.data(using: .utf8)))
                as? [String: Any]
        )
        let publicKey = try XCTUnwrap(parsed["public_key"] as? String)
        let signature = try XCTUnwrap(parsed["signature"] as? String)
        XCTAssertEqual(publicKey.count, 44)
        XCTAssertTrue(publicKey.hasSuffix("="))
        XCTAssertEqual(signature.count, 88)
        XCTAssertTrue(signature.hasSuffix("="))
        XCTAssertFalse(publicKey.contains("-") || publicKey.contains("_"))
        XCTAssertFalse(signature.contains("-") || signature.contains("_"))
    }
}
