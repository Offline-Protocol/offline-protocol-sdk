//
// AddressDeclarationPolicy.swift
// OfflineProtocol
//
// Whether this connection proves its `off1…` address to the relay, and the
// exact bytes it signs to do it.
// Mirrors android/src/main/java/com/offlineprotocol/AddressDeclarationPolicy.kt
// — keep in sync.
//

import Foundation

/// Builds the `DeclareAddress` proof a relay connection needs to be routed by
/// address instead of by account name.
///
/// # Why the declaration exists
///
/// The relay authenticates a JWT and knows the connection by its *account
/// name*, but since the addressing cutover the core stamps `Message.sender`
/// with `local_address()`. A relay that attributes an inbound frame by account
/// name therefore hands the receiver a `transport_peer_id` that cannot match
/// the sender it is strict-matched against in `validate_transport_sender`, and
/// every security-gated control frame (`__MLS_KEY_PKG__`, `__MLS_WELCOME__`)
/// is rejected — no MLS session can be established over the relay at all.
/// Declaring closes that, and is also what makes an `off1…` recipient
/// resolvable in the relay's registry.
///
/// # Why the account name is inside the signed bytes
///
/// The signing key here is the identity key that also signs mesh control
/// frames under `offline-ctrl-v1`. If the proof were a signature over a bare
/// relay-chosen challenge, a hostile relay could hand out a challenge shaped
/// like a control-frame payload and harvest a signature that replays as a
/// control frame from this peer. Binding the domain **and** the account makes
/// the signature meaningful only as "this account, on this relay, holds this
/// key" — it cannot be replayed under another account, nor onto another
/// connection, since the challenge is minted per connection.
///
/// The layout is fixed by the relay's `address_binding::address_proof_payload`
/// and pinned there by a hex vector, which
/// `AddressDeclarationPolicyTests.proofPayloadMatchesThePinnedRelayVector`
/// mirrors byte for byte. Neither side may change it alone.
///
/// # Why every failure is a skip rather than an error
///
/// A connection that does not declare keeps working exactly as it did before
/// addresses existed — the relay attributes it by account name, which is the
/// legacy path and still the only path an older relay has. So the absence of
/// a capability, of a challenge, or of a local identity is a reason to stay
/// quiet, not to fail a connection that is otherwise fine.
enum AddressDeclarationPolicy {

    /// The relay capability token gating the whole exchange. A relay that does
    /// not advertise it also omits `address_challenge`, and would parse a
    /// `DeclareAddress` into nothing and answer nothing — so the token is the
    /// tell, not a timeout.
    static let CAPABILITY = "address_routing_v1"

    /// Domain separator prefixing the signed payload. Must not prefix, nor be
    /// prefixed by, the core's `offline-ctrl-v1` control-frame domain; the
    /// relay pins that relation in
    /// `the_proof_domain_cannot_collide_with_control_message_signing`.
    static let PROOF_DOMAIN = "offline-relay-addr-v1"

    /// The relay mints exactly this many challenge bytes. A frame carrying any
    /// other length is malformed, and signing it would produce a proof that
    /// cannot verify — better to skip and say so.
    static let CHALLENGE_LENGTH = 32

    /// Stable diagnostic reasons, shared with the Kotlin mirror so a field
    /// report reproduces under the same string on both platforms.
    enum Reason {
        /// The relay does not advertise `address_routing_v1` — an older
        /// deployment. Expected, and the reason this is not an error.
        static let capabilityAbsent = "capability_absent"
        /// Capability advertised but no `address_challenge` came with it.
        static let challengeAbsent = "challenge_absent"
        /// The challenge was not standard base64, or did not decode to
        /// exactly `CHALLENGE_LENGTH` bytes.
        static let challengeMalformed = "challenge_malformed"
        /// The `Authenticated` frame carried no account name to bind the
        /// proof to. Never sign a locally-chosen substitute here: the relay
        /// verifies against the name *it* resolved, so a guess produces a
        /// signature that cannot verify and is indistinguishable, in the
        /// relay's logs, from an attack.
        static let accountAbsent = "account_absent"
        /// `local_address()` was nil — MLS is not initialized, so there is no
        /// identity to prove. An app running with encryption disabled stays in
        /// account-name space by construction.
        static let addressUnavailable = "address_unavailable"
        /// The identity key or the signature could not be produced.
        static let signingFailed = "signing_failed"
        /// The proof was built but the frame could not be serialized.
        static let frameUnserializable = "frame_unserializable"
    }

    enum Outcome: Equatable {
        /// Sign `proofPayload(account:challenge:)` and send the declaration.
        case declare(account: String, challenge: Data)
        /// Send nothing; the string is a `Reason`.
        case skip(reason: String)
    }

    /// Decides whether this connection can declare, from the `Authenticated`
    /// frame alone.
    ///
    /// Deliberately does **not** take the local address: this runs before any
    /// FFI call so that the common skip (an older relay) costs no acquisition
    /// of the protocol mutex. The caller fetches the address only after a
    /// `declare`, and reports `Reason.addressUnavailable` if it is nil.
    ///
    /// - Parameters:
    ///   - capabilities: the `capabilities` array, verbatim.
    ///   - addressChallenge: the `address_challenge` field — base64 of the raw
    ///     challenge bytes, absent on relays without the capability.
    ///   - username: the `username` field **as the relay sent it**. Callers
    ///     must not substitute a local fallback (see `Reason.accountAbsent`).
    static func decide(
        capabilities: [String],
        addressChallenge: String?,
        username: String?
    ) -> Outcome {
        guard capabilities.contains(CAPABILITY) else {
            return .skip(reason: Reason.capabilityAbsent)
        }
        guard let challengeBase64 = addressChallenge, !challengeBase64.isEmpty else {
            return .skip(reason: Reason.challengeAbsent)
        }
        // Standard alphabet, padding required — the relay decodes with the
        // same, and rejects base64url or unpadded input.
        guard let challenge = Data(base64Encoded: challengeBase64),
              challenge.count == CHALLENGE_LENGTH else {
            return .skip(reason: Reason.challengeMalformed)
        }
        guard let account = username, !account.isEmpty else {
            return .skip(reason: Reason.accountAbsent)
        }
        return .declare(account: account, challenge: challenge)
    }

    /// The exact bytes the relay verifies the signature over.
    ///
    ///     "offline-relay-addr-v1" ‖ u32be(account.utf8.count) ‖ account.utf8 ‖ challenge
    ///
    /// No separators, no terminators. The length prefix is what makes the
    /// concatenation unambiguous — without it, an account name ending in bytes
    /// that look like the start of a challenge could be re-split, so two
    /// different (account, challenge) pairs would share one payload.
    static func proofPayload(account: String, challenge: Data) -> Data {
        let accountBytes = Array(account.utf8)
        var payload = Data()
        payload.append(contentsOf: Array(PROOF_DOMAIN.utf8))
        // Big-endian, and of the UTF-8 *byte* count — not the character count,
        // which differs for any non-ASCII account name.
        var length = UInt32(accountBytes.count).bigEndian
        withUnsafeBytes(of: &length) { payload.append(contentsOf: $0) }
        payload.append(contentsOf: accountBytes)
        payload.append(challenge)
        return payload
    }

    /// The `DeclareAddress` frame, serialized.
    ///
    /// Owns the base64 encoding so the wire contract lives in one place per
    /// platform: standard alphabet, padded, no line breaks — a 32-byte key
    /// encodes to 44 characters and a 64-byte signature to 88. The relay
    /// decodes with a strict engine and refuses anything else.
    ///
    /// Returns nil only if serialization fails, which three `String` values
    /// cannot cause; the caller reports `Reason.frameUnserializable` rather
    /// than putting a malformed frame on the wire.
    static func declarationJson(address: String, publicKey: Data, signature: Data) -> String? {
        let frame: [String: Any] = [
            "type": "DeclareAddress",
            "address": address,
            "public_key": publicKey.base64EncodedString(),
            "signature": signature.base64EncodedString()
        ]
        guard let data = try? JSONSerialization.data(withJSONObject: frame, options: []),
              let json = String(data: data, encoding: .utf8) else {
            return nil
        }
        return json
    }
}
