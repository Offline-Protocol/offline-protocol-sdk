package com.offlineprotocol

import android.content.Context
import org.json.JSONArray
import org.json.JSONObject
import uniffi.offline_protocol.ProtocolStateStorageProvider
import uniffi.offline_protocol.runStorageConformance

/**
 * G7 PR2: typed access to [runStorageConformance] for native Android integrators (C11).
 */
data class StorageConformanceFailure(
    val check: String,
    val detail: String,
)

data class StorageConformanceReport(
    val passed: List<String>,
    val failures: List<StorageConformanceFailure>,
) {
    /** Green is the definition of a supported protocol-state backend. */
    val isGreen: Boolean
        get() = failures.isEmpty() && passed.isNotEmpty()
}

object StorageConformance {
    /** Runs the engine suite against [provider] and decodes the JSON report. */
    fun run(provider: ProtocolStateStorageProvider): StorageConformanceReport {
        val root = JSONObject(runStorageConformance(provider))
        val passed = root.getJSONArray("passed").toStringList()
        val failures = root.getJSONArray("failures").toFailureList()
        return StorageConformanceReport(passed, failures)
    }

    /**
     * Convenience: verify the shipped reference app-container store for one account.
     *
     * Requires the Rust library to be loaded (device, emulator, or a host test that
     * loads [uniffi.offline_protocol].
     */
    fun verifyReferenceProtocolStateStorage(
        context: Context,
        appId: String,
        profile: String,
    ): StorageConformanceReport {
        val storage = OfflineProtocolReferenceStorage.makeProtocolStateStorage(context, appId, profile)
        return run(storage)
    }

    private fun JSONArray.toStringList(): List<String> = buildList(length()) { i ->
        add(getString(i))
    }

    private fun JSONArray.toFailureList(): List<StorageConformanceFailure> =
        buildList(length()) { i ->
            val item = getJSONObject(i)
            add(
                StorageConformanceFailure(
                    check = item.getString("check"),
                    detail = item.getString("detail"),
                ),
            )
        }
}
