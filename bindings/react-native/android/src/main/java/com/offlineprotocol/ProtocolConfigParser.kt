package com.offlineprotocol

import org.json.JSONObject
import uniffi.offline_protocol.MeshRelayConfig
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
        // Nostr sealing (kill switch, default on). Nested home under
        // `transports.nostr.sealingEnabled` — where the rest of the Nostr
        // transport settings live — then the flat key, both cases. Same
        // nested-wins-over-flat rule as `encryption` and `group`; mirrors
        // OfflineProtocolModule.swift, keep in sync.
        val nostrJson = json.optJSONObject("transports")?.optJSONObject("nostr")
        val nostrSealingEnabled = nostrJson?.optBooleanCompat(
            "sealingEnabled",
            "sealing_enabled"
        ) ?: json.optBooleanCompat("nostrSealingEnabled", "nostr_sealing_enabled") ?: true
        // Nostr cold contact (key-package publication + peer resolution, kill
        // switch, default on). Same nested-then-flat shape as sealing above.
        val nostrColdContactEnabled = nostrJson?.optBooleanCompat(
            "coldContactEnabled",
            "cold_contact_enabled"
        ) ?: json.optBooleanCompat("nostrColdContactEnabled", "nostr_cold_contact_enabled") ?: true
        // Nostr username discovery (claim publication + username resolution).
        // Same nested-then-flat shape, but the default is FALSE: publishing
        // binds a human-readable name to an address in a public place, which
        // an app must opt into rather than inherit.
        val nostrUsernameDiscoveryEnabled = nostrJson?.optBooleanCompat(
            "usernameDiscoveryEnabled",
            "username_discovery_enabled"
        ) ?: json.optBooleanCompat(
            "nostrUsernameDiscoveryEnabled",
            "nostr_username_discovery_enabled"
        ) ?: false
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
        ) ?: json.optLongCompat("pendingTtlMs", "pending_ttl_ms") ?: 1_800_000L
        val overflowPolicyRaw = pendingQueueJson?.optStringCompat(
            "overflowPolicy",
            "overflow_policy"
        ) ?: json.optStringCompat("overflowPolicy", "overflow_policy")
        val overflowPolicy = when (overflowPolicyRaw?.lowercase()) {
            "drop_newest" -> OverflowPolicy.DROP_NEWEST
            "dropoldest", "drop_oldest", null, "" -> OverflowPolicy.DROP_OLDEST
            else -> OverflowPolicy.DROP_OLDEST
        }

        // Group section (nested home under `group`, then top level, both
        // cases — same shape rules as `encryption`). These were UniFFI-only
        // until the broadcast default flipped on: with no JS-reachable
        // opt-out, an RN app could not force per-member fan-out.
        val groupJson = json.optJSONObject("group")
        val maxGroupMembers = groupJson?.optLongCompat("maxGroupMembers", "max_group_members")
            ?: json.optLongCompat("maxGroupMembers", "max_group_members")
            ?: 256L
        val groupRelayEnabled = groupJson?.optBooleanCompat("relayEnabled", "relay_enabled")
            ?: json.optBooleanCompat("groupRelayEnabled", "group_relay_enabled")
            ?: true
        val groupRelayBroadcastEnabled = groupJson?.optBooleanCompat(
            "relayBroadcastEnabled",
            "relay_broadcast_enabled"
        ) ?: json.optBooleanCompat("groupRelayBroadcastEnabled", "group_relay_broadcast_enabled")
            ?: true
        // Default false — see GroupConfig::enforce_admin_commits. Enabling it
        // makes this device refuse membership commits it cannot authorize,
        // which forks it from every member that accepted them.
        val groupEnforceAdminCommits = groupJson?.optBooleanCompat(
            "enforceAdminCommits",
            "enforce_admin_commits"
        ) ?: json.optBooleanCompat("groupEnforceAdminCommits", "group_enforce_admin_commits")
            ?: false

        // Mesh forwarding section (nested home under `meshRelay`, both cases).
        // Absent stays absent all the way to the core: every field is nullable
        // and null means "keep the Rust default", so this parser never states
        // a default of its own and cannot silently reset one. Only the whole
        // section being present with at least one field produces an object.
        //
        // Widths are coerced before the unsigned conversion, never bare
        // `toULong()`: the value is app-supplied JS and a negative would wrap
        // to something enormous — a fan-out of -1 becoming 18 quintillion is
        // not a config error the core can see. Clamped low it reaches the
        // core's own validation, which rejects a zero fan-out.
        val meshRelayJson = json.optJSONObject("meshRelay") ?: json.optJSONObject("mesh_relay")
        val meshRelay = meshRelayJson?.let { section ->
            fun uLong(vararg keys: String): ULong? =
                section.optLongCompat(*keys)?.coerceAtLeast(0L)?.toULong()
            fun uByte(vararg keys: String): UByte? =
                section.optIntCompat(*keys)?.coerceIn(0, UByte.MAX_VALUE.toInt())?.toUByte()
            fun uInt(vararg keys: String): UInt? =
                section.optLongCompat(*keys)?.coerceIn(0L, UInt.MAX_VALUE.toLong())?.toUInt()
            fun float(vararg keys: String): Float? =
                section.optDoubleCompat(*keys)?.toFloat()

            MeshRelayConfig(
                maxTtl = uByte("maxTtl", "max_ttl"),
                denseMaxTtl = uByte("denseMaxTtl", "dense_max_ttl"),
                denseDegree = uLong("denseDegree", "dense_degree"),
                fanout = uLong("fanout"),
                jitterMinMs = uLong("jitterMinMs", "jitter_min_ms"),
                jitterMaxMs = uLong("jitterMaxMs", "jitter_max_ms"),
                ratePerSec = float("ratePerSec", "rate_per_sec"),
                burst = float("burst"),
                peerRatePerSec = float("peerRatePerSec", "peer_rate_per_sec"),
                peerBurst = float("peerBurst", "peer_burst"),
                queueCapacity = uLong("queueCapacity", "queue_capacity"),
                biasMinScale = float("biasMinScale", "bias_min_scale"),
                biasMaxHandicapMs = uLong("biasMaxHandicapMs", "bias_max_handicap_ms"),
                activityWindowMs = uLong("activityWindowMs", "activity_window_ms"),
                activityMinForwards = uLong("activityMinForwards", "activity_min_forwards"),
                activityIdleWindows = uInt("activityIdleWindows", "activity_idle_windows")
            )
        }

        // Data layer section (nested home under `data`, both cases). Same
        // rule as meshRelay: absent stays absent, so the Rust default is the
        // only default. The flag is read out of the section rather than as a
        // top-level `dataEnabled`, because the section is where the layer's
        // other knobs will land and one home is easier to keep correct than
        // two spellings.
        val dataJson = json.optJSONObject("data")
        val dataEnabled = dataJson?.optBooleanCompat("enabled")

        // Security section (nested home under `security`, both cases), with a
        // top-level spelling accepted too. Same absent-stays-absent rule as
        // the two sections above, and the same reason: the Rust default is on,
        // and restating it here as a literal is how a default that later moves
        // stops moving for every app that never set it.
        //
        // Unlike `data`, a flat spelling is honoured. This flag is the lever an
        // app reaches for when its fleet's clocks are wrong and its control
        // plane has gone quiet, and a lever that silently does nothing because
        // it was written one level too high is worse than no lever.
        val securityJson = json.optJSONObject("security")
        val controlFreshnessEnforced =
            securityJson?.optBooleanCompat("controlFreshnessEnforced", "control_freshness_enforced")
                ?: json.optBooleanCompat("controlFreshnessEnforced", "control_freshness_enforced")

        val config = ProtocolConfig(
            appId = json.safeOptString("appId", json.safeOptString("app_id")),
            profile = json.safeOptString("profile"),
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
            // Coerced, not bare `toUInt()` — the value is app-supplied JS, and
            // a negative would wrap to ~4 billion (silently unlimited). Clamped
            // low it reaches the core's own validation, which rejects 0.
            // Mirrors the iOS parser's `UInt32(clamping:)`.
            maxGroupMembers = maxGroupMembers.coerceIn(0L, UInt.MAX_VALUE.toLong()).toUInt(),
            groupRelayEnabled = groupRelayEnabled,
            groupRelayBroadcastEnabled = groupRelayBroadcastEnabled,
            groupEnforceAdminCommits = groupEnforceAdminCommits,
            binaryWireEnabled = binaryWireEnabled,
            nostrSealingEnabled = nostrSealingEnabled,
            nostrColdContactEnabled = nostrColdContactEnabled,
            nostrUsernameDiscoveryEnabled = nostrUsernameDiscoveryEnabled,
            compactEnvelopeEnabled = compactEnvelopeEnabled,
            richPayloadEnabled = richPayloadEnabled,
            cryptoRecoveryEnabled = cryptoRecoveryEnabled,
            meshRelay = meshRelay
        )

        // Assigned only when the app actually sent it. Writing
        // `dataEnabled = dataEnabled ?: false` above would restate the
        // default as a literal here, and the literal is what survives a
        // change to the real default: the release that flips it on would
        // keep forcing `false` for every app that omits the section, exactly
        // as `config.relay` sat at its defaults for several releases.
        if (dataEnabled != null) {
            config.dataEnabled = dataEnabled
        }
        if (controlFreshnessEnforced != null) {
            config.controlFreshnessEnforced = controlFreshnessEnforced
        }

        return ParsedConfig(config, json)
    }
}
