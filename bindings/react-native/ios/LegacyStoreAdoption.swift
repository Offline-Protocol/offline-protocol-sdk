//
// LegacyStoreAdoption.swift
// OfflineProtocol
//
// Decides whether a namespaced secure store may adopt the pre-namespace one.
//

import Foundation

/// Whether an account may read through to the legacy, un-namespaced secure
/// store that shipped before storage was scoped to `(app_id, user_id)`.
///
/// Scoping the store renamed it. Left alone, the first launch after an upgrade
/// would find an empty store, mint a *new* MLS signing identity, and abandon
/// every session, group, and TOFU pin the install already had — peers still
/// holding the old pin would then reject it. So the new store adopts the old
/// one instead.
///
/// Adoption is read-through rather than a bulk copy on purpose. The legacy
/// store's key types are not a closed set (OpenMLS contributes its own labels
/// and Python's keyring backend cannot enumerate at all), so there is no
/// reliable way to walk it. A miss in the new store consults the legacy one and
/// promotes what it finds, which is naturally idempotent and resumable.
///
/// The legacy store was shared by every account on the install, so at most one
/// account can inherit it. The first to launch writes a claim *and reads it
/// back* — an unverified claim is not an adoption, see `confirmClaim`; a second
/// account seeing a foreign claim gets a fresh identity — correct, because the
/// legacy store never held a separable identity for it — but must say so out
/// loud rather than rotate silently.
///
/// Read-through is also what the SDK's *protocol-state* adoption sweep rides
/// on: pre-split delivery state sits in this same un-namespaced store, and the
/// sweep enumerates the namespaced handle. So a conflict costs more than the
/// MLS identity — that account also comes up with an empty outbox, an empty
/// pending queue, and an empty **block list**, every previously blocked peer
/// unblocked. Say all of it, not just the identity.
///
/// Keep this policy in sync with `LegacyStoreAdoption.kt` and
/// `legacy_store_adoption.py`.
enum LegacyStoreAdoption {
    /// Key under which an adopting account records its claim in the *legacy*
    /// store. Namespaced away from any real key type so it can never collide
    /// with MLS material, and filtered out of read-through and listing.
    static let claimKeyType = "__offline_protocol_migration__"
    static let claimKeyId = "claimed_by"

    enum Decision: Equatable {
        /// Legacy store is unclaimed: claim it and read through.
        case adopt
        /// We already claimed it on an earlier launch: keep reading through.
        case resume
        /// Another account owns the legacy identity. Read-through is off and
        /// this account starts fresh — surface it.
        case conflict(claimedBy: String)
        /// The claim could not be recorded, so ownership is unproven.
        /// Read-through is off — see `confirmClaim`.
        case claimUnverified
    }

    static func decide(existingClaim: String?, namespace: String) -> Decision {
        guard let existingClaim, !existingClaim.isEmpty else {
            return .adopt
        }
        return existingClaim == namespace ? .resume : .conflict(claimedBy: existingClaim)
    }

    /// Confirms a claim by what the legacy store reports *after* the write.
    ///
    /// `decide` returning `.adopt` only means the store looked unclaimed; it is
    /// the recorded claim that makes inheritance exclusive. A write whose
    /// result is not read back is therefore not an adoption: if it silently
    /// failed, the next account to launch also finds the store unclaimed, also
    /// adopts, and the two end up sharing one MLS signing identity — and with
    /// it each other's sessions and group state. That is strictly worse than
    /// the conflict this claim exists to produce, so an unproven claim fails
    /// closed to `.claimUnverified`.
    ///
    /// The cost is a fresh identity for a launch that hit a transient store
    /// failure. Accepted deliberately: confidentiality between two accounts on
    /// one device outranks the sessions of an install whose credential store is
    /// failing writes — and the same failure would break every other write this
    /// session anyway.
    ///
    /// - Parameter readBack: what the legacy store reports for the claim entry
    ///   once the write returned, or `nil` when the write threw or the read
    ///   back failed.
    static func confirmClaim(readBack: String?, namespace: String) -> Decision {
        guard let readBack, !readBack.isEmpty else {
            return .claimUnverified
        }
        return readBack == namespace ? .adopt : .conflict(claimedBy: readBack)
    }

    /// What the legacy store reports about its claim, for a caller deciding
    /// whether it may *destroy* that store.
    ///
    /// Adoption collapses "absent" and "unreadable" into one answer on the way
    /// in — both mean "looks unclaimed", and the worst case is a fresh identity.
    /// A wipe cannot collapse them: the worst case there is deleting the MLS
    /// identity, sessions, and block list of a *different* account that has not
    /// yet had its first post-split launch. So the two are kept apart here.
    enum LegacyClaim: Equatable {
        /// No claim recorded. The store has not been inherited by anyone.
        case absent
        /// The recorded claim, verbatim.
        case owned(by: String)
        /// The claim could not be read, so ownership is unknown.
        case unreadable

        /// Classifies a claim value that was read successfully. An empty value
        /// is absence, matching how `decide` reads it.
        static func of(_ value: String?) -> LegacyClaim {
            guard let value, !value.isEmpty else {
                return .absent
            }
            return .owned(by: value)
        }
    }

    /// Whether `namespace` may delete the legacy store outright.
    ///
    /// Wiping is permitted when this account already owns the claim, and when
    /// the store is unclaimed. Unclaimed is the case that matters in practice:
    /// on the built-in path every account that has completed a post-split
    /// launch has recorded a claim, so an unclaimed store is what the *previous*
    /// install left behind — precisely the leftover a logout is asked to erase,
    /// and (on platforms whose credential store outlives the app container) the
    /// state that would otherwise be re-adopted after a reinstall.
    ///
    /// An unreadable claim fails closed. It is indistinguishable from a foreign
    /// claim, and the two outcomes are not symmetric: refusing costs a leftover
    /// store that the next successful wipe removes, while proceeding can
    /// silently destroy another account's identity and block list.
    static func shouldWipeLegacy(_ claim: LegacyClaim, namespace: String) -> Bool {
        switch claim {
        case .absent:
            return true
        case .owned(let owner):
            return owner == namespace
        case .unreadable:
            return false
        }
    }

    /// True when read-through to the legacy store is permitted.
    static func allowsReadThrough(_ decision: Decision) -> Bool {
        switch decision {
        case .adopt, .resume:
            return true
        case .conflict, .claimUnverified:
            return false
        }
    }

    /// True for the reserved claim entry, which must never be promoted into the
    /// new store or reported by `listKeys`.
    static func isClaimEntry(keyType: String) -> Bool {
        keyType == claimKeyType
    }
}
