//
// ReferenceStorage.swift
// OfflineProtocol
//
// The built-in MLS and protocol-state stores, opened for one account.
//

import Foundation

/// Opens, and erases, the SDK's built-in storage for one `(appId, profile)`
/// account: the Keychain-backed MLS store and the app-container protocol-state
/// store.
///
/// This is the one place those two stores are wired together. The React Native
/// module and the engine host both call it, so a native application that calls
/// it gets the same stores, the same account namespace and the same legacy
/// adoption a React Native application gets. A second copy of the wiring is how
/// the two drift: a host that derived the namespace differently would open an
/// empty store and start a fresh identity with no error.
public enum OfflineProtocolReferenceStorage {

    /// What happened to the store written before storage was split per
    /// account, the first time this account's secure store was opened.
    ///
    /// `.conflict` and `.claimUnverified` mean this account starts from a
    /// fresh MLS identity. They must reach someone: `warning` is the text the
    /// React Native module reports for them.
    public enum LegacyAdoption: Equatable {
        /// There was no earlier store to inherit.
        case none
        /// This account inherited the earlier store on this launch.
        case adopted
        /// This account inherited the earlier store on a previous launch.
        case resumed
        /// The earlier store belongs to another account. This account starts
        /// from a fresh identity, an empty outbox and an empty block list.
        case conflict
        /// This account's claim on the earlier store could not be recorded, so
        /// it was not inherited. This account starts from a fresh identity, an
        /// empty outbox and an empty block list.
        case claimUnverified

        /// Whether this account starts from a fresh MLS identity although an
        /// earlier one exists on the device.
        public var startsFreshIdentity: Bool {
            self == .conflict || self == .claimUnverified
        }

        /// The error to report when `startsFreshIdentity`, otherwise nil.
        public var warning: String? {
            switch self {
            case .conflict:
                return "Legacy secure store belongs to another account; this account starts from a fresh MLS "
                    + "identity and cannot decrypt its previous sessions. Its pre-split delivery state is "
                    + "unreachable too, so it also comes up with an empty outbox and an empty block list: "
                    + "every previously blocked peer is unblocked"
            case .claimUnverified:
                return "Could not record this account's claim on the legacy secure store, so it was not adopted: "
                    + "another account could otherwise inherit the same MLS identity. This account starts "
                    + "from a fresh identity and comes up with an empty outbox and an empty block list: "
                    + "every previously blocked peer is unblocked. The credential store is failing writes; "
                    + "retrying on a healthy store adopts normally"
            case .none, .adopted, .resumed:
                return nil
            }
        }

        init(_ decision: LegacyStoreAdoption.Decision?) {
            switch decision {
            case .adopt?: self = .adopted
            case .resume?: self = .resumed
            case .conflict?: self = .conflict
            case .claimUnverified?: self = .claimUnverified
            case nil: self = .none
            }
        }
    }

    /// The Keychain-backed MLS store for one account.
    public static func makeSecureStorage(appId: String, profile: String) throws -> MlsStorageProvider {
        try MlsSecureStorage(accountNamespace: StorageNamespace.account(appId: appId, profile: profile))
    }

    /// The app-container protocol-state store for one account.
    public static func makeProtocolStateStorage(
        appId: String,
        profile: String
    ) throws -> ProtocolStateStorageProvider {
        try AppContainerProtocolStateStorage(
            accountNamespace: StorageNamespace.account(appId: appId, profile: profile)
        )
    }

    /// Opens both stores for one account and hands them to `initializeMls`.
    ///
    /// Call it after `setEventCallback` and before `start()`, with the
    /// `appId` and `profile` the engine was configured with. Report the
    /// result's `warning` when it has one: it is the only signal that this
    /// account lost its earlier identity.
    public static func initializeMls(
        on mesh: OfflineProtocol,
        appId: String,
        profile: String
    ) throws -> LegacyAdoption {
        let namespace = StorageNamespace.account(appId: appId, profile: profile)
        let secure = try MlsSecureStorage(accountNamespace: namespace)
        let state = try AppContainerProtocolStateStorage(accountNamespace: namespace)
        try mesh.initializeMls(secureStorage: secure, protocolStateStorage: state)
        return LegacyAdoption(secure.legacyAdoption)
    }

    /// Erases everything the built-in stores hold for one account: its MLS
    /// identity, sessions and pins, its outbox and its protocol state.
    ///
    /// Stop the engine for this account and release it first. A running engine
    /// keeps writing, and would write into the stores this just emptied.
    ///
    /// The secure store goes first: it holds the key every sealed
    /// protocol-state record is written under, so an interrupted wipe leaves
    /// the rest as ciphertext nobody can open rather than readable state. Both
    /// are attempted even if the first fails, and the first error is rethrown.
    /// Idempotent: call again after an error.
    public static func wipe(appId: String, profile: String) throws {
        let namespace = StorageNamespace.account(appId: appId, profile: profile)
        var firstError: Error?
        do {
            try MlsSecureStorage.wipeAccount(accountNamespace: namespace)
        } catch {
            firstError = error
        }
        do {
            try AppContainerProtocolStateStorage.wipeAccount(accountNamespace: namespace)
        } catch {
            firstError = firstError ?? error
        }
        if let firstError { throw firstError }
    }
}
