package com.offlineprotocol

/** Shared fail-closed EncryptFailed remediation text for native integrator hints. */
internal object IntegratorEncryptionRemedy {
    fun message(context: String): String =
        "[OfflineProtocol] $context: send failed with EncryptFailed because " +
            "encryption is fail-closed by default. For production, run " +
            "OfflineProtocolReferenceStorage.initializeMls before start(). " +
            "For lab-only cleartext, use " +
            "OfflineProtocolDevelopmentCleartextConfig.make and never ship " +
            "that build to users. See docs/native-development-cleartext.md."
}
