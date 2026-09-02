//
// GatewayAttachPolicy.swift
// OfflineProtocol
//
// The gateway daemon contract's decisions and frame shapes, with no socket
// and no UniFFI, so they can be unit-tested. See
// docs/spec/gateway-contract.md.
//

import Foundation

/// Pure decisions and frame shapes for gateway daemon contract v1.
///
/// The manager owns the socket, the queues and the lifecycle; everything here
/// is a function of its arguments. That split is the only way any of this gets
/// tested on iOS: `ReticulumManager.swift` is on `Package.swift`'s `exclude:`
/// list because it needs the generated UniFFI module, so the SwiftPM harness
/// compiles none of it.
///
/// **What is deliberately not here: the signed proof.** The bytes a device
/// signs to attach are built and signed in the core, behind
/// `gatewayAddressDeclaration(challenge:)`. The relay's equivalent had to live
/// on this side because it commits the relay's account name, which only a
/// bridge knows, and it is now hand-mirrored in three languages. This one
/// commits our own address, so it exists once where the conformance vectors
/// can pin it. What is left here is the framing around it.
public enum GatewayAttachPolicy {

    // MARK: - Wire constants

    /// The contract version this client speaks, sent on `Identify`.
    public static let PROTOCOL_VERSION = 1

    /// Bytes of challenge the gateway mints per connection. A declaration is
    /// not attempted for anything else: the core refuses to sign it, and
    /// finding that out from a thrown error is worse than not asking.
    public static let CHALLENGE_LENGTH = 32

    /// How long the whole handshake may take, from `Identify` to
    /// `StatusUpdate(connected)`.
    ///
    /// Far shorter than the 60s connection timeout, and deliberately so: TCP
    /// to the daemon is a LAN hop, and a gateway that has accepted the socket
    /// but not finished the handshake is not slow, it is broken or wedged.
    /// Waiting a minute to find that out means a minute of a carrier the
    /// selector has been told nothing about.
    public static let ATTACH_TIMEOUT: TimeInterval = 10.0

    /// How long a submitted frame may go without a verdict before this client
    /// treats the gateway's silence as a failure.
    ///
    /// **Must stay below the core's 120s pending-confirmation timeout**, which
    /// is the other clock on the same frame. If this were the longer of the
    /// two, the core would expire the frame first and count it a failure, and
    /// the verdict arriving afterwards would confirm or fail a message id the
    /// core had already settled. A Rust guard pins both this number and that
    /// relationship.
    public static let VERDICT_TIMEOUT: TimeInterval = 60.0

    /// Frames submitted but not yet answered. The gateway answers every
    /// submission, so this bounds nothing but memory and the size of the loss
    /// when a connection dies; it is also roughly what the core's own session
    /// bootstrap bursts, so a smaller number would throttle the one case that
    /// matters most.
    public static let MAX_IN_FLIGHT = 8

    /// Longest line this client will assemble before abandoning the stream.
    ///
    /// A frame at the gateway's cap arrives base64-encoded, so 4/3 of its size
    /// plus the JSON around it, and the buffer has to hold the largest one the
    /// gateway can legitimately send. Past this the stream is not
    /// resynchronisable — the rest of the over-long line would be read as a
    /// fresh one — so the connection goes rather than the line.
    public static let MAX_LINE_BYTES = 1 << 20

    /// Capability bounds, matching the relay's, which is what the contract
    /// points at rather than inventing a second pair.
    public static let MAX_CAPABILITY_TOKENS = 64
    public static let MAX_CAPABILITY_TOKEN_BYTES = 128

    /// Peers a gateway answers per `CheckPresence`. Asking about more only
    /// guarantees silence for the ones past the cap.
    public static let MAX_PRESENCE_PEERS = 64

    // MARK: - Attach

    /// Why a declaration was not attempted. Reported as a diagnostic; the
    /// carrier stays unavailable either way.
    public enum SkipReason {
        public static let addressUnavailable = "address_unavailable"
        public static let challengeAbsent = "challenge_absent"
        public static let challengeMalformed = "challenge_malformed"
        public static let challengeWrongSize = "challenge_wrong_size"
        public static let signingFailed = "signing_failed"
        public static let frameUnserializable = "frame_unserializable"
    }

    /// What a `Challenge` frame yielded.
    public enum ChallengeOutcome: Equatable {
        case declare(challenge: Data)
        case skip(reason: String)
    }

    /// What a gateway's `AddressDeclared` echo says about this device.
    ///
    /// The same three answers the core reports on, decided again here because
    /// the two act on different things: the core emits the security warning,
    /// and the bridge decides whether this carrier can be offered at all.
    public enum BindingOutcome: Equatable {
        /// The gateway bound the address we declared. The session is proven.
        case bound
        /// It bound something else. A security event, not a retry.
        case mismatch
        /// We hold no address to compare against, so nothing here was ever
        /// declared by us.
        case unknownLocal
    }

    /// Reads the challenge out of a `Challenge` frame.
    ///
    /// The size is checked here as well as in the core because the two
    /// refusals mean different things to the reader: this one says the
    /// gateway is not speaking the contract, and the core's says something
    /// asked it to sign a payload it should not.
    public static func decodeChallenge(_ json: [String: Any]) -> ChallengeOutcome {
        guard let encoded = json["challenge"] as? String, !encoded.isEmpty else {
            return .skip(reason: SkipReason.challengeAbsent)
        }
        guard let challenge = Data(base64Encoded: encoded) else {
            return .skip(reason: SkipReason.challengeMalformed)
        }
        guard challenge.count == CHALLENGE_LENGTH else {
            return .skip(reason: SkipReason.challengeWrongSize)
        }
        return .declare(challenge: challenge)
    }

    /// Compares the gateway's echo with this device's own address.
    public static func bindingOutcome(declared: String, local: String?) -> BindingOutcome {
        guard let local = local, !local.isEmpty else { return .unknownLocal }
        return declared == local ? .bound : .mismatch
    }

    /// The capability tokens worth storing, bounded on the way in.
    ///
    /// Oversized tokens are dropped **before** the count is applied, so a
    /// gateway cannot pad its list to evict the tokens that matter.
    public static func capabilityTokens(from json: [String: Any]) -> [String] {
        guard let raw = json["tokens"] as? [Any] else { return [] }
        return raw.compactMap { $0 as? String }
            .filter { !$0.isEmpty && $0.utf8.count <= MAX_CAPABILITY_TOKEN_BYTES }
            .prefix(MAX_CAPABILITY_TOKENS)
            .map { $0 }
    }

    // MARK: - Message ids

    /// The gateway's own rule for a client-supplied id: 1 to 64 characters of
    /// `[A-Za-z0-9._-]`.
    ///
    /// Applied before sending, not after: an id the gateway would refuse is
    /// replaced *there* by one it mints, and the verdict then comes back under
    /// a name nothing here is waiting on. Message ids are UUIDs, which pass;
    /// this is what keeps that from being an assumption.
    public static func sanitizeMessageId(_ candidate: String?) -> String? {
        guard let candidate = candidate, !candidate.isEmpty, candidate.count <= 64 else {
            return nil
        }
        let allowed = CharacterSet(charactersIn:
            "ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789._-")
        return candidate.unicodeScalars.allSatisfy { allowed.contains($0) } ? candidate : nil
    }

    // MARK: - Frames this client sends

    public static func identifyJson(deviceId: String) -> String? {
        serialize([
            "type": "Identify",
            "device_id": deviceId,
            "protocol_version": PROTOCOL_VERSION,
        ])
    }

    public static func declarationJson(address: String, publicKey: Data, signature: Data) -> String? {
        serialize([
            "type": "DeclareAddress",
            "address": address,
            "public_key": publicKey.base64EncodedString(),
            "signature": signature.base64EncodedString(),
        ])
    }

    public static func sendMessageJson(
        messageId: String,
        recipient: String,
        content: String,
        replyToMsg: String?
    ) -> String? {
        var frame: [String: Any] = [
            "type": "SendMessage",
            "recipient": recipient,
            "content": content,
            "encoding": "base64",
            "message_id": messageId,
        ]
        if let replyToMsg = replyToMsg, !replyToMsg.isEmpty {
            frame["reply_to_msg"] = replyToMsg
        }
        return serialize(frame)
    }

    /// One frame for the whole batch, which is the shape the contract takes
    /// and the opposite of the relay's one-peer-per-frame `CheckPresence`.
    public static func checkPresenceJson(peers: [String]) -> String? {
        let asked = Array(peers.prefix(MAX_PRESENCE_PEERS))
        guard !asked.isEmpty else { return nil }
        return serialize(["type": "CheckPresence", "peers": asked])
    }

    // MARK: - Frames this client reads

    /// A verdict: the id it settles, and the reason if it is a refusal.
    public struct Verdict: Equatable {
        public let messageId: String
        /// `nil` for `MessageSent`, the gateway's own text for
        /// `DeliveryError`. Passed to the core verbatim: the classifier
        /// matches the `recipient_unreachable` prefix and discards the rest,
        /// so nothing here needs to understand it.
        public let reason: String?
        public let recipient: String?

        public var sent: Bool { reason == nil }
    }

    public static func parseVerdict(_ json: [String: Any], type: String) -> Verdict? {
        guard let messageId = json["message_id"] as? String, !messageId.isEmpty else {
            // Nothing to settle. The gateway mints an id for a submission that
            // carried none, but this client always sends one, so a verdict
            // without an id is not ours to act on.
            return nil
        }
        let recipient = json["recipient"] as? String
        if type == "MessageSent" {
            return Verdict(messageId: messageId, reason: nil, recipient: recipient)
        }
        let reason = json["reason"] as? String ?? "DeliveryError"
        return Verdict(messageId: messageId, reason: reason, recipient: recipient)
    }

    public struct PresenceAnswer: Equatable {
        public let peer: String
        public let online: Bool
        public let lastSeenMs: Int64?
    }

    public static func parsePresence(_ json: [String: Any]) -> PresenceAnswer? {
        guard let peer = json["peer"] as? String, !peer.isEmpty else { return nil }
        // A missing or non-boolean `online` is not readable as "offline": that
        // would manufacture a claim the gateway did not make.
        guard let online = json["online"] as? Bool else { return nil }
        let lastSeen = (json["last_seen_ms"] as? NSNumber)?.int64Value
        return PresenceAnswer(peer: peer, online: online, lastSeenMs: lastSeen)
    }

    // MARK: - Plumbing

    private static func serialize(_ frame: [String: Any]) -> String? {
        guard let data = try? JSONSerialization.data(withJSONObject: frame) else { return nil }
        return String(data: data, encoding: .utf8)
    }
}
