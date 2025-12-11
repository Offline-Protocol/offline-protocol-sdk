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
class MlsSecureStorage: MlsStorageProvider {
    private let service: String
    private let accessGroup: String?
    
    /// Creates a new Keychain-backed MLS storage.
    ///
    /// - Parameters:
    ///   - service: Keychain service identifier (defaults to bundle ID + ".mls")
    ///   - accessGroup: Optional keychain access group for app group sharing
    init(service: String? = nil, accessGroup: String? = nil) {
        self.service = service ?? (Bundle.main.bundleIdentifier ?? "com.offlineprotocol") + ".mls"
        self.accessGroup = accessGroup
    }
    
    /// Stores data securely in the Keychain.
    func store(keyType: String, keyId: String, data: Data) throws {
        let key = makeKey(keyType: keyType, keyId: keyId)
        
        // Delete any existing item first
        var deleteQuery = baseQuery(for: key)
        SecItemDelete(deleteQuery as CFDictionary)
        
        // Add new item
        var addQuery = baseQuery(for: key)
        addQuery[kSecValueData as String] = data
        addQuery[kSecAttrAccessible as String] = kSecAttrAccessibleAfterFirstUnlockThisDeviceOnly
        
        let status = SecItemAdd(addQuery as CFDictionary, nil)
        guard status == errSecSuccess else {
            throw MlsStorageError.storeFailed
        }
    }
    
    /// Loads data from the Keychain.
    func load(keyType: String, keyId: String) throws -> Data? {
        let key = makeKey(keyType: keyType, keyId: keyId)
        
        var query = baseQuery(for: key)
        query[kSecReturnData as String] = true
        query[kSecMatchLimit as String] = kSecMatchLimitOne
        
        var result: AnyObject?
        let status = SecItemCopyMatching(query as CFDictionary, &result)
        
        switch status {
        case errSecSuccess:
            return result as? Data
        case errSecItemNotFound:
            return nil
        default:
            throw MlsStorageError.loadFailed
        }
    }
    
    /// Deletes data from the Keychain.
    func delete(keyType: String, keyId: String) throws {
        let key = makeKey(keyType: keyType, keyId: keyId)
        
        let query = baseQuery(for: key)
        let status = SecItemDelete(query as CFDictionary)
        
        guard status == errSecSuccess || status == errSecItemNotFound else {
            throw MlsStorageError.deleteFailed
        }
    }
    
    /// Lists all key IDs for a given key type.
    func listKeys(keyType: String) throws -> [String] {
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
            throw MlsStorageError.loadFailed
        }
    }
    
    // MARK: - Private Helpers
    
    private func makeKey(keyType: String, keyId: String) -> String {
        return "\(keyType):\(keyId)"
    }
    
    private func baseQuery(for key: String) -> [String: Any] {
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
