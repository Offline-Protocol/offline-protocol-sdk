package com.offlineprotocol

import org.json.JSONArray
import org.json.JSONObject
import uniffi.offline_protocol.ProtocolStateStorageProvider
import uniffi.offline_protocol.runStorageConformance

/** One check the backend failed, with enough detail to fix the adapter. */
data class StorageConformanceFailure(
    val check: String,
    val detail: String,
)

/**
 * What the conformance suite found for one backend.
 *
 * The keys [StorageConformance] reads are the ones the Rust `ConformanceReport`
 * serialises; a guard in the FFI crate pins them, because a renamed key throws
 * here rather than failing any build.
 */
data class StorageConformanceReport(
    /** The checks that passed, in run order. */
    val passed: List<String>,
    /** The checks that failed. */
    val failures: List<StorageConformanceFailure>,
) {
    /**
     * Whether the backend is supported: nothing failed and the suite ran. An
     * empty report is not green, since it proves nothing was checked.
     */
    val isGreen: Boolean
        get() = failures.isEmpty() && passed.isNotEmpty()
}

/**
 * Runs the conformance suite (C11) against a protocol-state backend.
 *
 * Green is the definition of a supported backend, but it is not a persistence
 * test: every check runs against one open instance. A persistent backend proves
 * reopen with a test of its own.
 */
object StorageConformance {
    /**
     * Runs the suite against [provider] and decodes its report.
     *
     * No engine is needed, but the Rust library must be loaded. The suite
     * writes only under its own probe key types and deletes what it wrote, but
     * it writes records of several megabytes and blocks until it ends: run it
     * off the main thread, and on a scratch instance of a new backend until it
     * is green.
     */
    @JvmStatic
    fun run(provider: ProtocolStateStorageProvider): StorageConformanceReport =
        decode(runStorageConformance(provider))

    /** Decodes the JSON report the suite renders. */
    internal fun decode(json: String): StorageConformanceReport {
        val root = JSONObject(json)
        return StorageConformanceReport(
            passed = root.getJSONArray("passed").strings(),
            failures = root.getJSONArray("failures").failures(),
        )
    }

    private fun JSONArray.strings(): List<String> = List(length()) { getString(it) }

    private fun JSONArray.failures(): List<StorageConformanceFailure> = List(length()) {
        val item = getJSONObject(it)
        StorageConformanceFailure(check = item.getString("check"), detail = item.getString("detail"))
    }
}
