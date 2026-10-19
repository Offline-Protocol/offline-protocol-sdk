package com.offlineprotocol

import kotlin.math.max
import kotlin.math.min
import org.json.JSONObject
import uniffi.offline_protocol.OfflineProtocol
import uniffi.offline_protocol.RelayConfig
import uniffi.offline_protocol.RelayPriority

/**
 * Applies optional `dors` and `relay` sections from a host config JSON onto a
 * live [OfflineProtocol] instance after [ProtocolConfig] construction.
 *
 * Shared by the React Native module and the Flutter plugin (ADR 0025). Mirrors
 * `applyInitialRuntimeConfig` on iOS.
 */
internal object InitialRuntimeConfigApplier {

    private object Limits {
        const val MIN_BATTERY_LEVEL = 0
        const val MAX_BATTERY_LEVEL = 100
        const val MIN_HISTORY_WINDOW = 1L
        const val MAX_HISTORY_WINDOW = 100L
    }

    fun apply(
        proto: OfflineProtocol,
        json: JSONObject,
        onDiagnostic: ((level: String, message: String, context: Map<String, Any?>?) -> Unit)? = null,
    ) {
        json.optJSONObject("dors")?.let { dorsJson ->
            try {
                val baseConfig = proto.getDorsConfig()
                val updatedConfig = baseConfig.copy(
                    preferOnline = dorsJson.optBooleanCompat("preferOnline", "prefer_online")
                        ?: baseConfig.preferOnline,
                    switchHysteresis = dorsJson.optDoubleCompat("switchHysteresis", "switch_hysteresis")
                        ?.toFloat()
                        ?.coerceAtLeast(0f)
                        ?: baseConfig.switchHysteresis,
                    switchCooldownSecs = dorsJson.optLongCompat("switchCooldownSecs", "switch_cooldown_secs")
                        ?.coerceAtLeast(0)
                        ?.toULong()
                        ?: baseConfig.switchCooldownSecs,
                    bleToWifiRetryThreshold = dorsJson.optIntCompat("bleToWifiRetryThreshold", "ble_to_wifi_retry_threshold")
                        ?.coerceAtLeast(0)
                        ?.toUInt()
                        ?: baseConfig.bleToWifiRetryThreshold,
                    minSuccessRateBeforeEscalation = dorsJson.optDoubleCompat(
                        "minSuccessRateBeforeEscalation",
                        "min_success_rate_before_escalation",
                    )
                        ?.toFloat()
                        ?.coerceIn(0f, 1f)
                        ?: baseConfig.minSuccessRateBeforeEscalation,
                    minBleSamplesBeforeSuccessRateEscalation = dorsJson.optLongCompat(
                        "minBleSamplesBeforeSuccessRateEscalation",
                        "min_ble_samples_before_success_rate_escalation",
                    )
                        ?.coerceAtLeast(0)
                        ?.toULong()
                        ?: baseConfig.minBleSamplesBeforeSuccessRateEscalation,
                    rssiSwitchThreshold = dorsJson.optIntCompat("rssiSwitchThreshold", "rssi_switch_threshold")
                        ?.coerceIn(Short.MIN_VALUE.toInt(), Short.MAX_VALUE.toInt())
                        ?.toShort()
                        ?: baseConfig.rssiSwitchThreshold,
                    congestionQueueThreshold = dorsJson.optLongCompat("congestionQueueThreshold", "congestion_queue_threshold")
                        ?.coerceAtLeast(0)
                        ?.toULong()
                        ?: baseConfig.congestionQueueThreshold,
                    stabilityWindowSecs = dorsJson.optLongCompat("stabilityWindowSecs", "stability_window_secs")
                        ?.coerceAtLeast(0)
                        ?.toULong()
                        ?: baseConfig.stabilityWindowSecs,
                    poorSignalDurationSecs = dorsJson.optLongCompat("poorSignalDurationSecs", "poor_signal_duration_secs")
                        ?.coerceAtLeast(0)
                        ?.toULong()
                        ?: baseConfig.poorSignalDurationSecs,
                    ttlEscalationThreshold = dorsJson.optIntCompat("ttlEscalationThreshold", "ttl_escalation_threshold")
                        ?.coerceIn(0, UByte.MAX_VALUE.toInt())
                        ?.toUByte()
                        ?: baseConfig.ttlEscalationThreshold,
                    congestionDurationSecs = dorsJson.optLongCompat("congestionDurationSecs", "congestion_duration_secs")
                        ?.coerceAtLeast(0)
                        ?.toULong()
                        ?: baseConfig.congestionDurationSecs,
                    ttlEscalationHoldSecs = dorsJson.optLongCompat("ttlEscalationHoldSecs", "ttl_escalation_hold_secs")
                        ?.coerceAtLeast(1)
                        ?.toULong()
                        ?: baseConfig.ttlEscalationHoldSecs,
                    historyWindowSize = dorsJson.optLongCompat("historyWindowSize", "history_window_size")
                        ?.let { max(Limits.MIN_HISTORY_WINDOW, min(Limits.MAX_HISTORY_WINDOW, it)) }
                        ?.toULong()
                        ?: baseConfig.historyWindowSize,
                    queueRecoveryRatio = dorsJson.optDoubleCompat("queueRecoveryRatio", "queue_recovery_ratio")
                        ?.toFloat()
                        ?.coerceIn(0f, 1f)
                        ?: baseConfig.queueRecoveryRatio,
                    lowBatteryThreshold = dorsJson.optIntCompat("lowBatteryThreshold", "low_battery_threshold")
                        ?.coerceIn(Limits.MIN_BATTERY_LEVEL, Limits.MAX_BATTERY_LEVEL)
                        ?.toUByte()
                        ?: baseConfig.lowBatteryThreshold,
                    relayMinBatteryLevel = dorsJson.optIntCompat("relayMinBatteryLevel", "relay_min_battery_level")
                        ?.coerceIn(Limits.MIN_BATTERY_LEVEL, Limits.MAX_BATTERY_LEVEL)
                        ?.toUByte()
                        ?: baseConfig.relayMinBatteryLevel,
                    relayOptimalConnectionCount = dorsJson.optIntCompat(
                        "relayOptimalConnectionCount",
                        "relay_optimal_connection_count",
                    )
                        ?.coerceIn(0, UByte.MAX_VALUE.toInt())
                        ?.toUByte()
                        ?: baseConfig.relayOptimalConnectionCount,
                )
                proto.updateDorsConfig(updatedConfig)
                onDiagnostic?.invoke("info", "Applied initial DORS config", null)
            } catch (e: Exception) {
                onDiagnostic?.invoke(
                    "warning",
                    "Failed to apply initial DORS config",
                    mapOf("message" to (e.message ?: "unknown")),
                )
            }
        }

        json.optJSONObject("relay")?.let { relayJson ->
            try {
                val current = proto.getRelayConfig()
                val priorityRaw = relayJson.safeOptString("relayPriority", relayJson.safeOptString("relay_priority"))
                val priority = normalizeRelayPriority(priorityRaw) ?: current.relayPriority
                val updated = RelayConfig(
                    minBatteryForRelay = relayJson.optIntCompat("minBatteryForRelay", "min_battery_for_relay")
                        ?.coerceIn(Limits.MIN_BATTERY_LEVEL, Limits.MAX_BATTERY_LEVEL)?.toUByte()
                        ?: current.minBatteryForRelay,
                    allowRelay = relayJson.optBooleanCompat("allowRelay", "allow_relay") ?: current.allowRelay,
                    relayPriority = priority,
                )
                proto.updateRelayConfig(updated)
                onDiagnostic?.invoke(
                    "info",
                    "Applied initial relay config",
                    mapOf("priority" to relayPriorityName(updated.relayPriority)),
                )
            } catch (e: Exception) {
                onDiagnostic?.invoke(
                    "warning",
                    "Failed to apply initial relay config",
                    mapOf("message" to (e.message ?: "unknown")),
                )
            }
        }
    }

    internal fun normalizeRelayPriority(priority: String?): RelayPriority? {
        if (priority.isNullOrBlank()) {
            return null
        }
        return when (priority.lowercase()) {
            "never", "low" -> RelayPriority.NEVER
            "auto", "medium" -> RelayPriority.AUTO
            "always", "high" -> RelayPriority.ALWAYS
            else -> null
        }
    }

    internal fun relayPriorityName(priority: RelayPriority?): String = when (priority) {
        RelayPriority.NEVER -> "never"
        RelayPriority.ALWAYS -> "always"
        else -> "auto"
    }
}
