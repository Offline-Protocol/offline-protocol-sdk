//
// ProtocolStateStorage.swift
// OfflineProtocol
//
// App-container storage for restartable delivery and protocol state.
//

import Foundation

#if SWIFT_PACKAGE
enum MlsStorageError: Error {
    case StoreFailed(message: String)
    case LoadFailed(message: String)
    case DeleteFailed(message: String)
}

protocol ProtocolStateStorageProvider {
    func store(keyType: String, keyId: String, data: [UInt8]) throws
    func load(keyType: String, keyId: String) throws -> [UInt8]?
    func delete(keyType: String, keyId: String) throws
    func listKeys(keyType: String) throws -> [String]
}
#endif

/// File-backed protocol state whose lifecycle is tied to the app container.
///
/// Each entry is written atomically. The directory is excluded from device
/// backup so uninstall/reinstall and device restore cannot resurrect an old
/// outbox or retry lifecycle.
final class AppContainerProtocolStateStorage: ProtocolStateStorageProvider {
    private static let schemaDirectory = "protocol-state-v1"

    private let root: URL
    private let fileManager: FileManager
    private let lock = NSLock()

    convenience init(
        accountNamespace: String,
        fileManager: FileManager = .default
    ) throws {
        guard let applicationSupport = fileManager.urls(
            for: .applicationSupportDirectory,
            in: .userDomainMask
        ).first else {
            throw MlsStorageError.StoreFailed(
                message: "Application Support directory is unavailable"
            )
        }

        let bundleComponent = Bundle.main.bundleIdentifier ?? "com.offlineprotocol"
        let root = applicationSupport
            .appendingPathComponent(bundleComponent, isDirectory: true)
            .appendingPathComponent(Self.schemaDirectory, isDirectory: true)
            .appendingPathComponent(accountNamespace, isDirectory: true)
        try self.init(root: root, fileManager: fileManager)
    }

    init(
        root: URL,
        fileManager: FileManager = .default
    ) throws {
        self.root = root
        self.fileManager = fileManager

        do {
            try fileManager.createDirectory(
                at: root,
                withIntermediateDirectories: true
            )
            var resourceValues = URLResourceValues()
            resourceValues.isExcludedFromBackup = true
            var mutableRoot = root
            try mutableRoot.setResourceValues(resourceValues)
        } catch {
            throw MlsStorageError.StoreFailed(
                message: "Failed to initialize protocol-state directory: \(error)"
            )
        }
    }

    func store(keyType: String, keyId: String, data: [UInt8]) throws {
        lock.lock()
        defer { lock.unlock() }

        let directory = typeDirectory(keyType)
        do {
            try fileManager.createDirectory(
                at: directory,
                withIntermediateDirectories: true
            )
            try Data(data).write(to: entryURL(keyType: keyType, keyId: keyId), options: .atomic)
        } catch {
            throw MlsStorageError.StoreFailed(
                message: "Failed to persist protocol state: \(error)"
            )
        }
    }

    func load(keyType: String, keyId: String) throws -> [UInt8]? {
        lock.lock()
        defer { lock.unlock() }

        let url = entryURL(keyType: keyType, keyId: keyId)
        guard fileManager.fileExists(atPath: url.path) else {
            return nil
        }
        do {
            return [UInt8](try Data(contentsOf: url))
        } catch {
            throw MlsStorageError.LoadFailed(
                message: "Failed to load protocol state: \(error)"
            )
        }
    }

    func delete(keyType: String, keyId: String) throws {
        lock.lock()
        defer { lock.unlock() }

        let url = entryURL(keyType: keyType, keyId: keyId)
        guard fileManager.fileExists(atPath: url.path) else {
            return
        }
        do {
            try fileManager.removeItem(at: url)
        } catch {
            throw MlsStorageError.DeleteFailed(
                message: "Failed to delete protocol state: \(error)"
            )
        }
    }

    func listKeys(keyType: String) throws -> [String] {
        lock.lock()
        defer { lock.unlock() }

        let directory = typeDirectory(keyType)
        guard fileManager.fileExists(atPath: directory.path) else {
            return []
        }
        do {
            return try fileManager
                .contentsOfDirectory(
                    at: directory,
                    includingPropertiesForKeys: nil,
                    options: [.skipsHiddenFiles]
                )
                .compactMap { decodeComponent($0.lastPathComponent) }
                .sorted()
        } catch {
            throw MlsStorageError.LoadFailed(
                message: "Failed to list protocol-state keys: \(error)"
            )
        }
    }

    private func typeDirectory(_ keyType: String) -> URL {
        root.appendingPathComponent(encodeComponent(keyType), isDirectory: true)
    }

    private func entryURL(keyType: String, keyId: String) -> URL {
        typeDirectory(keyType).appendingPathComponent(encodeComponent(keyId), isDirectory: false)
    }

    private func encodeComponent(_ value: String) -> String {
        let encoded = Data(value.utf8).base64EncodedString()
            .replacingOccurrences(of: "+", with: "-")
            .replacingOccurrences(of: "/", with: "_")
            .replacingOccurrences(of: "=", with: "")
        return "k_\(encoded)"
    }

    private func decodeComponent(_ value: String) -> String? {
        guard value.hasPrefix("k_") else {
            return nil
        }
        var encoded = String(value.dropFirst(2))
            .replacingOccurrences(of: "-", with: "+")
            .replacingOccurrences(of: "_", with: "/")
        let remainder = encoded.count % 4
        if remainder != 0 {
            encoded.append(String(repeating: "=", count: 4 - remainder))
        }
        guard let data = Data(base64Encoded: encoded) else {
            return nil
        }
        return String(data: data, encoding: .utf8)
    }
}
