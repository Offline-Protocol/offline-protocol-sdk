//
// StorageConformance.swift
// OfflineProtocol
//
// G7 PR2: typed access to `runStorageConformance` for native iOS integrators (C11).
//

import Foundation

public struct StorageConformanceFailure: Codable, Sendable, Equatable {
    public let check: String
    public let detail: String
}

public struct StorageConformanceReport: Codable, Sendable, Equatable {
    public let passed: [String]
    public let failures: [StorageConformanceFailure]

    /// Green is the definition of a supported protocol-state backend.
    public var isGreen: Bool {
        failures.isEmpty && !passed.isEmpty
    }
}

public enum StorageConformance {
    /// Runs the engine suite against `provider` and decodes the JSON report.
    public static func run(on provider: ProtocolStateStorageProvider) throws -> StorageConformanceReport {
        let json = runStorageConformance(storage: provider)
        guard let data = json.data(using: .utf8) else {
            throw MlsStorageError.StoreFailed(message: "conformance report is not UTF-8")
        }
        let decoder = JSONDecoder()
        return try decoder.decode(StorageConformanceReport.self, from: data)
    }

    /// Convenience: verify the shipped reference app-container store for one account.
    public static func verifyReferenceProtocolStateStorage(
        appId: String,
        profile: String
    ) throws -> StorageConformanceReport {
        let storage = try OfflineProtocolReferenceStorage.makeProtocolStateStorage(
            appId: appId,
            profile: profile
        )
        return try run(on: storage)
    }
}
