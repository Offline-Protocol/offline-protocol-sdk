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
    private let lock = NSLock()

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
        let bundleComponent = Bundle.main.bundleIdentifier ?? "com.offlineprotocol"
        let servicePrefix = service ?? bundleComponent + ".mls.v2"
        self.service = "\(servicePrefix).\(namespace)"
        // The legacy store predates namespacing and predates the ".v2" prefix,
        // so it is only ever the default, un-suffixed service.
        self.legacyService = (service == nil && adoptLegacyStore)
            ? bundleComponent + ".mls"
            : nil
        self.accessGroup = accessGroup

        if let legacy = self.legacyService {
            self.legacyAdoption = resolveLegacyAdoption(namespace: namespace, legacy: legacy)
        }
    }

    /// Stores data securely in the Keychain.
    func store(keyType: String, keyId: String, data: [UInt8]) throws {
        try store(keyType: keyType, keyId: keyId, data: data, in: service)
    }

    /// Loads data from the Keychain.
    ///
    /// On a miss, falls through to the adopted legacy store and promotes what
    /// it finds, so an upgraded install keeps its identity, sessions, and TOFU
    /// pins without a bulk migration pass.
    func load(keyType: String, keyId: String) throws -> [UInt8]? {
        if let data = try load(keyType: keyType, keyId: keyId, from: service) {
            return data
        }
        guard let legacy = readThroughService(for: keyType) else {
            return nil
        }
        guard let inherited = try load(keyType: keyType, keyId: keyId, from: legacy) else {
            return nil
        }
        // Best-effort promotion: a failed copy still returns the value, it just
        // costs another read-through next launch.
        try? store(keyType: keyType, keyId: keyId, data: inherited, in: service)
        return inherited
    }

    /// Deletes data from the Keychain.
    ///
    /// Deletes from the legacy store too. A delete that left the legacy copy in
    /// place would let read-through resurrect key material the caller believes
    /// is gone.
    func delete(keyType: String, keyId: String) throws {
        try delete(keyType: keyType, keyId: keyId, from: service)
        if let legacy = readThroughService(for: keyType) {
            try? delete(keyType: keyType, keyId: keyId, from: legacy)
        }
    }

    /// Lists all key IDs for a given key type, unioned across the adopted
    /// legacy store so a not-yet-promoted entry is still discoverable.
    func listKeys(keyType: String) throws -> [String] {
        var keys = try listKeys(keyType: keyType, in: service)
        if let legacy = readThroughService(for: keyType) {
            let inherited = (try? listKeys(keyType: keyType, in: legacy)) ?? []
            for key in inherited where !keys.contains(key) {
                keys.append(key)
            }
        }
        return keys
    }

    // MARK: - Legacy adoption

    /// Resolves — and, when the legacy store is unclaimed, records and then
    /// *verifies* — this account's right to inherit it.
    ///
    /// The claim is read back rather than assumed, because a write that failed
    /// silently would leave the store looking unclaimed to the next account,
    /// which would then adopt the same identity. See
    /// `LegacyStoreAdoption.confirmClaim`.
    private func resolveLegacyAdoption(
        namespace: String,
        legacy: String
    ) -> LegacyStoreAdoption.Decision {
        let decision = LegacyStoreAdoption.decide(
            existingClaim: readLegacyClaim(from: legacy),
            namespace: namespace
        )
        guard decision == .adopt else {
            return decision
        }

        do {
            try store(
                keyType: LegacyStoreAdoption.claimKeyType,
                keyId: LegacyStoreAdoption.claimKeyId,
                data: Array(namespace.utf8),
                in: legacy
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
        (try? load(
            keyType: LegacyStoreAdoption.claimKeyType,
            keyId: LegacyStoreAdoption.claimKeyId,
            from: legacy
        )).flatMap { $0 }.flatMap { String(bytes: $0, encoding: .utf8) }
    }

    /// The legacy service to consult for `keyType`, or `nil` when read-through
    /// is off (no legacy store, another account claimed it, or this account
    /// could not prove its own claim).
    private func readThroughService(for keyType: String) -> String? {
        guard let legacy = legacyService,
              let decision = legacyAdoption,
              LegacyStoreAdoption.allowsReadThrough(decision),
              !LegacyStoreAdoption.isClaimEntry(keyType: keyType)
        else {
            return nil
        }
        return legacy
    }

    // MARK: - Keychain primitives

    private func store(
        keyType: String,
        keyId: String,
        data: [UInt8],
        in service: String
    ) throws {
        lock.lock()
        defer { lock.unlock() }

        let key = makeKey(keyType: keyType, keyId: keyId)

        // Delete any existing item first
        let deleteQuery = baseQuery(for: key, in: service)
        SecItemDelete(deleteQuery as CFDictionary)

        // Add new item
        var addQuery = baseQuery(for: key, in: service)
        addQuery[kSecValueData as String] = Data(data)
        addQuery[kSecAttrAccessible as String] = kSecAttrAccessibleAfterFirstUnlockThisDeviceOnly

        let status = SecItemAdd(addQuery as CFDictionary, nil)
        guard status == errSecSuccess else {
            throw MlsStorageError.StoreFailed(message: "Keychain store failed with status: \(status)")
        }
    }

    private func load(
        keyType: String,
        keyId: String,
        from service: String
    ) throws -> [UInt8]? {
        lock.lock()
        defer { lock.unlock() }

        let key = makeKey(keyType: keyType, keyId: keyId)

        var query = baseQuery(for: key, in: service)
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

    private func delete(keyType: String, keyId: String, from service: String) throws {
        lock.lock()
        defer { lock.unlock() }

        let key = makeKey(keyType: keyType, keyId: keyId)

        let query = baseQuery(for: key, in: service)
        let status = SecItemDelete(query as CFDictionary)

        guard status == errSecSuccess || status == errSecItemNotFound else {
            throw MlsStorageError.DeleteFailed(message: "Keychain delete failed with status: \(status)")
        }
    }

    private func listKeys(keyType: String, in service: String) throws -> [String] {
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

    private func makeKey(keyType: String, keyId: String) -> String {
        return "\(keyType):\(keyId)"
    }

    private func baseQuery(for key: String, in service: String) -> [String: Any] {
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
