package com.offlineprotocol

/**
 * Shared fail-closed EncryptFailed remediation text for native integrator hints.
 * Callers emit it only when MLS is not initialized: the same error with MLS
 * ready has another cause, and this text would send that app the wrong way.
 * [initializer] is the call the reader runs: the reference storage helper for
 * a direct UniFFI caller, the host method for a [ProtocolEngineHost] caller.
 */
internal object IntegratorEncryptionRemedy {
    const val DIRECT_MLS_INITIALIZER: String = "OfflineProtocolReferenceStorage.initializeMls"

    fun message(context: String, initializer: String = DIRECT_MLS_INITIALIZER): String =
        "[OfflineProtocol] $context: send failed with EncryptFailed because " +
            "encryption is fail-closed by default and MLS is not initialized. " +
            "For production, run $initializer before start(). " +
            "For lab-only cleartext, use " +
            "OfflineProtocolDevelopmentCleartextConfig.make and never ship " +
            "that build to users. See docs/native-development-cleartext.md."
}
