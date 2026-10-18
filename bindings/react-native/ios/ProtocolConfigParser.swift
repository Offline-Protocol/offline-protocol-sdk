//
// ProtocolConfigParser.swift
//
// Parses the JSON config string from a host wrapper into UniFFI ProtocolConfig.
// Shared by the React Native module, the Flutter plugin, and the Swift package
// assembly (ADR 0025). Mirrors android/ ProtocolConfigParser.kt — keep read
// order and precedence in sync.
//

import Foundation

enum ProtocolConfigParser {
    static func parse(_ configJson: String) throws -> (config: ProtocolConfig, raw: [String: Any]) {
        guard let jsonData = configJson.data(using: .utf8),
              let raw = try? JSONSerialization.jsonObject(with: jsonData) as? [String: Any] else {
            throw NSError(
                domain: "OfflineProtocol",
                code: -1,
                userInfo: [NSLocalizedDescriptionKey: "Invalid JSON"]
            )
        }

        let encryption = EncryptionConfigReader.read(raw)
        let overflowPolicy: OverflowPolicy
        switch encryption.overflowPolicyRaw.lowercased() {
        case "drop_newest":
            overflowPolicy = .dropNewest
        default:
            overflowPolicy = .dropOldest
        }

        let binaryWireEnabled = raw["binaryWireEnabled"] as? Bool
            ?? raw["binary_wire_enabled"] as? Bool ?? true

        let nostrRaw = (raw["transports"] as? [String: Any])?["nostr"] as? [String: Any]
        let nostrSealingEnabled = nostrRaw?["sealingEnabled"] as? Bool
            ?? nostrRaw?["sealing_enabled"] as? Bool
            ?? raw["nostrSealingEnabled"] as? Bool
            ?? raw["nostr_sealing_enabled"] as? Bool
            ?? true

        let nostrColdContactEnabled = nostrRaw?["coldContactEnabled"] as? Bool
            ?? nostrRaw?["cold_contact_enabled"] as? Bool
            ?? raw["nostrColdContactEnabled"] as? Bool
            ?? raw["nostr_cold_contact_enabled"] as? Bool
            ?? true

        let nostrUsernameDiscoveryEnabled = nostrRaw?["usernameDiscoveryEnabled"] as? Bool
            ?? nostrRaw?["username_discovery_enabled"] as? Bool
            ?? raw["nostrUsernameDiscoveryEnabled"] as? Bool
            ?? raw["nostr_username_discovery_enabled"] as? Bool
            ?? false

        let groupRaw = raw["group"] as? [String: Any]
        let maxGroupMembers = groupRaw?["maxGroupMembers"] as? Int
            ?? groupRaw?["max_group_members"] as? Int
            ?? raw["maxGroupMembers"] as? Int
            ?? raw["max_group_members"] as? Int
            ?? 256
        let groupRelayEnabled = groupRaw?["relayEnabled"] as? Bool
            ?? groupRaw?["relay_enabled"] as? Bool
            ?? raw["groupRelayEnabled"] as? Bool
            ?? raw["group_relay_enabled"] as? Bool
            ?? true
        let groupRelayBroadcastEnabled = groupRaw?["relayBroadcastEnabled"] as? Bool
            ?? groupRaw?["relay_broadcast_enabled"] as? Bool
            ?? raw["groupRelayBroadcastEnabled"] as? Bool
            ?? raw["group_relay_broadcast_enabled"] as? Bool
            ?? true
        let groupEnforceAdminCommits = groupRaw?["enforceAdminCommits"] as? Bool
            ?? groupRaw?["enforce_admin_commits"] as? Bool
            ?? raw["groupEnforceAdminCommits"] as? Bool
            ?? raw["group_enforce_admin_commits"] as? Bool
            ?? false

        let meshRelay = MeshRelayConfigReader.read(raw).map { values in
            MeshRelayConfig(
                maxTtl: values.maxTtl,
                denseMaxTtl: values.denseMaxTtl,
                denseDegree: values.denseDegree,
                fanout: values.fanout,
                jitterMinMs: values.jitterMinMs,
                jitterMaxMs: values.jitterMaxMs,
                ratePerSec: values.ratePerSec,
                burst: values.burst,
                peerRatePerSec: values.peerRatePerSec,
                peerBurst: values.peerBurst,
                queueCapacity: values.queueCapacity,
                biasMinScale: values.biasMinScale,
                biasMaxHandicapMs: values.biasMaxHandicapMs,
                activityWindowMs: values.activityWindowMs,
                activityMinForwards: values.activityMinForwards,
                activityIdleWindows: values.activityIdleWindows
            )
        }

        let custody = CustodyConfigReader.read(raw).map { values in
            CustodyConfig(
                enabled: values.enabled,
                holdMs: values.holdMs,
                maxEntriesPerDepositor: values.maxEntriesPerDepositor,
                maxBytesPerDepositor: values.maxBytesPerDepositor,
                maxEntries: values.maxEntries,
                maxBytes: values.maxBytes,
                strangerMaxEntries: values.strangerMaxEntries,
                strangerMaxBytes: values.strangerMaxBytes,
                overflowPolicy: values.overflowPolicy.flatMap(custodyOverflowPolicy)
            )
        }

        let dataSection = raw["data"] as? [String: Any]
        let dataEnabled = dataSection?["enabled"] as? Bool

        let securitySection = raw["security"] as? [String: Any]
        let controlFreshnessEnforced =
            securitySection?["controlFreshnessEnforced"] as? Bool
            ?? securitySection?["control_freshness_enforced"] as? Bool
            ?? raw["controlFreshnessEnforced"] as? Bool
            ?? raw["control_freshness_enforced"] as? Bool

        var config = ProtocolConfig(
            appId: raw["appId"] as? String ?? raw["app_id"] as? String ?? "",
            profile: raw["profile"] as? String ?? "",
            bleEnabled: raw["bleEnabled"] as? Bool ?? raw["ble_enabled"] as? Bool ?? true,
            wifiDirectEnabled: raw["wifiDirectEnabled"] as? Bool ?? raw["wifi_direct_enabled"] as? Bool ?? true,
            internetEnabled: raw["internetEnabled"] as? Bool ?? raw["internet_enabled"] as? Bool ?? true,
            reticulumEnabled: raw["reticulumEnabled"] as? Bool ?? raw["reticulum_enabled"] as? Bool ?? false,
            nostrEnabled: raw["nostrEnabled"] as? Bool ?? raw["nostr_enabled"] as? Bool ?? false,
            preferOnline: raw["preferOnline"] as? Bool ?? raw["prefer_online"] as? Bool ?? false,
            initialTtl: UInt8(min(255, max(0, raw["initialTtl"] as? Int ?? raw["initial_ttl"] as? Int ?? 8))),
            encryptionEnabled: encryption.enabled,
            autoKeyExchange: encryption.autoKeyExchange,
            storePending: encryption.storePending,
            requireEncryption: encryption.requireEncryption,
            maxPendingPerPeer: encryption.maxPendingPerPeer,
            maxPendingGlobal: encryption.maxPendingGlobal,
            pendingTtlMs: encryption.pendingTtlMs,
            overflowPolicy: overflowPolicy,
            maxGroupMembers: UInt32(clamping: maxGroupMembers),
            groupRelayEnabled: groupRelayEnabled,
            groupRelayBroadcastEnabled: groupRelayBroadcastEnabled,
            groupEnforceAdminCommits: groupEnforceAdminCommits,
            binaryWireEnabled: binaryWireEnabled,
            nostrSealingEnabled: nostrSealingEnabled,
            nostrColdContactEnabled: nostrColdContactEnabled,
            nostrUsernameDiscoveryEnabled: nostrUsernameDiscoveryEnabled,
            compactEnvelopeEnabled: encryption.compactEnvelopeEnabled,
            richPayloadEnabled: encryption.richPayloadEnabled,
            cryptoRecoveryEnabled: encryption.cryptoRecoveryEnabled,
            meshRelay: meshRelay,
            custody: custody
        )

        config.encryptServiceMessages = encryption.encryptServiceMessages
        if let dataEnabled {
            config.dataEnabled = dataEnabled
        }
        if let controlFreshnessEnforced {
            config.controlFreshnessEnforced = controlFreshnessEnforced
        }

        return (config, raw)
    }

    private static func custodyOverflowPolicy(_ raw: String) -> OverflowPolicy? {
        switch raw.lowercased() {
        case "drop_newest", "dropnewest":
            return .dropNewest
        case "drop_oldest", "dropoldest":
            return .dropOldest
        default:
            return nil
        }
    }
}
