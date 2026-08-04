//
// MlsSecureStorage.swift
// OfflineProtocol
//
// Built-in MLS storage implementation using iOS Keychain.
// This provides secure, hardware-backed storage for MLS cryptographic material.
//

import Foundation
import Security

/// Built-in MLS storage using iOS Keychain for secure key material storage.
///
/// This implementation:
/// - Uses kSecAttrAccessibleAfterFirstUnlockThisDeviceOnly for security
/// - Stores data device-bound (not synced to iCloud Keychain)
/// - Provides atomic operations for thread safety
/// - Adopts the pre-namespace store on upgrade (see `LegacyStoreAdoption`)
final class MlsSecureStorage: MlsStorageProvider {
    private let service: String
    private let legacyService: String?
    private let accessGroup: String?

    /// Serialises every Keychain primitive in the process.
    ///
    /// Deliberately `static`, matching `AppContainerProtocolStateStorage`.
    /// Two instances over one service are not hypothetical — the React Native
    /// bridge constructs a fresh provider on every `initializeMls` — and
    /// `wipeAccount` deletes a whole service without holding an instance at
    /// all, so a per-instance lock could not order it against a live writer.
    ///
    /// Lock order is always `adoptionLock` → `lock`; nothing acquires them the
    /// other way round.
    private static let lock = NSLock()

    /// Serialises legacy-store adoption — and legacy-store *destruction* —
    /// across every instance in the process.
    ///
    /// Deliberately not `lock` above: adoption is a read-modify-write over the
    /// claim, so it has to exclude other accounts for its whole duration rather
    /// than per primitive. See `resolveLegacyAdoption`.
    private static let adoptionLock = NSLock()

    /// Bundle component every service name is derived from.
    private static var bundleComponent: String {
        Bundle.main.bundleIdentifier ?? "com.offlineprotocol"
    }

    /// The namespaced service for one account.
    ///
    /// Shared by `init` and `wipeAccount` so the two can never disagree about
    /// which service an account's material lives in — a wipe that computed a
    /// service name of its own would silently erase nothing.
    private static func namespacedService(prefix: String?, namespace: String) -> String {
        "\(prefix ?? bundleComponent + ".mls.v2").\(namespace)"
    }

    /// The pre-namespace service. It predates namespacing *and* the ".v2"
    /// prefix, so it is only ever the default, un-suffixed name.
    private static var legacyServiceName: String {
        bundleComponent + ".mls"
    }

    /// Outcome of the one-time legacy-store adoption, for the caller to
    /// surface. `.conflict` in particular must not pass silently: this account
    /// is starting from a fresh identity.
    private(set) var legacyAdoption: LegacyStoreAdoption.Decision?

    /// Creates a new Keychain-backed MLS storage.
    ///
    /// - Parameters:
    ///   - accountNamespace: Opaque namespace derived from app and user IDs.
    ///   - service: Keychain service prefix (defaults to bundle ID + ".mls.v2")
    ///   - accessGroup: Optional keychain access group for app group sharing
    ///   - adoptLegacyStore: Whether to read through to the pre-namespace store
    ///     (`<bundle>.mls`). Off only for tests that need a clean slate.
    init(
        accountNamespace: String,
        service: String? = nil,
        accessGroup: String? = nil,
        adoptLegacyStore: Bool = true
    ) throws {
        let namespace = try StorageNamespace.requireAccount(accountNamespace)
        self.service = Self.namespacedService(prefix: service, namespace: namespace)
        self.legacyService = (service == nil && adoptLegacyStore)
            ? Self.legacyServiceName
            : nil
        self.accessGroup = accessGroup

        if let legacy = self.legacyService {
            self.legacyAdoption = resolveLegacyAdoption(namespace: namespace, legacy: legacy)
        }
    }

    /// Stores data securely in the Keychain.
    func store(keyType: String, keyId: String, data: [UInt8]) throws {
        try Self.store(
            keyType: keyType,
            keyId: keyId,
            data: data,
            in: service,
            accessGroup: accessGroup
        )
    }

    /// Loads data from the Keychain.
    ///
    /// On a miss, falls through to the adopted legacy store and promotes what
    /// it finds, so an upgraded install keeps its identity, sessions, and TOFU
    /// pins without a bulk migration pass.
    ///
    /// Each primitive below takes `lock`, but this *compound* read-then-promote
    /// is not atomic against `delete`, and deliberately so. Interleaved, they
    /// would resurrect key material: this method could observe a miss in the
    /// namespaced store, read the legacy value, and then promote it after a
    /// concurrent delete had already removed both copies — defeating the very
    /// guarantee `delete` documents.
    ///
    /// That is unreachable because the SDK is the only caller and serialises
    /// every storage operation behind its own mutex: `OfflineProtocol`'s methods
    /// take `&mut self` and the UniFFI wrapper holds them under one lock, so no
    /// two provider calls overlap. Widening the lock to cover the whole compound
    /// operation would mean holding it across a Keychain read on every miss,
    /// which is the common path during an upgrade. If a second caller is ever
    /// given this provider, that trade has to be revisited.
    ///
    /// `wipeAccount` is not that second caller. It touches the same primitives
    /// but only ever for an account with no live instance — the bridge refuses
    /// to wipe the one it is running — so it cannot interleave with a promotion
    /// on the same service.
    ///
    /// A tombstoned key reads as absent without consulting the legacy store at
    /// all: its copy there outlived a delete, and promoting it would resurrect
    /// key material the caller was told was gone.
    func load(keyType: String, keyId: String) throws -> [UInt8]? {
        if LegacyStoreAdoption.isReservedEntry(keyType: keyType) {
            return nil
        }
        if let data = try Self.load(
            keyType: keyType,
            keyId: keyId,
            from: service,
            accessGroup: accessGroup
        ) {
            return data
        }
        guard let legacy = readThroughService(for: keyType) else {
            return nil
        }
        let tombstone = tombstoneState(keyType: keyType, keyId: keyId)
        if tombstone.suppressesReadThrough {
            if tombstone.allowsRemovalRetry {
                // Opportunistic heal: the removal that failed may succeed now,
                // which is the only thing that retires a tombstone. Gated on a
                // *confirmed* tombstone — a read that merely failed must not
                // delete a copy that may still be inheritable.
                retryTombstonedRemoval(from: legacy, keyType: keyType, keyId: keyId)
            }
            return nil
        }
        guard let inherited = try Self.load(
            keyType: keyType,
            keyId: keyId,
            from: legacy,
            accessGroup: accessGroup
        ) else {
            return nil
        }
        // Best-effort promotion: a failed copy still returns the value, it just
        // costs another read-through next launch.
        try? Self.store(
            keyType: keyType,
            keyId: keyId,
            data: inherited,
            in: service,
            accessGroup: accessGroup
        )
        return inherited
    }

    /// Deletes data from the Keychain, and from the legacy store too.
    ///
    /// A delete that left the legacy copy in place would let read-through
    /// resurrect key material the caller believes is gone. When that removal
    /// fails, the key is tombstoned rather than reported: see
    /// `LegacyStoreAdoption.tombstoneKeyType` for why this cannot be signalled
    /// by throwing. The delete has still done what it promised — nothing will
    /// hand that key back — so it returns successfully.
    ///
    /// Only a *double* fault throws: a legacy copy that will not delete and a
    /// namespaced store that will not record the tombstone leaves no way to
    /// keep the promise, and a Keychain failing both is failing everything else
    /// too. Locking a device mid-delete is the realistic single fault, and it
    /// heals on the next read.
    func delete(keyType: String, keyId: String) throws {
        try Self.delete(
            keyType: keyType,
            keyId: keyId,
            from: service,
            accessGroup: accessGroup
        )
        guard let legacy = readThroughService(for: keyType) else {
            return
        }
        do {
            try Self.delete(
                keyType: keyType,
                keyId: keyId,
                from: legacy,
                accessGroup: accessGroup
            )
        } catch {
            try tombstone(keyType: keyType, keyId: keyId, cause: error)
            return
        }
        clearTombstone(keyType: keyType, keyId: keyId)
    }

    /// Lists all key IDs for a given key type, unioned across the adopted
    /// legacy store so a not-yet-promoted entry is still discoverable — except
    /// where a tombstone says that entry is a corpse, which must not be
    /// advertised as a key that can be loaded.
    func listKeys(keyType: String) throws -> [String] {
        if LegacyStoreAdoption.isReservedEntry(keyType: keyType) {
            return []
        }
        var keys = try Self.listKeys(
            keyType: keyType,
            in: service,
            accessGroup: accessGroup
        )
        if let legacy = readThroughService(for: keyType) {
            let inherited = (try? Self.listKeys(
                keyType: keyType,
                in: legacy,
                accessGroup: accessGroup
            )) ?? []
            for key in inherited where !keys.contains(key)
                && !tombstoneState(keyType: keyType, keyId: key).suppressesReadThrough {
                keys.append(key)
            }
        }
        return keys
    }

    // MARK: - Tombstones

    /// Records that a legacy copy survived its deletion.
    ///
    /// - Parameter cause: the removal failure this stands in for, folded into
    ///   the thrown message so a double fault names both halves.
    private func tombstone(keyType: String, keyId: String, cause: Error) throws {
        do {
            try Self.store(
                keyType: LegacyStoreAdoption.tombstoneKeyType,
                keyId: LegacyStoreAdoption.tombstoneKeyId(keyType: keyType, keyId: keyId),
                data: Self.tombstoneValue,
                in: service,
                accessGroup: accessGroup
            )
        } catch {
            throw MlsStorageError.DeleteFailed(
                message: "Keychain delete left an inherited copy of \(keyType) in "
                    + "place (\(cause)) and could not tombstone it: \(error)"
            )
        }
    }

    /// What the namespaced store says about this key's legacy copy.
    ///
    /// A read that throws is `.unreadable`, which fails closed as far as
    /// *reading* goes: read-through cannot be proven safe, and suppressing a
    /// legitimate inherited entry costs an identity rotation while resurrecting
    /// a consumed key costs forward secrecy. It deliberately stops short of
    /// authorising the removal retry — see
    /// `LegacyStoreAdoption.TombstoneState`. Near-unreachable in practice: the
    /// namespaced read in `load` runs first against the same service and would
    /// have thrown.
    private func tombstoneState(
        keyType: String,
        keyId: String
    ) -> LegacyStoreAdoption.TombstoneState {
        do {
            let recorded = try Self.load(
                keyType: LegacyStoreAdoption.tombstoneKeyType,
                keyId: LegacyStoreAdoption.tombstoneKeyId(keyType: keyType, keyId: keyId),
                from: service,
                accessGroup: accessGroup
            ) != nil
            return recorded ? .recorded : .absent
        } catch {
            return .unreadable
        }
    }

    /// Best-effort retry of the legacy removal a tombstone stands in for.
    private func retryTombstonedRemoval(from legacy: String, keyType: String, keyId: String) {
        do {
            try Self.delete(
                keyType: keyType,
                keyId: keyId,
                from: legacy,
                accessGroup: accessGroup
            )
        } catch {
            return
        }
        clearTombstone(keyType: keyType, keyId: keyId)
    }

    /// Retires a tombstone once the legacy copy is genuinely gone.
    ///
    /// Best effort: a tombstone that outlives its corpse only costs the
    /// inherited entry it suppresses, and there is nothing left to resurrect.
    private func clearTombstone(keyType: String, keyId: String) {
        try? Self.delete(
            keyType: LegacyStoreAdoption.tombstoneKeyType,
            keyId: LegacyStoreAdoption.tombstoneKeyId(keyType: keyType, keyId: keyId),
            from: service,
            accessGroup: accessGroup
        )
    }

    /// Value written for a tombstone. Only its *presence* is the signal —
    /// nothing reads the bytes back — so it stays one byte rather than
    /// restating the key.
    private static let tombstoneValue: [UInt8] = [0x01]

    // MARK: - Account wipe

    /// Erases every Keychain item this SDK holds for one account.
    ///
    /// Deliberately `static`: it must run when no instance exists — after
    /// `destroy`, on logout — and constructing one would be actively wrong,
    /// because `init` *claims* the legacy store as a side effect. A wipe that
    /// built a provider first could therefore claim a store on behalf of an
    /// account that is being erased.
    ///
    /// The legacy store goes first. Read-through and the claim both live there,
    /// so wiping the namespaced store first and then failing would leave an
    /// install that re-promotes, on its next launch, exactly the material it was
    /// asked to destroy. In the other order a partial wipe leaves only the
    /// namespaced store, which the next wipe removes and which nothing
    /// re-populates.
    ///
    /// Whether the legacy store may be destroyed at all is
    /// `LegacyStoreAdoption.shouldWipeLegacy`'s decision: it was shared by every
    /// account on a pre-split install, so another account's claim makes it
    /// off-limits.
    ///
    /// Both phases are attempted even if the first fails, and the first error is
    /// rethrown afterwards — a Keychain failure on the legacy store must not
    /// strand the namespaced one, which is where everything written since the
    /// storage split lives. Idempotent: a caller that gets an error should call
    /// again.
    ///
    /// - Parameters:
    ///   - accountNamespace: Namespace from `StorageNamespace.account`.
    ///   - service: Keychain service prefix, matching `init`. A custom prefix
    ///     opts out of legacy handling entirely, exactly as it does there.
    ///   - accessGroup: Optional keychain access group, matching `init`.
    ///   - wipeLegacyStore: Off only for tests that must not touch the shared
    ///     pre-namespace store.
    static func wipeAccount(
        accountNamespace: String,
        service: String? = nil,
        accessGroup: String? = nil,
        wipeLegacyStore: Bool = true
    ) throws {
        let namespace = try StorageNamespace.requireAccount(accountNamespace)
        let namespaced = namespacedService(prefix: service, namespace: namespace)
        let legacy = (service == nil && wipeLegacyStore) ? legacyServiceName : nil

        adoptionLock.lock()
        defer { adoptionLock.unlock() }

        var firstError: Error?

        if let legacy {
            let claim = readClaim(from: legacy, accessGroup: accessGroup)
            if LegacyStoreAdoption.shouldWipeLegacy(claim, namespace: namespace) {
                do {
                    try deleteAll(in: legacy, accessGroup: accessGroup)
                } catch {
                    firstError = firstError ?? error
                }
            }
        }

        do {
            try deleteAll(in: namespaced, accessGroup: accessGroup)
        } catch {
            firstError = firstError ?? error
        }

        if let firstError {
            throw firstError
        }
    }

    /// Deletes every generic-password item filed under one service.
    ///
    /// A service-wide query rather than a walk over `listKeys`: the MLS key-type
    /// set is open — OpenMLS contributes its own labels — so an enumeration
    /// keyed on the types this SDK knows about would leave the rest behind, and
    /// what it left behind would include signing-identity material.
    private static func deleteAll(in service: String, accessGroup: String?) throws {
        lock.lock()
        defer { lock.unlock() }

        var query: [String: Any] = [
            kSecClass as String: kSecClassGenericPassword,
            kSecAttrService as String: service
        ]
        if let accessGroup {
            query[kSecAttrAccessGroup as String] = accessGroup
        }

        let status = SecItemDelete(query as CFDictionary)
        guard status == errSecSuccess || status == errSecItemNotFound else {
            throw MlsStorageError.DeleteFailed(
                message: "Keychain wipe failed for \(service) with status: \(status)"
            )
        }
    }

    // MARK: - Legacy adoption

    /// Resolves — and, when the legacy store is unclaimed, records and then
    /// *verifies* — this account's right to inherit it.
    ///
    /// The claim is read back rather than assumed, because a write that failed
    /// silently would leave the store looking unclaimed to the next account,
    /// which would then adopt the same identity. See
    /// `LegacyStoreAdoption.confirmClaim`.
    ///
    /// The whole probe → claim → read-back sequence runs under `adoptionLock`,
    /// because reading it back is not on its own enough to make inheritance
    /// exclusive. The read back closes a write that silently failed, and a
    /// second account claiming between our probe and our write. It does not
    /// close two accounts interleaving like this:
    ///
    ///     A.readLegacyClaim() -> nil    B.readLegacyClaim() -> nil
    ///     A.store(nsA)
    ///     A.readLegacyClaim() -> nsA  => adopt
    ///                                   B.store(nsB)
    ///                                   B.readLegacyClaim() -> nsB  => adopt
    ///
    /// Both adopt, both promote the same MLS signing identity, and each ends up
    /// holding the other's sessions and group state — the outcome the claim
    /// exists to prevent, arriving silently. The invariant is "at most one
    /// account holds a verified claim", and an unsynchronised read-modify-write
    /// does not provide it. The lock is `static` for the same reason: two
    /// accounts on one device are two instances, so the per-instance `lock`
    /// cannot order them.
    private func resolveLegacyAdoption(
        namespace: String,
        legacy: String
    ) -> LegacyStoreAdoption.Decision {
        Self.adoptionLock.lock()
        defer { Self.adoptionLock.unlock() }

        let decision = LegacyStoreAdoption.decide(
            existingClaim: readLegacyClaim(from: legacy),
            namespace: namespace
        )
        guard decision == .adopt else {
            return decision
        }

        do {
            try Self.store(
                keyType: LegacyStoreAdoption.claimKeyType,
                keyId: LegacyStoreAdoption.claimKeyId,
                data: Array(namespace.utf8),
                in: legacy,
                accessGroup: accessGroup
            )
        } catch {
            return .claimUnverified
        }
        return LegacyStoreAdoption.confirmClaim(
            readBack: readLegacyClaim(from: legacy),
            namespace: namespace
        )
    }

    /// The claim recorded in the legacy store, or `nil` when absent or
    /// unreadable. A failed read is deliberately not distinguished from an
    /// absent claim on the way *in* (both mean "looks unclaimed") but is on the
    /// way back *out*, where it means the claim is unproven.
    private func readLegacyClaim(from legacy: String) -> String? {
        switch Self.readClaim(from: legacy, accessGroup: accessGroup) {
        case .owned(let owner):
            return owner
        case .absent, .unreadable:
            return nil
        }
    }

    /// Reads the legacy store's claim, keeping "not recorded" and "could not be
    /// read" apart.
    ///
    /// `readLegacyClaim` above collapses them, which is right for adoption and
    /// wrong for `wipeAccount` — see `LegacyStoreAdoption.LegacyClaim`. Bytes
    /// that are present but not UTF-8 are decoded lossily and read as *owned*,
    /// never absent: they are still evidence that something claimed the store,
    /// and absence is the classification that authorises destroying it.
    private static func readClaim(
        from legacy: String,
        accessGroup: String?
    ) -> LegacyStoreAdoption.LegacyClaim {
        do {
            guard let raw = try load(
                keyType: LegacyStoreAdoption.claimKeyType,
                keyId: LegacyStoreAdoption.claimKeyId,
                from: legacy,
                accessGroup: accessGroup
            ) else {
                return .absent
            }
            return LegacyStoreAdoption.LegacyClaim.of(bytes: raw)
        } catch {
            return .unreadable
        }
    }

    /// The legacy service to consult for `keyType`, or `nil` when read-through
    /// is off (no legacy store, another account claimed it, or this account
    /// could not prove its own claim).
    private func readThroughService(for keyType: String) -> String? {
        guard let legacy = legacyService,
              let decision = legacyAdoption,
              LegacyStoreAdoption.allowsReadThrough(decision),
              !LegacyStoreAdoption.isReservedEntry(keyType: keyType)
        else {
            return nil
        }
        return legacy
    }

    // MARK: - Keychain primitives

    /// Writes one item, replacing any existing value.
    ///
    /// Add first, then update on `errSecDuplicateItem` — never delete first. A
    /// delete-then-add loses the *previous* value whenever the add fails: the
    /// old item is already gone, the new one never lands, and the throw reports
    /// a failed write for a key that no longer has any value at all. For MLS
    /// material that is unrecoverable — a session's ratchet state, or the
    /// signing identity — where the honest outcome is that a failed write
    /// leaves the last good value in place. Android's `commit()` and Python's
    /// `set_password` both overwrite; this is the same guarantee.
    ///
    /// Both arms pin `kSecAttrAccessibleAfterFirstUnlockThisDeviceOnly`: the
    /// update path has to restate it, because an item's accessibility is not
    /// carried over by a value-only update.
    private static func store(
        keyType: String,
        keyId: String,
        data: [UInt8],
        in service: String,
        accessGroup: String?
    ) throws {
        lock.lock()
        defer { lock.unlock() }

        let key = makeKey(keyType: keyType, keyId: keyId)

        var addQuery = baseQuery(for: key, in: service, accessGroup: accessGroup)
        addQuery[kSecValueData as String] = Data(data)
        addQuery[kSecAttrAccessible as String] = kSecAttrAccessibleAfterFirstUnlockThisDeviceOnly

        let addStatus = SecItemAdd(addQuery as CFDictionary, nil)
        if addStatus == errSecSuccess {
            return
        }
        guard addStatus == errSecDuplicateItem else {
            throw MlsStorageError.StoreFailed(
                message: "Keychain store failed with status: \(addStatus)"
            )
        }

        let updateQuery = baseQuery(for: key, in: service, accessGroup: accessGroup)
        let attributes: [String: Any] = [
            kSecValueData as String: Data(data),
            kSecAttrAccessible as String: kSecAttrAccessibleAfterFirstUnlockThisDeviceOnly
        ]
        let updateStatus = SecItemUpdate(
            updateQuery as CFDictionary,
            attributes as CFDictionary
        )
        guard updateStatus == errSecSuccess else {
            throw MlsStorageError.StoreFailed(
                message: "Keychain overwrite failed with status: \(updateStatus)"
            )
        }
    }

    private static func load(
        keyType: String,
        keyId: String,
        from service: String,
        accessGroup: String?
    ) throws -> [UInt8]? {
        lock.lock()
        defer { lock.unlock() }

        let key = makeKey(keyType: keyType, keyId: keyId)

        var query = baseQuery(for: key, in: service, accessGroup: accessGroup)
        query[kSecReturnData as String] = true
        query[kSecMatchLimit as String] = kSecMatchLimitOne

        var result: AnyObject?
        let status = SecItemCopyMatching(query as CFDictionary, &result)

        switch status {
        case errSecSuccess:
            guard let data = result as? Data else {
                return nil
            }
            return [UInt8](data)
        case errSecItemNotFound:
            return nil
        default:
            throw MlsStorageError.LoadFailed(message: "Keychain load failed with status: \(status)")
        }
    }

    private static func delete(
        keyType: String,
        keyId: String,
        from service: String,
        accessGroup: String?
    ) throws {
        lock.lock()
        defer { lock.unlock() }

        let key = makeKey(keyType: keyType, keyId: keyId)

        let query = baseQuery(for: key, in: service, accessGroup: accessGroup)
        let status = SecItemDelete(query as CFDictionary)

        guard status == errSecSuccess || status == errSecItemNotFound else {
            throw MlsStorageError.DeleteFailed(message: "Keychain delete failed with status: \(status)")
        }
    }

    private static func listKeys(
        keyType: String,
        in service: String,
        accessGroup: String?
    ) throws -> [String] {
        lock.lock()
        defer { lock.unlock() }

        let prefix = "\(keyType):"

        var query: [String: Any] = [
            kSecClass as String: kSecClassGenericPassword,
            kSecAttrService as String: service,
            kSecReturnAttributes as String: true,
            kSecMatchLimit as String: kSecMatchLimitAll
        ]

        if let group = accessGroup {
            query[kSecAttrAccessGroup as String] = group
        }

        var result: AnyObject?
        let status = SecItemCopyMatching(query as CFDictionary, &result)

        switch status {
        case errSecSuccess:
            guard let items = result as? [[String: Any]] else {
                return []
            }
            return items.compactMap { item -> String? in
                guard let account = item[kSecAttrAccount as String] as? String,
                      account.hasPrefix(prefix) else {
                    return nil
                }
                return String(account.dropFirst(prefix.count))
            }
        case errSecItemNotFound:
            return []
        default:
            throw MlsStorageError.LoadFailed(message: "Keychain listKeys failed with status: \(status)")
        }
    }

    // MARK: - Private Helpers

    private static func makeKey(keyType: String, keyId: String) -> String {
        return "\(keyType):\(keyId)"
    }

    private static func baseQuery(
        for key: String,
        in service: String,
        accessGroup: String?
    ) -> [String: Any] {
        var query: [String: Any] = [
            kSecClass as String: kSecClassGenericPassword,
            kSecAttrService as String: service,
            kSecAttrAccount as String: key
        ]

        if let group = accessGroup {
            query[kSecAttrAccessGroup as String] = group
        }

        return query
    }
}
