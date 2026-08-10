//
// PeerIdentityBinding.swift
//
// Whether a BLE peer has proved the address it advertises.
// Mirrors android's PeerIdentityBinding.kt — keep in sync.
//

import Foundation

/// Decides whether a discovered BLE peer may be announced to the protocol
/// layer, and under which identifier.
///
/// A peer serves two GATT characteristics: `DEVICE_ID` (`6E400003…`), a bare
/// string, and `IDENTITY` (`6E400004…`), an Ed25519 public key with a
/// signature over its mesh advertisement. Only the second proves anything.
/// This type is the rule that joins them: the advertised string is accepted
/// only when it equals the address derived from the key that signed the
/// identity blob, and the value handed onward is always the **derived** one.
///
/// # Why the derived address is what gets announced
///
/// The two are equal whenever [`resolve`] returns `verified`, so returning the
/// derived value costs nothing — but it means no code path can announce a
/// string that came from the unauthenticated characteristic. The announced id
/// becomes `Message.recipient` for every outbound frame, the key of the Rust
/// `peers` and `peer_mtus` maps, and the `transport_peer_id` the core matches
/// against `Message.sender` in `validate_transport_sender`. Sourcing it from
/// the proof rather than from the claim keeps a future relaxation of the
/// comparison from silently reopening the hole.
///
/// # Why a mismatch is fatal rather than degrading
///
/// Announcing an unproven id is exactly the unauthenticated-advertisement hole
/// this closes: it seeds the routing table, the app's peer list, and the mesh
/// controller under a name the peer cannot prove. There is no useful "accept
/// but flag" state — the id is either load-bearing or it is not. A peer that
/// serves no identity, or one whose identity names a different address, is
/// therefore not surfaced at all.
///
/// A build that still advertises its app-chosen profile in `DEVICE_ID` lands
/// on `mismatch` and stays invisible. That is the intended cutover behaviour:
/// its frames would be rejected by the core's control gate anyway, since the
/// `Message.sender` it stamps is its derived address.
enum PeerIdentityBinding {

    /// Stable diagnostic reasons, shared with the Kotlin mirror so a bug
    /// reproduces under the same string on both platforms.
    enum Reason {
        /// The peer served no `DEVICE_ID`, or served an empty one.
        static let missingDeviceId = "device_id_missing"
        /// No verified identity: absent, undecodable, bad signature, or a key
        /// that would not derive.
        static let unverifiedIdentity = "identity_unverified"
        /// Both present, and they name different addresses.
        static let addressMismatch = "identity_address_mismatch"
    }

    enum Outcome: Equatable {
        /// Announce the peer under this address.
        case verified(peerId: String)
        /// Surface nothing and drop the link; the string is a `Reason`.
        case rejected(reason: String)
    }

    /// Joins the advertised `DEVICE_ID` to the address derived from the peer's
    /// verified `IDENTITY` key.
    ///
    /// - Parameters:
    ///   - advertisedDeviceId: the raw `DEVICE_ID` characteristic value, or
    ///     `nil` if it was never read.
    ///   - derivedAddress: `deriveAddress(IDENTITY.publicKey)`, passed **only**
    ///     when the identity blob decoded and its signature verified. Callers
    ///     must not pass a derived address for an unverified blob — the
    ///     signature is what makes the derivation mean anything.
    ///
    /// Comparison is exact. Addresses are canonical bech32m as produced by the
    /// core's single `derive_address` implementation, so an equal-but-differently-
    /// cased advertisement is a peer that did not derive its own id the way this
    /// one did, and is refused rather than normalised into agreement.
    static func resolve(advertisedDeviceId: String?, derivedAddress: String?) -> Outcome {
        guard let advertised = advertisedDeviceId, !advertised.isEmpty else {
            return .rejected(reason: Reason.missingDeviceId)
        }
        guard let derived = derivedAddress, !derived.isEmpty else {
            return .rejected(reason: Reason.unverifiedIdentity)
        }
        guard advertised == derived else {
            return .rejected(reason: Reason.addressMismatch)
        }
        return .verified(peerId: derived)
    }
}
