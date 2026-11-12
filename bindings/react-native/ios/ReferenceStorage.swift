//
// ReferenceStorage.swift
// OfflineProtocol
//
// G7: shipped reference MLS + protocol-state backends for native iOS integrators.
// Same stores the React Native bridge wires in `initializeMlsWithSecureStorage`.
//

import Foundation

/// Factory for the built-in Keychain and app-container storage providers.
public enum OfflineProtocolReferenceStorage {
    /// Stable namespace for one `(appId, profile)` pair; matches built-in providers.
    public static func accountNamespace(appId: String, profile: String) -> String {
        StorageNamespace.account(appId: appId, profile: profile)
    }

    public static func makeSecureStorage(
        appId: String,
        profile: String,
        service: String? = nil,
        accessGroup: String? = nil
    ) throws -> MlsStorageProvider {
        let namespace = StorageNamespace.account(appId: appId, profile: profile)
        return try MlsSecureStorage(
            accountNamespace: namespace,
            service: service,
            accessGroup: accessGroup
        )
    }

    public static func makeProtocolStateStorage(
        appId: String,
        profile: String
    ) throws -> ProtocolStateStorageProvider {
        let namespace = StorageNamespace.account(appId: appId, profile: profile)
        return try AppContainerProtocolStateStorage(accountNamespace: namespace)
    }

    /// Opens both built-in stores for `appId` / `profile` and calls `initializeMls`.
    public static func initializeMls(
        on mesh: OfflineProtocol,
        appId: String,
        profile: String
    ) throws {
        let secure = try makeSecureStorage(appId: appId, profile: profile)
        let state = try makeProtocolStateStorage(appId: appId, profile: profile)
        try mesh.initializeMls(secureStorage: secure, protocolStateStorage: state)
    }
}
