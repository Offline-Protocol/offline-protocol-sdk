//
// IntegratorEncryptionRemedy.swift
// OfflineProtocol
//
// Shared fail-closed EncryptFailed remediation text (Foundation-only so the
// policy test harness can compile it without Generated UniFFI).
//

import Foundation

/// Callers emit this only when MLS is not initialized: the same error with MLS
/// ready has another cause, and this text would send that app the wrong way.
/// `initializer` is the call the reader runs: the reference storage helper for
/// a direct UniFFI caller, the host method for a ProtocolEngineHost caller.
enum IntegratorEncryptionRemedy {
    static let directMlsInitializer = "OfflineProtocolReferenceStorage.initializeMls"

    static func message(context: String, initializer: String = directMlsInitializer) -> String {
        "[OfflineProtocol] \(context): send failed with EncryptFailed because "
            + "encryption is fail-closed by default and MLS is not initialized. "
            + "For production, run \(initializer) before start(). "
            + "For lab-only cleartext, use "
            + "OfflineProtocolDevelopmentCleartextConfig.make and never ship "
            + "that build to users. See docs/native-development-cleartext.md."
    }
}
