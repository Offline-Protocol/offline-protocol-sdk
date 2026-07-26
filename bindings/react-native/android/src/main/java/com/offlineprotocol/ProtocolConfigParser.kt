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
 * field to its default with no error anywhere. The iOS bridge mirrors the
 * encryption-section reads in EncryptionConfigReader.swift (covered by the
 * SwiftPM suite) and keeps the rest inline in OfflineProtocolModule.swift
 * `parseConfig`; keep the read order and precedence in sync.
 */
internal object ProtocolConfigParser {

    data class ParsedConfig(
        val coreConfig: ProtocolConfig,
        val rawJson: JSONObject
    )

    private const val DEFAULT_INITIAL_TTL = 8

    fun parse(configJson: String): ParsedConfig {
        val json = JSONObject(configJson)

        // Encryption flags (default on): nested home under `encryption`
        // first, then top level, in camelCase or snake_case. The JS wrapper
        // sends both shapes with identical values; direct native callers may
        // send either. Accepting both is what keeps a sender-side shape
        // change from silently reverting a flag to its default — these four
        // were nested-only while the wrapper sent flat, so every app-set
        // value was dropped.
        val encryptionJson = json.optJSONObject("encryption")
        val encryptionEnabled = encryptionJson?.optBooleanCompat("enabled")
            ?: json.optBooleanCompat("encryptionEnabled", "encryption_enabled")
            ?: true
        val autoKeyExchange =
            encryptionJson?.optBooleanCompat("autoKeyExchange", "auto_key_exchange")
                ?: json.optBooleanCompat("autoKeyExchange", "auto_key_exchange")
                ?: true
        val storePending = encryptionJson?.optBooleanCompat("storePending", "store_pending")
            ?: json.optBooleanCompat("storePending", "store_pending")
            ?: true
        // Fail-closed default (SEC-M3): plaintext operation is an explicit opt-out.
        val requireEncryption =
            encryptionJson?.optBooleanCompat("requireEncryption", "require_encryption")
                ?: json.optBooleanCompat("requireEncryption", "require_encryption")
                ?: true
        // Wire-format kill switches (default on), same shape rules as above
        // for compactEnvelopeEnabled. binaryWireEnabled: top level only —
        // that IS its home in both the JS config and the flat UniFFI
        // dictionary.
        val binaryWireEnabled =
            json.optBooleanCompat("binaryWireEnabled", "binary_wire_enabled") ?: true
        val compactEnvelopeEnabled = encryptionJson?.optBooleanCompat(
            "compactEnvelopeEnabled",
            "compact_envelope_enabled"
        ) ?: json.optBooleanCompat("compactEnvelopeEnabled", "compact_envelope_enabled") ?: true
        val richPayloadEnabled = encryptionJson?.optBooleanCompat(
            "richPayloadEnabled",
            "rich_payload_enabled"
        ) ?: json.optBooleanCompat("richPayloadEnabled", "rich_payload_enabled") ?: true
        val cryptoRecoveryEnabled = encryptionJson?.optBooleanCompat(
            "cryptoRecoveryEnabled",
            "crypto_recovery_enabled"
        ) ?: json.optBooleanCompat("cryptoRecoveryEnabled", "crypto_recovery_enabled") ?: true
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
            compactEnvelopeEnabled = compactEnvelopeEnabled,
            richPayloadEnabled = richPayloadEnabled,
            cryptoRecoveryEnabled = cryptoRecoveryEnabled
        )

        return ParsedConfig(config, json)
    }
}
