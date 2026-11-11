package com.offlineprotocol

import android.util.Log
import uniffi.offline_protocol.OfflineProtocol
import uniffi.offline_protocol.ProtocolState

/**
 * Development-only ordering hints for native [ProtocolEngineHost] integrators
 * (Flutter plugin, direct Kotlin hosts). React Native disables these at the
 * host constructor because the TypeScript layer already warns (G3 PR1).
 */
object IntegratorLifecycleHints {
    private const val TAG = "OfflineProtocol"
    private val docPointer =
        "docs/integrator-map.md (React Native lifecycle ordering) and the iOS/Android integration guides"

    private val warnedKeys = mutableSetOf<String>()

    fun resetSession() {
        warnedKeys.clear()
    }

    private fun warnOnce(key: String, message: String, enabled: Boolean) {
        if (!enabled) return
        if (!warnedKeys.add(key)) return
        Log.w(TAG, message)
    }

    fun warnIfCallBeforeEngineStarted(
        context: String,
        proto: OfflineProtocol?,
        enabled: Boolean,
    ) {
        if (!enabled || proto == null) return
        if (proto.getState() == ProtocolState.RUNNING) return
        warnOnce(
            "engine:$context",
            "$context: call start() on the protocol core before this API. " +
                "Install setEventCallback before start(). See $docPointer.",
            true,
        )
    }

    fun warnIfDataMutationBeforeEngine(
        context: String,
        proto: OfflineProtocol?,
        enabled: Boolean,
    ) {
        if (!enabled || proto == null) return
        if (proto.getState() == ProtocolState.RUNNING) return
        warnOnce(
            "data:$context",
            "$context: the data store is not open yet. " +
                "Call initializeMlsWithSecureStorage and start() first, or declare " +
                "setInterest before start() for replication policy.",
            true,
        )
    }

    fun warnIfInterestAfterStart(enabled: Boolean) {
        if (!enabled) return
        warnOnce(
            "interest:after-start",
            "setInterest after start() can only widen replication; " +
                "narrowing does not delete documents already pulled during start-up. " +
                "Declare interest before start() when you need a tight filter from the first exchange.",
            true,
        )
    }

    fun warnIfStartWithoutMls(
        encryptionEnabled: Boolean,
        proto: OfflineProtocol?,
        enabled: Boolean,
    ) {
        if (!enabled || !encryptionEnabled || proto == null) return
        if (proto.isMlsInitialized()) return
        warnOnce(
            "start:mls",
            "start() with encryption enabled before initializeMlsWithSecureStorage(). " +
                "Secure send and handshake will not work until MLS storage is opened.",
            true,
        )
    }

    fun warnIfSendWithoutMls(
        context: String,
        encryptionEnabled: Boolean,
        proto: OfflineProtocol?,
        enabled: Boolean,
    ) {
        if (!enabled || !encryptionEnabled || proto == null) return
        if (proto.getState() != ProtocolState.RUNNING) return
        if (proto.isMlsInitialized()) return
        warnOnce(
            "mls:$context",
            "$context: encryption is enabled but MLS is not initialized. " +
                "Run initializeMlsWithSecureStorage() successfully before send.",
            true,
        )
    }
}
