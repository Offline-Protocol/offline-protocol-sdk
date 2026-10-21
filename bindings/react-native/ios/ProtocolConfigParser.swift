//
// ProtocolConfigParser.swift
//
// Parses the JSON config string from a host wrapper into UniFFI ProtocolConfig.
// Shared by the React Native module, the Flutter plugin, and the Swift package
// assembly (ADR 0025). Mirrors android/ ProtocolConfigParser.kt: keep read
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

        // Encryption section (four flags, compact-envelope kill switch,
        // pending queue): read by EncryptionConfigReader, Foundation-only so
        // the SwiftPM suite covers it; mirrors ProtocolConfigParser.kt on
        // Android, keep the read order and precedence in sync.
        let encryption = EncryptionConfigReader.read(raw)
        let overflowPolicy: OverflowPolicy
        switch encryption.overflowPolicyRaw.lowercased() {
        case "drop_newest":
            overflowPolicy = .dropNewest
        default:
            overflowPolicy = .dropOldest
        }

        // binaryWireEnabled (kill switch, default on): top level only, which
        // IS its home in both the JS config and the flat UniFFI dictionary.
        // camelCase or snake_case.
        let binaryWireEnabled = raw["binaryWireEnabled"] as? Bool
            ?? raw["binary_wire_enabled"] as? Bool ?? true

        // Nostr sealing (kill switch, default on). Nested home under
        // `transports.nostr.sealingEnabled`, where the rest of the Nostr
        // transport settings live, then the flat key, both cases. Same
        // nested-wins-over-flat rule as `encryption` and `group`; mirrors
        // ProtocolConfigParser.kt, keep in sync.
        let nostrRaw = (raw["transports"] as? [String: Any])?["nostr"] as? [String: Any]
        let nostrSealingEnabled = nostrRaw?["sealingEnabled"] as? Bool
            ?? nostrRaw?["sealing_enabled"] as? Bool
            ?? raw["nostrSealingEnabled"] as? Bool
            ?? raw["nostr_sealing_enabled"] as? Bool
            ?? true

        // Nostr cold contact (key-package publication + peer resolution, kill
        // switch, default on). Same nested-then-flat shape as sealing above.
        let nostrColdContactEnabled = nostrRaw?["coldContactEnabled"] as? Bool
            ?? nostrRaw?["cold_contact_enabled"] as? Bool
            ?? raw["nostrColdContactEnabled"] as? Bool
            ?? raw["nostr_cold_contact_enabled"] as? Bool
            ?? true

        // Nostr username discovery (claim publication + username resolution).
        // Same nested-then-flat shape, but the default is FALSE: publishing
        // binds a human-readable name to an address in a public place, which
        // an app must opt into rather than inherit.
        let nostrUsernameDiscoveryEnabled = nostrRaw?["usernameDiscoveryEnabled"] as? Bool
            ?? nostrRaw?["username_discovery_enabled"] as? Bool
            ?? raw["nostrUsernameDiscoveryEnabled"] as? Bool
            ?? raw["nostr_username_discovery_enabled"] as? Bool
            ?? false

        // Group section (nested home under `group`, then top level, both
        // cases; same shape rules as `encryption`; mirrors
        // ProtocolConfigParser.kt, keep in sync). These were UniFFI-only
        // until the broadcast default flipped on: with no JS-reachable
        // opt-out, an RN app could not force per-member fan-out.
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
        // Default false, see GroupConfig::enforce_admin_commits. Enabling it
        // makes this device refuse membership commits it cannot authorize,
        // which forks it from every member that accepted them.
        let groupEnforceAdminCommits = groupRaw?["enforceAdminCommits"] as? Bool
            ?? groupRaw?["enforce_admin_commits"] as? Bool
            ?? raw["groupEnforceAdminCommits"] as? Bool
            ?? raw["group_enforce_admin_commits"] as? Bool
            ?? false

        // Mesh forwarding section: read by MeshRelayConfigReader, Foundation-only
        // so the SwiftPM suite covers it; mirrors ProtocolConfigParser.kt, keep
        // the read order and precedence in sync. Every field stays optional all
        // the way to the core, which owns the defaults.
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

        // Custody section: read by CustodyConfigReader, Foundation-only like
        // the mesh forwarding reader and for the same reason; mirrors
        // ProtocolConfigParser.kt, keep the read order in sync. Absent stays
        // absent all the way to the core, whose default is off; the reader
        // carries the overflow policy as the app spelled it, and an unknown
        // spelling stays absent rather than becoming a default written here.
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

        // Data layer section (nested home under `data`, both cases). Same
        // rule as meshRelay: absent stays absent, so the Rust default is the
        // only default. Kept in step with android/ ProtocolConfigParser:
        // read order and precedence must match.
        let dataSection = raw["data"] as? [String: Any]
        let dataEnabled = dataSection?["enabled"] as? Bool

        // Security section, with a top-level spelling accepted too. Absent
        // stays absent, so the Rust default is the only default; see the
        // assignment below for why a literal here would be the bug.
        //
        // Unlike `data`, a flat spelling is honoured: this flag is the lever an
        // app reaches for when its fleet's clocks are wrong and its control
        // plane has gone quiet, and a lever that silently does nothing because
        // it was written one level too high is worse than no lever.
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
            // Clamped, not converted: `initialTtl` is a public config field
            // of unbounded `number` type in TypeScript, so an application
            // passing 300 would trap here and abort inside `create()`.
            // Android truncates the same value through `toUByte()`.
            initialTtl: UInt8(min(255, max(0, raw["initialTtl"] as? Int ?? raw["initial_ttl"] as? Int ?? 8))),
            encryptionEnabled: encryption.enabled,
            autoKeyExchange: encryption.autoKeyExchange,
            storePending: encryption.storePending,
            requireEncryption: encryption.requireEncryption,
            maxPendingPerPeer: encryption.maxPendingPerPeer,
            maxPendingGlobal: encryption.maxPendingGlobal,
            pendingTtlMs: encryption.pendingTtlMs,
            overflowPolicy: overflowPolicy,
            // `clamping:` and not `UInt32(_:)`: the value is app-supplied JS,
            // and a negative or oversized number traps the initializer.
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

        // Assigned only when the app actually sent it. Writing `?? false`
        // above would restate the default as a literal here, and the literal
        // is what survives a change to the real default: the release that
        // flips it on would keep forcing `false` for every app that omits the
        // section, exactly as `config.relay` sat at its defaults for several
        // releases.
        // Always assigned: the reader resolves it with the same default the
        // UDL declares, so there is no literal here to outlive a change to it.
        config.encryptServiceMessages = encryption.encryptServiceMessages
        if let dataEnabled {
            config.dataEnabled = dataEnabled
        }
        if let controlFreshnessEnforced {
            config.controlFreshnessEnforced = controlFreshnessEnforced
        }

        return (config, raw)
    }

    /// The custody overflow policy as the app spelled it, or nil for a
    /// spelling this build does not know: nil reaches the core as "keep the
    /// default", never as a default chosen here.
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
