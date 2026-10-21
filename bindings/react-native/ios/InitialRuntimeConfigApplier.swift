//
// InitialRuntimeConfigApplier.swift
//
// Applies optional `dors` and `relay` sections from host config JSON onto a live
// OfflineProtocol after ProtocolConfig construction. Shared by the React
// Native module and the Flutter plugin (ADR 0025).
//

import Foundation

enum InitialRuntimeConfigApplier {
    typealias DiagnosticHandler = (_ level: String, _ message: String, _ context: [String: Any]?) -> Void

    static func apply(
        proto: OfflineProtocol,
        rawConfig: [String: Any],
        onDiagnostic: DiagnosticHandler? = nil
    ) {
        if let dorsDict = rawConfig["dors"] as? [String: Any] {
            let preferOnline = dorsDict["preferOnline"] as? Bool ?? dorsDict["prefer_online"] as? Bool ?? false
            let switchHysteresis = Float((dorsDict["switchHysteresis"] as? NSNumber)?.doubleValue
                                         ?? (dorsDict["switch_hysteresis"] as? NSNumber)?.doubleValue
                                         ?? 15.0)
            let switchCooldown = UInt64((dorsDict["switchCooldownSecs"] as? NSNumber)?.uint64Value
                                        ?? (dorsDict["switch_cooldown_secs"] as? NSNumber)?.uint64Value
                                        ?? 20)
            let bleRetry = UInt32((dorsDict["bleToWifiRetryThreshold"] as? NSNumber)?.uint32Value
                                  ?? (dorsDict["ble_to_wifi_retry_threshold"] as? NSNumber)?.uint32Value
                                  ?? 2)
            let minSuccessRate = Float((dorsDict["minSuccessRateBeforeEscalation"] as? NSNumber)?.floatValue
                                   ?? (dorsDict["min_success_rate_before_escalation"] as? NSNumber)?.floatValue
                                   ?? 0.3)
            let minSuccessRateClamped = min(max(minSuccessRate, 0.0), 1.0)
            let minBleSamples = UInt64((dorsDict["minBleSamplesBeforeSuccessRateEscalation"] as? NSNumber)?.uint64Value
                                      ?? (dorsDict["min_ble_samples_before_success_rate_escalation"] as? NSNumber)?.uint64Value
                                      ?? 5)
            let rssiThreshold = Int16((dorsDict["rssiSwitchThreshold"] as? NSNumber)?.int16Value
                                      ?? (dorsDict["rssi_switch_threshold"] as? NSNumber)?.int16Value
                                      ?? -85)
            let congestionThreshold = UInt64((dorsDict["congestionQueueThreshold"] as? NSNumber)?.uint64Value
                                             ?? (dorsDict["congestion_queue_threshold"] as? NSNumber)?.uint64Value
                                             ?? 50)
            let stabilityWindow = UInt64((dorsDict["stabilityWindowSecs"] as? NSNumber)?.uint64Value
                                         ?? (dorsDict["stability_window_secs"] as? NSNumber)?.uint64Value
                                         ?? 8)
            let poorSignalDuration = UInt64((dorsDict["poorSignalDurationSecs"] as? NSNumber)?.uint64Value
                                            ?? (dorsDict["poor_signal_duration_secs"] as? NSNumber)?.uint64Value
                                            ?? 10)
            let ttlThreshold = UInt8((dorsDict["ttlEscalationThreshold"] as? NSNumber)?.uint8Value
                                     ?? (dorsDict["ttl_escalation_threshold"] as? NSNumber)?.uint8Value
                                     ?? 2)
            let congestionDuration = UInt64((dorsDict["congestionDurationSecs"] as? NSNumber)?.uint64Value
                                            ?? (dorsDict["congestion_duration_secs"] as? NSNumber)?.uint64Value
                                            ?? 10)
            let ttlHold = UInt64((dorsDict["ttlEscalationHoldSecs"] as? NSNumber)?.uint64Value
                                 ?? (dorsDict["ttl_escalation_hold_secs"] as? NSNumber)?.uint64Value
                                 ?? 20)
            let historyWindowRaw = UInt64((dorsDict["historyWindowSize"] as? NSNumber)?.uint64Value
                                          ?? (dorsDict["history_window_size"] as? NSNumber)?.uint64Value
                                          ?? 10)
            // Clamped in the unsigned domain, never through `Int`: a negative
            // `historyWindowSize` from JavaScript reaches `uint64Value` as
            // `UInt64.max`, and narrowing that to `Int` traps and aborts the
            // application before the clamp around it can run.
            let historyWindow = max(1, min(100, historyWindowRaw))
            let rawQueueRecovery = Float((dorsDict["queueRecoveryRatio"] as? NSNumber)?.floatValue
                                         ?? (dorsDict["queue_recovery_ratio"] as? NSNumber)?.floatValue
                                         ?? 0.5)
            let queueRecovery = min(max(rawQueueRecovery, 0.0), 1.0)
            let lowBattery = UInt8(min(100, max(0, Int((dorsDict["lowBatteryThreshold"] as? NSNumber)?.intValue
                                                       ?? (dorsDict["low_battery_threshold"] as? NSNumber)?.intValue
                                                       ?? 20))))
            let relayMinBattery = UInt8(min(100, max(0, Int((dorsDict["relayMinBatteryLevel"] as? NSNumber)?.intValue
                                                            ?? (dorsDict["relay_min_battery_level"] as? NSNumber)?.intValue
                                                            ?? 30))))
            let relayOptimalConnections = UInt8(min(255, max(0, Int((dorsDict["relayOptimalConnectionCount"] as? NSNumber)?.intValue
                                                                    ?? (dorsDict["relay_optimal_connection_count"] as? NSNumber)?.intValue
                                                                    ?? 4))))

            let dorsConfig = DorsConfig(
                preferOnline: preferOnline,
                switchHysteresis: switchHysteresis,
                switchCooldownSecs: switchCooldown,
                bleToWifiRetryThreshold: bleRetry,
                minSuccessRateBeforeEscalation: minSuccessRateClamped,
                minBleSamplesBeforeSuccessRateEscalation: minBleSamples,
                rssiSwitchThreshold: rssiThreshold,
                congestionQueueThreshold: congestionThreshold,
                stabilityWindowSecs: stabilityWindow,
                poorSignalDurationSecs: poorSignalDuration,
                ttlEscalationThreshold: ttlThreshold,
                congestionDurationSecs: congestionDuration,
                ttlEscalationHoldSecs: ttlHold,
                historyWindowSize: historyWindow,
                queueRecoveryRatio: queueRecovery,
                lowBatteryThreshold: lowBattery,
                relayMinBatteryLevel: relayMinBattery,
                relayOptimalConnectionCount: relayOptimalConnections
            )

            do {
                try proto.updateDorsConfig(config: dorsConfig)
                onDiagnostic?("info", "Applied initial DORS config", nil)
            } catch {
                onDiagnostic?("warning", "Failed to apply initial DORS config", [
                    "error": error.localizedDescription,
                ])
            }
        }

        // The whole relay section, not just the priority: allowRelay and
        // minBatteryForRelay used to be parsed by nothing, leaving
        // `config.relay` on mobile permanently at its defaults.
        if let relayDict = rawConfig["relay"] as? [String: Any] {
            do {
                let updated = mergedRelayConfig(current: proto.getRelayConfig(), from: relayDict)
                try proto.updateRelayConfig(config: updated)
                onDiagnostic?("info", "Applied initial relay config", [
                    "priority": relayPriorityName(updated.relayPriority),
                ])
            } catch {
                onDiagnostic?("warning", "Failed to apply initial relay config", [
                    "error": error.localizedDescription,
                ])
            }
        }
    }

    static func mergedRelayConfig(current: RelayConfig, from dict: [String: Any]) -> RelayConfig {
        func number(_ keys: String...) -> NSNumber? {
            for key in keys {
                if let value = dict[key] as? NSNumber { return value }
            }
            return nil
        }
        func flag(_ keys: String...) -> Bool? {
            for key in keys {
                if let value = dict[key] as? Bool { return value }
                if let value = dict[key] as? NSNumber { return value.boolValue }
            }
            return nil
        }
        let priorityRaw = (dict["relayPriority"] as? String) ?? (dict["relay_priority"] as? String)
        return RelayConfig(
            minBatteryForRelay: number("minBatteryForRelay", "min_battery_for_relay")
                .map { UInt8(min(100, max(0, $0.intValue))) } ?? current.minBatteryForRelay,
            allowRelay: flag("allowRelay", "allow_relay") ?? current.allowRelay,
            relayPriority: normalizeRelayPriority(priorityRaw) ?? current.relayPriority
        )
    }

    /// Accepts both the current vocabulary and the pre-0.22 `low`/`medium`/`high`
    /// spelling of the same three values, so an app that has not migrated its
    /// config keeps working.
    static func normalizeRelayPriority(_ priority: String?) -> RelayPriority? {
        guard let value = priority?.lowercased(), !value.isEmpty else {
            return nil
        }
        switch value {
        case "never", "low":
            return .never
        case "auto", "medium":
            return .auto
        case "always", "high":
            return .always
        default:
            return nil
        }
    }

    static func relayPriorityName(_ priority: RelayPriority) -> String {
        switch priority {
        case .never: return "never"
        case .auto: return "auto"
        case .always: return "always"
        @unknown default: return "auto"
        }
    }
}
