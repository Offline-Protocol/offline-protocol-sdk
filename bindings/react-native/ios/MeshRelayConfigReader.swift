import Foundation

/// The mesh forwarding section of the `create()` config JSON.
///
/// Mirrors android/ `ProtocolConfigParser`'s mesh-relay block — keep the read
/// order and precedence in sync: nested home under `meshRelay` (or
/// `mesh_relay`), camelCase or snake_case within it.
///
/// Every value is optional and stays optional. Unlike the encryption reader
/// beside it, this one resolves *nothing*: an absent field must reach the core
/// absent, because the core owns every default. A default written here would
/// be a second copy free to drift from the Rust one, with no runtime update
/// path to correct it, and a partial section from an app would silently reset
/// every field it did not mention — the failure DORS shipped.
///
/// Foundation-only on purpose: the SwiftPM test harness (Package.swift)
/// compiles this file without React or the Generated UniFFI module, so the
/// values struct uses plain types and `OfflineProtocolModule` maps them onto
/// the UniFFI `MeshRelayConfig`.
struct MeshRelayConfigValues: Equatable {
    var maxTtl: UInt8?
    var denseMaxTtl: UInt8?
    var denseDegree: UInt64?
    var fanout: UInt64?
    var jitterMinMs: UInt64?
    var jitterMaxMs: UInt64?
    var ratePerSec: Float?
    var burst: Float?
    var peerRatePerSec: Float?
    var peerBurst: Float?
    var queueCapacity: UInt64?
    var biasMinScale: Float?
    var biasMaxHandicapMs: UInt64?
    var activityWindowMs: UInt64?
    var activityMinForwards: UInt64?
    var activityIdleWindows: UInt32?
}

enum MeshRelayConfigReader {

    /// Returns nil when the app set no mesh-relay section at all, so the
    /// module passes nil across the FFI and the core keeps every default.
    static func read(_ raw: [String: Any]) -> MeshRelayConfigValues? {
        guard let nested = (raw["meshRelay"] as? [String: Any])
            ?? (raw["mesh_relay"] as? [String: Any]) else {
            return nil
        }

        return MeshRelayConfigValues(
            maxTtl: uint8(nested, "maxTtl", "max_ttl"),
            denseMaxTtl: uint8(nested, "denseMaxTtl", "dense_max_ttl"),
            denseDegree: uint64(nested, "denseDegree", "dense_degree"),
            fanout: uint64(nested, "fanout"),
            jitterMinMs: uint64(nested, "jitterMinMs", "jitter_min_ms"),
            jitterMaxMs: uint64(nested, "jitterMaxMs", "jitter_max_ms"),
            ratePerSec: float(nested, "ratePerSec", "rate_per_sec"),
            burst: float(nested, "burst"),
            peerRatePerSec: float(nested, "peerRatePerSec", "peer_rate_per_sec"),
            peerBurst: float(nested, "peerBurst", "peer_burst"),
            queueCapacity: uint64(nested, "queueCapacity", "queue_capacity"),
            biasMinScale: float(nested, "biasMinScale", "bias_min_scale"),
            biasMaxHandicapMs: uint64(nested, "biasMaxHandicapMs", "bias_max_handicap_ms"),
            activityWindowMs: uint64(nested, "activityWindowMs", "activity_window_ms"),
            activityMinForwards: uint64(nested, "activityMinForwards", "activity_min_forwards"),
            activityIdleWindows: uint32(nested, "activityIdleWindows", "activity_idle_windows")
        )
    }

    // Clamped rather than converted: the value is app-supplied JS, so a
    // negative would trap the unsigned initializer outright. Clamped to zero
    // it reaches the core's own validation, which is the one place that gets
    // to decide what is legal.

    private static func uint8(_ dict: [String: Any], _ keys: String...) -> UInt8? {
        guard let value = number(dict, keys) else { return nil }
        return UInt8(clamping: value.int64Value)
    }

    private static func uint32(_ dict: [String: Any], _ keys: String...) -> UInt32? {
        guard let value = number(dict, keys) else { return nil }
        return UInt32(clamping: value.int64Value)
    }

    private static func uint64(_ dict: [String: Any], _ keys: String...) -> UInt64? {
        guard let value = number(dict, keys) else { return nil }
        return UInt64(clamping: value.int64Value)
    }

    private static func float(_ dict: [String: Any], _ keys: String...) -> Float? {
        guard let value = number(dict, keys) else { return nil }
        return value.floatValue
    }

    private static func number(_ dict: [String: Any], _ keys: [String]) -> NSNumber? {
        for key in keys {
            // Bool is bridged as NSNumber, so an explicit exclusion keeps a
            // stray `true` from arriving as the number 1.
            if let value = dict[key] as? NSNumber, !(dict[key] is Bool) {
                return value
            }
        }
        return nil
    }
}
