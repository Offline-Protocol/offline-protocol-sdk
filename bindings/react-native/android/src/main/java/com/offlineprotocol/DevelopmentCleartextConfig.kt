package com.offlineprotocol

import uniffi.offline_protocol.OverflowPolicy
import uniffi.offline_protocol.ProtocolConfig
import uniffi.offline_protocol.ProtocolException

/**
 * Native counterpart to the React Native `developmentCleartext` scenario:
 * BLE on, every other transport off, encryption fully disabled so sends work
 * without MLS. **Never ship this to end users.**
 *
 * See `docs/native-development-cleartext.md` and
 * `docs/react-native-config-presets.md`.
 */
object OfflineProtocolDevelopmentCleartextConfig {
    const val LAB_ONLY_NOTICE: String =
        "OfflineProtocolDevelopmentCleartextConfig is for lab / simulator smoke " +
            "tests only. Production builds must use default encryption and " +
            "OfflineProtocolReferenceStorage.initializeMls before start()."

    /** Same encryption posture as `forScenario("developmentCleartext")` after `create()`. */
    fun make(
        appId: String,
        profile: String,
    ): ProtocolConfig =
        ProtocolConfig(
            appId = appId,
            profile = profile,
            bleEnabled = true,
            wifiDirectEnabled = false,
            internetEnabled = false,
            reticulumEnabled = false,
            nostrEnabled = false,
            preferOnline = false,
            initialTtl = 8u,
            encryptionEnabled = false,
            autoKeyExchange = true,
            storePending = true,
            requireEncryption = false,
            maxPendingPerPeer = 100u,
            maxPendingGlobal = 1000u,
            pendingTtlMs = 86_400_000u,
            overflowPolicy = OverflowPolicy.DROP_OLDEST,
        )
}

/**
 * One-shot remediation when fail-closed encryption rejects a send with
 * `EncryptFailed`. Direct UniFFI callers use this in a catch; [ProtocolEngineHost]
 * emits the same text through [IntegratorLifecycleHints].
 */
object OfflineProtocolEncryptFailedDiagnostics {
    @Volatile
    private var warned = false

    /** Resets the once-per-process guard (tests only). */
    @JvmStatic
    fun resetForTests() {
        warned = false
    }

    @JvmStatic
    fun isEncryptFailed(error: Throwable): Boolean = error is ProtocolException.EncryptFailed

    @JvmStatic
    fun remedyMessage(context: String): String = IntegratorEncryptionRemedy.message(context)

    @JvmStatic
    fun logFirstRemedyIfNeeded(
        context: String,
        error: Throwable,
        sink: (String) -> Unit,
    ) {
        if (!isEncryptFailed(error)) return
        val first = synchronized(this) {
            if (warned) false else {
                warned = true
                true
            }
        }
        if (first) sink(remedyMessage(context))
    }
}
