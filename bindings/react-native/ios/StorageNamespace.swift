//
// StorageNamespace.swift
// OfflineProtocol
//
// Stable account namespaces for built-in protocol storage.
//

import CryptoKit
import Foundation

enum StorageNamespace {
    private static let domain = "offline-protocol-storage-v1"
    private static let hex = Array("0123456789abcdef".utf8)

    /// A namespace becomes a path component and a Keychain service suffix, so
    /// an arbitrary caller-supplied string could escape its own account (`..`)
    /// or collide with another's. Mirrors `requireAccount` on Android and
    /// `require_account_storage_namespace` in Python.
    ///
    /// Deliberately matched against a literal alphabet rather than
    /// `Character.isHexDigit`: that property also accepts fullwidth
    /// compatibility forms, and `String.count` measures grapheme clusters
    /// rather than bytes — so the obvious spelling would admit namespaces the
    /// Android (`Regex("account-[0-9a-f]{64}")`) and Python
    /// (`\Aaccount-[0-9a-f]{64}\Z`) validators reject. A validator that is the
    /// security boundary on one platform has to be the same boundary on all
    /// three.
    static func requireAccount(_ value: String) throws -> String {
        let prefix = "account-"
        let digits = value.hasPrefix(prefix) ? value.dropFirst(prefix.count) : ""
        guard digits.utf8.count == 64,
              digits.allSatisfy({ lowercaseHexAlphabet.contains($0) })
        else {
            throw MlsStorageError.StoreFailed(
                message: "Invalid protocol storage account namespace"
            )
        }
        return value
    }

    private static let lowercaseHexAlphabet = Set("0123456789abcdef")

    static func account(appId: String, userId: String) -> String {
        let input = "\(domain)\0\(appId)\0\(userId)"
        let digest = SHA256.hash(data: Data(input.utf8))
        var output = Array("account-".utf8)
        output.reserveCapacity(output.count + SHA256.byteCount * 2)
        for byte in digest {
            output.append(hex[Int(byte >> 4)])
            output.append(hex[Int(byte & 0x0f)])
        }
        return String(decoding: output, as: UTF8.self)
    }
}
