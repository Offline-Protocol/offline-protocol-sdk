//
// StorageConformance.swift
// OfflineProtocol
//
// The protocol-state storage conformance suite, with a typed report.
//

import Foundation

/// One check the backend failed, with enough detail to fix the adapter.
public struct StorageConformanceFailure: Codable, Sendable, Equatable {
    public let check: String
    public let detail: String
}

/// What the conformance suite found for one backend.
///
/// The field names are the keys the Rust `ConformanceReport` serialises; a
/// guard in the FFI crate pins them, because a renamed key decodes as an error
/// here rather than failing any build.
public struct StorageConformanceReport: Codable, Sendable, Equatable {
    /// The checks that passed, in run order.
    public let passed: [String]
    /// The checks that failed.
    public let failures: [StorageConformanceFailure]

    /// Whether the backend is supported: nothing failed and the suite ran.
    /// An empty report is not green, since it proves nothing was checked.
    public var isGreen: Bool {
        failures.isEmpty && !passed.isEmpty
    }
}

/// Runs the conformance suite (C11) against a protocol-state backend.
///
/// Green is the definition of a supported backend, but it is not a persistence
/// test: every check runs against one open instance. A persistent backend
/// proves reopen with a test of its own.
public enum StorageConformance {

    /// Runs the suite against `provider` and decodes its report.
    ///
    /// No engine is needed. The suite writes only under its own probe key
    /// types and deletes what it wrote, but it writes records of several
    /// megabytes and blocks until it ends: run it off the main thread, and on
    /// a scratch instance of a new backend until it is green.
    public static func run(on provider: ProtocolStateStorageProvider) throws -> StorageConformanceReport {
        try decode(runStorageConformance(storage: provider))
    }

    /// Decodes the JSON report the suite renders.
    static func decode(_ json: String) throws -> StorageConformanceReport {
        try JSONDecoder().decode(StorageConformanceReport.self, from: Data(json.utf8))
    }
}
