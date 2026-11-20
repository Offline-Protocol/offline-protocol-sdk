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

    /**
     * Same encryption posture as `forScenario("developmentCleartext")` after `create()`:
     * encryption off, `requireEncryption` off, and sibling MLS flags off.
     */
    fun make(
        appLabel: String,
        profile: String,
    ): ProtocolConfig =
        ProtocolConfig(
            appLabel = appLabel,
            profile = profile,
            bleEnabled = true,
            wifiDirectEnabled = false,
            internetEnabled = false,
            reticulumEnabled = false,
            nostrEnabled = false,
            preferOnline = false,
            initialTtl = 8u,
            encryptionEnabled = false,
            autoKeyExchange = false,
            storePending = false,
            requireEncryption = false,
            maxPendingPerPeer = 64u,
            maxPendingGlobal = 4096u,
            pendingTtlMs = 86_400_000u,
            overflowPolicy = OverflowPolicy.DROP_OLDEST,
        )
}

/**
 * One-shot remediation when fail-closed encryption rejects a send with
 * `EncryptFailed` because MLS is not initialized. Direct UniFFI callers use this
 * in a catch, passing `protocol.isMlsInitialized()`; [ProtocolEngineHost] emits
 * the same text through [IntegratorLifecycleHints].
 */
object OfflineProtocolEncryptFailedDiagnostics {
    @Volatile
    private var warned = false

    /** Resets the once-per-process guard (tests only). */
    @JvmStatic
    internal fun resetForTests() {
        warned = false
    }

    @JvmStatic
    fun isEncryptFailed(error: Throwable): Boolean = error is ProtocolException.EncryptFailed

    @JvmStatic
    fun remedyMessage(context: String): String = IntegratorEncryptionRemedy.message(context)

    /**
     * Logs [remedyMessage] at most once per process, for an `EncryptFailed` with
     * MLS not initialized. With [mlsInitialized] true the error has another
     * cause (a session encrypt failure, a transfer that cannot leave unsealed)
     * and nothing is logged. Call it only from an app on the default encrypted
     * config: with the lab config, an `EncryptFailed` comes from a feature that
     * is sealed or not sent at all, and this text would name the wrong fix.
     */
    @JvmStatic
    fun logFirstRemedyIfNeeded(
        context: String,
        error: Throwable,
        mlsInitialized: Boolean,
        sink: (String) -> Unit,
    ) {
        if (mlsInitialized || !isEncryptFailed(error)) return
        val first = synchronized(this) {
            if (warned) false else {
                warned = true
                true
            }
        }
        if (first) sink(remedyMessage(context))
    }
}
