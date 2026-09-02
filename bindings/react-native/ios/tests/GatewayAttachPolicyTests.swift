import XCTest

@testable import OfflineProtocol

/// The gateway contract's decisions and frame shapes.
///
/// What is *not* here is the signed proof: those bytes are built and pinned in
/// the core, by conformance vectors, so this side has no copy of the layout to
/// drift from. What is here is everything the manager decides on its own.
final class GatewayAttachPolicyTests: XCTestCase {

    // MARK: - Challenge

    func testAWellFormedChallengeIsAccepted() {
        let challenge = Data((0..<32).map { UInt8($0) })
        let outcome = GatewayAttachPolicy.decodeChallenge([
            "challenge": challenge.base64EncodedString()
        ])
        XCTAssertEqual(outcome, .declare(challenge: challenge))
    }

    func testAMissingChallengeIsSkippedNotGuessed() {
        XCTAssertEqual(
            GatewayAttachPolicy.decodeChallenge([:]),
            .skip(reason: GatewayAttachPolicy.SkipReason.challengeAbsent))
        XCTAssertEqual(
            GatewayAttachPolicy.decodeChallenge(["challenge": ""]),
            .skip(reason: GatewayAttachPolicy.SkipReason.challengeAbsent))
    }

    func testANonBase64ChallengeIsSkipped() {
        XCTAssertEqual(
            GatewayAttachPolicy.decodeChallenge(["challenge": "not base64!!"]),
            .skip(reason: GatewayAttachPolicy.SkipReason.challengeMalformed))
    }

    /// A short challenge is refused here as well as in the core.
    ///
    /// The replay bound is the challenge, so a gateway that mints eight bytes
    /// has weakened the thing the handshake exists for. Refusing it on this
    /// side means the diagnostic says "the gateway is not speaking the
    /// contract" rather than surfacing as a thrown error from the signer.
    func testAWrongSizedChallengeIsRefused() {
        for size in [0, 8, 31, 33, 64] {
            let challenge = Data(repeating: 7, count: size)
            let outcome = GatewayAttachPolicy.decodeChallenge([
                "challenge": challenge.base64EncodedString()
            ])
            if size == 0 {
                // An empty string reads as absent, which is the same refusal
                // by a different name.
                XCTAssertEqual(
                    outcome, .skip(reason: GatewayAttachPolicy.SkipReason.challengeAbsent))
            } else {
                XCTAssertEqual(
                    outcome, .skip(reason: GatewayAttachPolicy.SkipReason.challengeWrongSize),
                    "a \(size)-byte challenge must not be signed")
            }
        }
    }

    // MARK: - Binding

    func testTheEchoedAddressMustBeOurs() {
        XCTAssertEqual(
            GatewayAttachPolicy.bindingOutcome(declared: "off1alice", local: "off1alice"),
            .bound)
        XCTAssertEqual(
            GatewayAttachPolicy.bindingOutcome(declared: "off1bob", local: "off1alice"),
            .mismatch)
    }

    /// With no local address there is nothing this device declared, so an echo
    /// is not evidence of anything and must not count as bound.
    func testAnEchoWithNoLocalAddressIsNotABinding() {
        XCTAssertEqual(
            GatewayAttachPolicy.bindingOutcome(declared: "off1alice", local: nil),
            .unknownLocal)
        XCTAssertEqual(
            GatewayAttachPolicy.bindingOutcome(declared: "off1alice", local: ""),
            .unknownLocal)
    }

    // MARK: - Capabilities

    func testCapabilitiesAreBounded() {
        let tokens = (0..<200).map { "cap_\($0)" }
        let kept = GatewayAttachPolicy.capabilityTokens(from: ["tokens": tokens])
        XCTAssertEqual(kept.count, GatewayAttachPolicy.MAX_CAPABILITY_TOKENS)
    }

    /// Oversized tokens are dropped before the count is applied, so padding
    /// cannot push out the tokens that matter.
    func testPaddingCannotEvictRealCapabilityTokens() {
        var tokens = [String](repeating: String(repeating: "x", count: 4096), count: 64)
        tokens.append("gateway_v1")
        let kept = GatewayAttachPolicy.capabilityTokens(from: ["tokens": tokens])
        XCTAssertEqual(kept, ["gateway_v1"])
    }

    func testCapabilitiesIgnoreNonStringsAndEmpties() {
        let kept = GatewayAttachPolicy.capabilityTokens(from: [
            "tokens": ["gateway_v1", 7, "", "backbone_reticulum_v1"] as [Any]
        ])
        XCTAssertEqual(kept, ["gateway_v1", "backbone_reticulum_v1"])
    }

    func testAbsentCapabilitiesAreEmptyNotAnError() {
        XCTAssertEqual(GatewayAttachPolicy.capabilityTokens(from: [:]), [])
        XCTAssertEqual(GatewayAttachPolicy.capabilityTokens(from: ["tokens": "nope"]), [])
    }

    // MARK: - Message ids

    /// A UUID is what the core hands us, and it has to survive the gateway's
    /// own rule unchanged: an id it would refuse comes back under a name
    /// nothing is waiting on.
    func testAUuidPassesTheGatewaysOwnRule() {
        let uuid = UUID().uuidString
        XCTAssertEqual(GatewayAttachPolicy.sanitizeMessageId(uuid), uuid)
    }

    func testAnIdTheGatewayWouldRefuseIsRejectedHere() {
        XCTAssertNil(GatewayAttachPolicy.sanitizeMessageId(nil))
        XCTAssertNil(GatewayAttachPolicy.sanitizeMessageId(""))
        XCTAssertNil(GatewayAttachPolicy.sanitizeMessageId("has space"))
        XCTAssertNil(GatewayAttachPolicy.sanitizeMessageId("has/slash"))
        XCTAssertNil(GatewayAttachPolicy.sanitizeMessageId(String(repeating: "a", count: 65)))
        XCTAssertEqual(
            GatewayAttachPolicy.sanitizeMessageId(String(repeating: "a", count: 64)),
            String(repeating: "a", count: 64))
    }

    // MARK: - Frames we send

    func testIdentifyCarriesTheVersion() throws {
        let json = try frame(GatewayAttachPolicy.identifyJson(deviceId: "off1alice"))
        XCTAssertEqual(json["type"] as? String, "Identify")
        XCTAssertEqual(json["device_id"] as? String, "off1alice")
        XCTAssertEqual(json["protocol_version"] as? Int, 1)
    }

    func testDeclarationCarriesBase64Fields() throws {
        let key = Data(repeating: 1, count: 32)
        let sig = Data(repeating: 2, count: 64)
        let json = try frame(
            GatewayAttachPolicy.declarationJson(
                address: "off1alice", publicKey: key, signature: sig))
        XCTAssertEqual(json["type"] as? String, "DeclareAddress")
        XCTAssertEqual(json["address"] as? String, "off1alice")
        XCTAssertEqual(Data(base64Encoded: json["public_key"] as? String ?? ""), key)
        XCTAssertEqual(Data(base64Encoded: json["signature"] as? String ?? ""), sig)
    }

    /// The id goes on the wire. Without it the gateway mints its own and every
    /// verdict comes back uncorrelatable, which is the whole reason the shipped
    /// bridge could not settle on an answer.
    func testSendMessageCarriesTheMessageId() throws {
        let json = try frame(
            GatewayAttachPolicy.sendMessageJson(
                messageId: "abc-123", recipient: "off1bob", content: "Zm9v", replyToMsg: nil))
        XCTAssertEqual(json["type"] as? String, "SendMessage")
        XCTAssertEqual(json["message_id"] as? String, "abc-123")
        XCTAssertEqual(json["recipient"] as? String, "off1bob")
        XCTAssertEqual(json["encoding"] as? String, "base64")
        XCTAssertNil(json["reply_to_msg"])
    }

    func testSendMessageOmitsAnEmptyReplyTo() throws {
        let json = try frame(
            GatewayAttachPolicy.sendMessageJson(
                messageId: "abc", recipient: "off1bob", content: "Zm9v", replyToMsg: ""))
        XCTAssertNil(json["reply_to_msg"])
    }

    /// One frame for the batch, which is the contract's shape and not the
    /// relay's one-peer-per-frame query.
    func testCheckPresenceAsksAboutEveryPeerInOneFrame() throws {
        let json = try frame(
            GatewayAttachPolicy.checkPresenceJson(peers: ["off1a", "off1b", "off1c"]))
        XCTAssertEqual(json["type"] as? String, "CheckPresence")
        XCTAssertEqual(json["peers"] as? [String], ["off1a", "off1b", "off1c"])
    }

    /// Asking about more than the gateway answers only guarantees silence for
    /// the surplus, and a caller waiting per peer would wait forever.
    func testCheckPresenceIsCappedAtWhatAGatewayAnswers() throws {
        let peers = (0..<200).map { "off1peer\($0)" }
        let json = try frame(GatewayAttachPolicy.checkPresenceJson(peers: peers))
        XCTAssertEqual(
            (json["peers"] as? [String])?.count, GatewayAttachPolicy.MAX_PRESENCE_PEERS)
    }

    func testCheckPresenceWithNoPeersIsNotSent() {
        XCTAssertNil(GatewayAttachPolicy.checkPresenceJson(peers: []))
    }

    // MARK: - Frames we read

    func testAMessageSentSettlesItsId() {
        let verdict = GatewayAttachPolicy.parseVerdict(
            ["message_id": "abc", "recipient": "off1bob"], type: "MessageSent")
        XCTAssertEqual(verdict?.messageId, "abc")
        XCTAssertTrue(verdict?.sent ?? false)
        XCTAssertNil(verdict?.reason)
    }

    /// The reason travels verbatim: the core classifies on the
    /// `recipient_unreachable` prefix and discards the rest, so nothing here
    /// needs to parse it.
    func testADeliveryErrorCarriesItsReasonUntouched() {
        let verdict = GatewayAttachPolicy.parseVerdict(
            ["message_id": "abc", "reason": "recipient_unreachable: not attached here"],
            type: "DeliveryError")
        XCTAssertFalse(verdict?.sent ?? true)
        XCTAssertEqual(verdict?.reason, "recipient_unreachable: not attached here")
    }

    func testAVerdictWithNoIdSettlesNothing() {
        XCTAssertNil(GatewayAttachPolicy.parseVerdict([:], type: "MessageSent"))
        XCTAssertNil(GatewayAttachPolicy.parseVerdict(["message_id": ""], type: "DeliveryError"))
    }

    func testPresenceIsRead() {
        let answer = GatewayAttachPolicy.parsePresence([
            "peer": "off1bob", "online": true, "last_seen_ms": 1_786_924_800_000,
        ])
        XCTAssertEqual(answer?.peer, "off1bob")
        XCTAssertEqual(answer?.online, true)
        XCTAssertEqual(answer?.lastSeenMs, 1_786_924_800_000)
    }

    /// A missing `online` is not readable as "offline": that manufactures a
    /// claim the gateway never made, and a claim is what drives parking.
    func testPresenceWithoutAnOnlineFlagIsNotAClaim() {
        XCTAssertNil(GatewayAttachPolicy.parsePresence(["peer": "off1bob"]))
        XCTAssertNil(GatewayAttachPolicy.parsePresence(["online": true]))
        XCTAssertNil(GatewayAttachPolicy.parsePresence(["peer": "", "online": false]))
    }

    func testPresenceWithoutLastSeenIsStillAnAnswer() {
        let answer = GatewayAttachPolicy.parsePresence(["peer": "off1bob", "online": false])
        XCTAssertEqual(answer?.online, false)
        XCTAssertNil(answer?.lastSeenMs)
    }

    // MARK: - Constants

    /// The verdict timeout must stay below the core's pending-confirmation
    /// expiry, or the core settles the frame first and the verdict then
    /// arrives for an id it has already moved past. A Rust guard pins the same
    /// relationship across both bridges; this is the local half.
    func testTheVerdictTimeoutIsShorterThanTheCoresOwnExpiry() {
        XCTAssertLessThan(GatewayAttachPolicy.VERDICT_TIMEOUT, 120.0)
        XCTAssertLessThan(GatewayAttachPolicy.ATTACH_TIMEOUT, 60.0)
    }

    // MARK: - Helpers

    private func frame(_ json: String?) throws -> [String: Any] {
        let text = try XCTUnwrap(json)
        let data = try XCTUnwrap(text.data(using: .utf8))
        return try XCTUnwrap(JSONSerialization.jsonObject(with: data) as? [String: Any])
    }
}
