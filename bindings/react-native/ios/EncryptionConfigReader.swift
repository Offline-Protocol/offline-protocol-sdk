import Foundation

/// The encryption section of the `create()` config JSON, resolved to
/// effective values.
///
/// Mirrors android/ `ProtocolConfigParser` — keep the read order and
/// precedence in sync: nested home under `encryption` first, then top level,
/// camelCase or snake_case, defaults on. Both shapes are accepted so a
/// sender-side shape change can never silently revert a flag to its default —
/// the four flags below were read nested-only while the JS wrapper sent the
/// flat shape, silently discarding every app-set value.
///
/// Foundation-only on purpose: the SwiftPM test harness (Package.swift)
/// compiles this file without React or the Generated UniFFI module, so the
/// values struct uses plain types and `OfflineProtocolModule` maps them onto
/// the UniFFI `ProtocolConfig` (including the `OverflowPolicy` enum, which
/// must not be imported here).
struct EncryptionConfigValues: Equatable {
    var enabled: Bool
    var autoKeyExchange: Bool
    var storePending: Bool
    var requireEncryption: Bool
    var compactEnvelopeEnabled: Bool
    var richPayloadEnabled: Bool
    var maxPendingPerPeer: UInt64
    var maxPendingGlobal: UInt64
    var pendingTtlMs: UInt64
    /// Raw policy string ("drop_oldest" / "drop_newest"); the module maps it
    /// onto the Generated `OverflowPolicy` enum.
    var overflowPolicyRaw: String
}

enum EncryptionConfigReader {

    static func read(_ raw: [String: Any]) -> EncryptionConfigValues {
        let nested = raw["encryption"] as? [String: Any] ?? [:]

        let enabled = bool(nested, "enabled")
            ?? bool(raw, "encryptionEnabled", "encryption_enabled")
            ?? true
        let autoKeyExchange = bool(nested, "autoKeyExchange", "auto_key_exchange")
            ?? bool(raw, "autoKeyExchange", "auto_key_exchange")
            ?? true
        let storePending = bool(nested, "storePending", "store_pending")
            ?? bool(raw, "storePending", "store_pending")
            ?? true
        // Fail-closed default (SEC-M3): plaintext operation is an explicit opt-out.
        let requireEncryption = bool(nested, "requireEncryption", "require_encryption")
            ?? bool(raw, "requireEncryption", "require_encryption")
            ?? true
        let compactEnvelopeEnabled = bool(nested, "compactEnvelopeEnabled", "compact_envelope_enabled")
            ?? bool(raw, "compactEnvelopeEnabled", "compact_envelope_enabled")
            ?? true
        let richPayloadEnabled = bool(nested, "richPayloadEnabled", "rich_payload_enabled")
            ?? bool(raw, "richPayloadEnabled", "rich_payload_enabled")
            ?? true

        let pendingQueue = (nested["pendingQueue"] as? [String: Any])
            ?? (nested["pending_queue"] as? [String: Any])
            ?? [:]
        let maxPendingPerPeer = number(pendingQueue, "maxPendingPerPeer", "max_pending_per_peer")
            ?? number(raw, "maxPendingPerPeer", "max_pending_per_peer")
            ?? 64
        let maxPendingGlobal = number(pendingQueue, "maxPendingGlobal", "max_pending_global")
            ?? number(raw, "maxPendingGlobal", "max_pending_global")
            ?? 4096
        let pendingTtlMs = number(pendingQueue, "pendingTtlMs", "pending_ttl_ms")
            ?? number(raw, "pendingTtlMs", "pending_ttl_ms")
            ?? 120_000
        let overflowPolicyRaw = string(pendingQueue, "overflowPolicy", "overflow_policy")
            ?? string(raw, "overflowPolicy", "overflow_policy")
            ?? "drop_oldest"

        return EncryptionConfigValues(
            enabled: enabled,
            autoKeyExchange: autoKeyExchange,
            storePending: storePending,
            requireEncryption: requireEncryption,
            compactEnvelopeEnabled: compactEnvelopeEnabled,
            richPayloadEnabled: richPayloadEnabled,
            maxPendingPerPeer: maxPendingPerPeer,
            maxPendingGlobal: maxPendingGlobal,
            pendingTtlMs: pendingTtlMs,
            overflowPolicyRaw: overflowPolicyRaw
        )
    }

    private static func bool(_ dict: [String: Any], _ keys: String...) -> Bool? {
        for key in keys {
            if let value = dict[key] as? Bool {
                return value
            }
        }
        return nil
    }

    private static func number(_ dict: [String: Any], _ keys: String...) -> UInt64? {
        for key in keys {
            if let value = dict[key] as? NSNumber {
                return value.uint64Value
            }
        }
        return nil
    }

    private static func string(_ dict: [String: Any], _ keys: String...) -> String? {
        for key in keys {
            if let value = dict[key] as? String {
                return value
            }
        }
        return nil
    }
}
