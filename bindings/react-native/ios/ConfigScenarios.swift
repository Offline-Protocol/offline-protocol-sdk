//
// ConfigScenarios.swift
//
// Opinionated ProtocolConfig presets for native iOS / Flutter hosts.
// Mirrors bindings/react-native/src/configScenarios.ts (G4 PR1); keep scenario
// ids and transport posture in sync. No UDL or wire changes.
//

import Foundation

/// Named integration scenarios covered by docs and tests.
public enum ConfigScenario: String, CaseIterable, Sendable {
    case localBle
    case proximityMesh
    case internetRelay
    case nostrMesh
    /// Cleartext-only local testing. Never ship to production users.
    case developmentCleartext
}

public enum ConfigScenarios {
    public static let scenarioIds: [ConfigScenario] = ConfigScenario.allCases

    /// Builds a full `ProtocolConfig` for direct UniFFI use.
    public static func forScenario(
        _ scenario: ConfigScenario,
        appId: String,
        profile: String,
        customize: ((inout ProtocolConfig) -> Void)? = nil
    ) -> ProtocolConfig {
        var config = meshBaseline(appId: appId, profile: profile)
        applyScenario(scenario, to: &config)
        customize?(&config)
        return config
    }

    /// JSON for `ProtocolEngineHost` / Flutter `create(configJson:)`.
    public static func forScenarioJson(
        _ scenario: ConfigScenario,
        appId: String,
        profile: String,
        extra: [String: Any] = [:]
    ) throws -> String {
        var payload: [String: Any] = [
            "appId": appId,
            "profile": profile,
        ]
        mergeJson(scenarioPresetJson(scenario), into: &payload)
        mergeJson(extra, into: &payload)
        let data = try JSONSerialization.data(withJSONObject: payload)
        guard let text = String(data: data, encoding: .utf8) else {
            throw NSError(
                domain: "OfflineProtocol",
                code: -1,
                userInfo: [NSLocalizedDescriptionKey: "Failed to encode scenario JSON"]
            )
        }
        return text
    }

    // MARK: - Private

    private static func meshBaseline(appId: String, profile: String) -> ProtocolConfig {
        let encryption = EncryptionConfigReader.read([:])
        let overflow: OverflowPolicy =
            encryption.overflowPolicyRaw.lowercased() == "drop_newest" ? .dropNewest : .dropOldest
        return ProtocolConfig(
            appId: appId,
            profile: profile,
            bleEnabled: true,
            wifiDirectEnabled: false,
            internetEnabled: false,
            reticulumEnabled: false,
            nostrEnabled: false,
            preferOnline: false,
            initialTtl: 8,
            encryptionEnabled: encryption.enabled,
            autoKeyExchange: encryption.autoKeyExchange,
            storePending: encryption.storePending,
            requireEncryption: encryption.requireEncryption,
            maxPendingPerPeer: encryption.maxPendingPerPeer,
            maxPendingGlobal: encryption.maxPendingGlobal,
            pendingTtlMs: encryption.pendingTtlMs,
            overflowPolicy: overflow,
            binaryWireEnabled: true,
            nostrSealingEnabled: true,
            nostrColdContactEnabled: true,
            compactEnvelopeEnabled: encryption.compactEnvelopeEnabled,
            richPayloadEnabled: encryption.richPayloadEnabled,
            cryptoRecoveryEnabled: encryption.cryptoRecoveryEnabled,
            encryptServiceMessages: encryption.encryptServiceMessages
        )
    }

    private static func applyScenario(_ scenario: ConfigScenario, to config: inout ProtocolConfig) {
        switch scenario {
        case .localBle:
            break
        case .proximityMesh:
            config.wifiDirectEnabled = true
        case .internetRelay:
            config.internetEnabled = true
            config.preferOnline = true
        case .nostrMesh:
            config.nostrEnabled = true
        case .developmentCleartext:
            config.encryptionEnabled = false
            config.autoKeyExchange = false
            config.storePending = false
            config.requireEncryption = false
        }
    }

    /// JSON for host parsers: flat UniFFI flags plus nested `transports` / `dors`
    /// aligned with bindings/react-native/src/configScenarios.ts.
    private static func scenarioPresetJson(_ scenario: ConfigScenario) -> [String: Any] {
        var payload = flatTransportFlags(scenario)
        payload["transports"] = nestedTransports(scenario)
        switch scenario {
        case .localBle, .proximityMesh, .nostrMesh:
            break
        case .internetRelay:
            payload["preferOnline"] = true
            payload["dors"] = ["preferOnline": true]
        case .developmentCleartext:
            payload["encryption"] = ["enabled": false]
        }
        return payload
    }

    private static func flatTransportFlags(_ scenario: ConfigScenario) -> [String: Any] {
        var flags: [String: Any] = [
            "bleEnabled": true,
            "wifiDirectEnabled": false,
            "internetEnabled": false,
            "nostrEnabled": false,
            "reticulumEnabled": false,
        ]
        switch scenario {
        case .localBle:
            break
        case .proximityMesh:
            flags["wifiDirectEnabled"] = true
        case .internetRelay:
            flags["internetEnabled"] = true
        case .nostrMesh:
            flags["nostrEnabled"] = true
        case .developmentCleartext:
            flags["wifiDirectEnabled"] = false
            flags["internetEnabled"] = false
            flags["nostrEnabled"] = false
            flags["reticulumEnabled"] = false
        }
        return flags
    }

    private static func nestedTransports(_ scenario: ConfigScenario) -> [String: Any] {
        switch scenario {
        case .localBle:
            return [
                "ble": ["enabled": true],
                "wifiDirect": ["enabled": false],
                "internet": ["enabled": false],
                "nostr": ["enabled": false],
                "reticulum": ["enabled": false],
            ]
        case .proximityMesh:
            return [
                "ble": ["enabled": true],
                "wifiDirect": ["enabled": true],
                "internet": ["enabled": false],
                "nostr": ["enabled": false],
            ]
        case .internetRelay:
            return [
                "ble": ["enabled": true],
                "internet": ["enabled": true],
            ]
        case .nostrMesh:
            return [
                "ble": ["enabled": true],
                "nostr": ["enabled": true],
            ]
        case .developmentCleartext:
            return ["ble": ["enabled": true]]
        }
    }

    private static func mergeJson(_ layer: [String: Any], into base: inout [String: Any]) {
        for (key, value) in layer {
            if let nested = value as? [String: Any],
               let existing = base[key] as? [String: Any] {
                var merged = existing
                mergeJson(nested, into: &merged)
                base[key] = merged
            } else {
                base[key] = value
            }
        }
    }
}
