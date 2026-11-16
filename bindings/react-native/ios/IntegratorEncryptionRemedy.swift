//
// IntegratorEncryptionRemedy.swift
// OfflineProtocol
//
// Fail-closed EncryptFailed remediation text for a direct UniFFI caller,
// emitted by OfflineProtocolEncryptFailedDiagnostics.
//

import Foundation

/// Callers emit this only when MLS is not initialized: the same error with MLS
/// ready has another cause, and this text would send that app the wrong way.
enum IntegratorEncryptionRemedy {
    static func message(context: String) -> String {
        "[OfflineProtocol] \(context): send failed with EncryptFailed because "
            + "encryption is fail-closed by default and MLS is not initialized. "
            + "For production, run OfflineProtocolReferenceStorage.initializeMls before start(). "
            + "For lab-only cleartext, use "
            + "OfflineProtocolDevelopmentCleartextConfig.make and never ship "
            + "that build to users. See docs/native-development-cleartext.md."
    }
}
