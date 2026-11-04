//
// BleServiceInstanceSelection.swift
//
// Which of several Offline Protocol service instances on one BLE link a
// central binds to.
// Mirrors android's BleServiceInstanceSelection.kt. Keep in sync.
//

import Foundation

/// Picks the one service instance a central handshakes with when a remote
/// phone presents several.
///
/// A phone running more than one SDK app presents one instance of the service
/// per app, all behind a single BLE link (see `BleAppTag`). The link still
/// carries exactly one peer: the peripheral role attributes an inbound write to
/// a sender by the link it arrived on, so a second identity on the same link
/// would have its hop-0 control frames refused by the core's
/// `validate_transport_sender`. The central therefore chooses one instance,
/// and everything after the choice (the DEVICE_ID and IDENTITY reads, the
/// subscription, every write) goes to that instance only.
///
/// # Why it never refuses the link
///
/// A phone running only apps other than ours is still a mesh neighbour: relay
/// forwarding is keyed on the recipient and never on the app, so that phone
/// carries our traffic today. Refusing it would regress relaying to fix
/// discovery. When no instance is ours the choice falls back to the first
/// instance, which is what the central did before this type existed.
///
/// # Why a single instance never reaches this type
///
/// The callers only consult it with two or more instances. A link with one
/// instance runs the handshake exactly as it did before the tag existed and
/// does not read the tag at all, so no working deployment changes behaviour.
enum BleServiceInstanceSelection {

    /// Stable diagnostic reasons, shared with the Kotlin mirror so a trace
    /// reads the same on both platforms.
    enum Reason {
        /// An instance served this app's tag.
        static let sameApp = "same_app"
        /// No instance served this app's tag, and exactly one served none: a
        /// build that predates the tag, most plausibly this app's own.
        static let soleUntagged = "sole_untagged"
        /// No basis to prefer any instance; the first one, as before the tag.
        static let firstInstance = "first_instance"
        /// No instance can complete the handshake. The first is returned so
        /// the handshake refuses it for the characteristic it lacks, as it
        /// did before the tag.
        static let noneCanHandshake = "none_can_handshake"
    }

    /// What one service instance offered, in the order the platform reported
    /// the instances.
    struct Candidate: Equatable {
        /// The APP_TAG value, or nil when the instance serves none (a build
        /// that predates the tag) or the read failed or timed out.
        let tag: Data?
        /// Whether the instance exposes both DEVICE_ID and IDENTITY. One that
        /// lacks either can never complete the handshake.
        let canHandshake: Bool
        /// The DEVICE_ID value, read only by a central whose application has
        /// a publisher key (`selectKeyed`), and only on an instance that
        /// served a tag; nil otherwise, or when the read failed.
        var deviceId: String? = nil
    }

    struct Selection: Equatable {
        /// Index into the candidates passed to `select`.
        let index: Int
        /// One of `Reason`.
        let reason: String
    }

    /// Chooses an instance. `candidates` is never empty at the call sites; an
    /// empty list returns index 0 with `noneCanHandshake`.
    ///
    /// In order: the first handshake-capable instance serving `ownTag`; else,
    /// when exactly one handshake-capable instance serves no tag, that one;
    /// else the first handshake-capable instance; else index 0.
    static func select(_ candidates: [Candidate], ownTag: Data) -> Selection {
        select(candidates) { $0.tag == ownTag }
    }

    /// Chooses an instance for a central whose application has a publisher
    /// key (ADR 0037). Its tag is per instance, `HMAC(K_app, device_id)[0..8]`,
    /// so rule 1 takes the first handshake-capable instance whose tag equals
    /// `tagFor` over that instance's own `deviceId`; rules 2 and 3 are
    /// `select`'s. The Device id is unproven here, so a match only chooses the
    /// instance: the handshake that follows still proves its address.
    ///
    /// - Parameter tagFor: the engine's `bleAppTag`, or nil for a Device id
    ///   it has no tag for.
    static func selectKeyed(_ candidates: [Candidate], tagFor: (String) -> Data?) -> Selection {
        select(candidates) { candidate in
            guard let tag = candidate.tag, let deviceId = candidate.deviceId,
                  let expected = tagFor(deviceId) else { return false }
            return tag == expected
        }
    }

    private static func select(_ candidates: [Candidate], isOurs: (Candidate) -> Bool) -> Selection {
        let usable = candidates.indices.filter { candidates[$0].canHandshake }

        if let sameApp = usable.first(where: { isOurs(candidates[$0]) }) {
            return Selection(index: sameApp, reason: Reason.sameApp)
        }

        let untagged = usable.filter { candidates[$0].tag == nil }
        if untagged.count == 1 {
            return Selection(index: untagged[0], reason: Reason.soleUntagged)
        }

        if let first = usable.first {
            return Selection(index: first, reason: Reason.firstInstance)
        }
        return Selection(index: 0, reason: Reason.noneCanHandshake)
    }
}
