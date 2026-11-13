//
// IntegratorEncryptionRemedy.swift
// OfflineProtocol
//
// Shared fail-closed EncryptFailed remediation text (Foundation-only so the
// policy test harness can compile it without Generated UniFFI).
//

import Foundation

enum IntegratorEncryptionRemedy {
    static func message(context: String) -> String {
        "[OfflineProtocol] \(context): send failed with EncryptFailed because "
            + "encryption is fail-closed by default. For production, run "
            + "OfflineProtocolReferenceStorage.initializeMls before start(). "
            + "For lab-only cleartext, use "
            + "OfflineProtocolDevelopmentCleartextConfig.make and never ship "
            + "that build to users. See docs/native-development-cleartext.md."
    }
}
