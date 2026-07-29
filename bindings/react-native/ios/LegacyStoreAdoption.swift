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
/// account can inherit it. The first to launch writes a claim; a second account
/// seeing a foreign claim gets a fresh identity — correct, because the legacy
/// store never held a separable identity for it — but must say so out loud
/// rather than rotate silently.
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
    }

    static func decide(existingClaim: String?, namespace: String) -> Decision {
        guard let existingClaim, !existingClaim.isEmpty else {
            return .adopt
        }
        return existingClaim == namespace ? .resume : .conflict(claimedBy: existingClaim)
    }

    /// True when read-through to the legacy store is permitted.
    static func allowsReadThrough(_ decision: Decision) -> Bool {
        switch decision {
        case .adopt, .resume:
            return true
        case .conflict:
            return false
        }
    }

    /// True for the reserved claim entry, which must never be promoted into the
    /// new store or reported by `listKeys`.
    static func isClaimEntry(keyType: String) -> Bool {
        keyType == claimKeyType
    }
}
