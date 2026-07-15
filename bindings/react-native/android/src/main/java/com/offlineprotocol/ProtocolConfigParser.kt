package com.offlineprotocol

import org.json.JSONObject
import uniffi.offline_protocol.OverflowPolicy
import uniffi.offline_protocol.ProtocolConfig

/**
 * Parses the JSON config string sent by the JS wrapper (or a direct native
 * caller) into the UniFFI [ProtocolConfig].
 *
 * Extracted from [OfflineProtocolModule] so the dual-shape/dual-case field
 * reads are JVM unit testable — a silent parse regression here reverts a
 * field to its default with no error anywhere. The iOS bridge keeps the same
 * logic inline in OfflineProtocolModule.swift `parseConfig`; keep the read
 * order and precedence in sync.
 */
internal object ProtocolConfigParser {

    data class ParsedConfig(
        val coreConfig: ProtocolConfig,
        val rawJson: JSONObject
    )

    private const val DEFAULT_INITIAL_TTL = 8

    fun parse(configJson: String): ParsedConfig {
        val json = JSONObject(configJson)

        // Parse encryption config with defaults (enabled by default)
        val encryptionJson = json.optJSONObject("encryption")
        val encryptionEnabled = encryptionJson?.optBoolean("enabled", true) ?: true
        val autoKeyExchange = encryptionJson?.let {
            it.optBooleanCompat("autoKeyExchange", "auto_key_exchange") ?: true
        } ?: true
        val storePending = encryptionJson?.let {
            it.optBooleanCompat("storePending", "store_pending") ?: true
        } ?: true
        // Fail-closed default (SEC-M3): plaintext operation is an explicit opt-out.
        val requireEncryption = encryptionJson?.let {
            it.optBooleanCompat("requireEncryption", "require_encryption") ?: true
        } ?: true
        // Wire-format kill switches (default on), accepted in camelCase or
        // snake_case. compactEnvelopeEnabled: nested home under `encryption`
        // first, then top level (the JS wrapper sends the flat shape; direct
        // native callers may follow the nested config). binaryWireEnabled:
        // top level only — that IS its home in both the JS config and the
        // flat UniFFI dictionary.
        val binaryWireEnabled =
            json.optBooleanCompat("binaryWireEnabled", "binary_wire_enabled") ?: true
        val compactEnvelopeEnabled = encryptionJson?.optBooleanCompat(
            "compactEnvelopeEnabled",
            "compact_envelope_enabled"
        ) ?: json.optBooleanCompat("compactEnvelopeEnabled", "compact_envelope_enabled") ?: true
        val pendingQueueJson = encryptionJson?.optJSONObject("pendingQueue")
            ?: encryptionJson?.optJSONObject("pending_queue")
        val maxPendingPerPeer = pendingQueueJson?.optLongCompat(
            "maxPendingPerPeer",
            "max_pending_per_peer"
        ) ?: json.optLongCompat("maxPendingPerPeer", "max_pending_per_peer") ?: 64L
        val maxPendingGlobal = pendingQueueJson?.optLongCompat(
            "maxPendingGlobal",
            "max_pending_global"
        ) ?: json.optLongCompat("maxPendingGlobal", "max_pending_global") ?: 4096L
        val pendingTtlMs = pendingQueueJson?.optLongCompat(
            "pendingTtlMs",
            "pending_ttl_ms"
        ) ?: json.optLongCompat("pendingTtlMs", "pending_ttl_ms") ?: 120_000L
        val overflowPolicyRaw = pendingQueueJson?.optStringCompat(
            "overflowPolicy",
            "overflow_policy"
        ) ?: json.optStringCompat("overflowPolicy", "overflow_policy")
        val overflowPolicy = when (overflowPolicyRaw?.lowercase()) {
            "drop_newest" -> OverflowPolicy.DROP_NEWEST
            "dropoldest", "drop_oldest", null, "" -> OverflowPolicy.DROP_OLDEST
            else -> OverflowPolicy.DROP_OLDEST
        }

        val config = ProtocolConfig(
            appId = json.safeOptString("appId", json.safeOptString("app_id")),
            userId = json.safeOptString("userId", json.safeOptString("user_id")),
            bleEnabled = json.optBoolean("bleEnabled", json.optBoolean("ble_enabled", true)),
            wifiDirectEnabled = json.optBoolean("wifiDirectEnabled", json.optBoolean("wifi_direct_enabled", true)),
            internetEnabled = json.optBoolean("internetEnabled", json.optBoolean("internet_enabled", true)),
            reticulumEnabled = json.optBoolean("reticulumEnabled", json.optBoolean("reticulum_enabled", false)),
            nostrEnabled = json.optBoolean("nostrEnabled", json.optBoolean("nostr_enabled", false)),
            preferOnline = json.optBoolean("preferOnline", json.optBoolean("prefer_online", false)),
            initialTtl = json.optInt("initialTtl", json.optInt("initial_ttl", DEFAULT_INITIAL_TTL)).toUByte(),
            encryptionEnabled = encryptionEnabled,
            autoKeyExchange = autoKeyExchange,
            storePending = storePending,
            requireEncryption = requireEncryption,
            maxPendingPerPeer = maxPendingPerPeer.toULong(),
            maxPendingGlobal = maxPendingGlobal.toULong(),
            pendingTtlMs = pendingTtlMs.toULong(),
            overflowPolicy = overflowPolicy,
            binaryWireEnabled = binaryWireEnabled,
            compactEnvelopeEnabled = compactEnvelopeEnabled
        )

        return ParsedConfig(config, json)
    }
}
