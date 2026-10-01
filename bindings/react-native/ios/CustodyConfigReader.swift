import Foundation

/// The custody section of the `create()` config JSON (`docs/spec/custody.md`).
///
/// Mirrors android/ `ProtocolConfigParser`'s custody block, and keeps the
/// read order and precedence in sync: nested home under `custody`, camelCase
/// or snake_case within it.
///
/// Every value is optional and stays optional, for the reason the mesh
/// forwarding reader beside it gives: an absent field must reach the core
/// absent, because the core owns every default, and here the default that
/// matters most is "off". A reader that filled `enabled` in with `false`
/// would be a second copy of that default, and the release that ever flips
/// it would keep forcing `false` for every app that omitted the section.
///
/// Foundation-only on purpose: the SwiftPM test harness (Package.swift)
/// compiles this file without React or the Generated UniFFI module, so the
/// overflow policy is carried as the string the app wrote and mapped onto the
/// UniFFI enum by `OfflineProtocolModule`.
struct CustodyConfigValues: Equatable {
    var enabled: Bool?
    var holdMs: UInt64?
    var maxEntriesPerDepositor: UInt64?
    var maxBytesPerDepositor: UInt64?
    var maxEntries: UInt64?
    var maxBytes: UInt64?
    var strangerMaxEntries: UInt64?
    var strangerMaxBytes: UInt64?
    /// The policy as the app spelled it (`drop_oldest` or `drop_newest`).
    var overflowPolicy: String?
}

enum CustodyConfigReader {

    /// Returns nil when the app set no custody section at all, so the module
    /// passes nil across the FFI and the core keeps every default.
    static func read(_ raw: [String: Any]) -> CustodyConfigValues? {
        guard let nested = raw["custody"] as? [String: Any] else {
            return nil
        }

        return CustodyConfigValues(
            enabled: bool(nested, "enabled"),
            holdMs: uint64(nested, "holdMs", "hold_ms"),
            maxEntriesPerDepositor: uint64(nested, "maxEntriesPerDepositor", "max_entries_per_depositor"),
            maxBytesPerDepositor: uint64(nested, "maxBytesPerDepositor", "max_bytes_per_depositor"),
            maxEntries: uint64(nested, "maxEntries", "max_entries"),
            maxBytes: uint64(nested, "maxBytes", "max_bytes"),
            strangerMaxEntries: uint64(nested, "strangerMaxEntries", "stranger_max_entries"),
            strangerMaxBytes: uint64(nested, "strangerMaxBytes", "stranger_max_bytes"),
            overflowPolicy: string(nested, "overflowPolicy", "overflow_policy")
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

    // Clamped rather than converted: the value is app-supplied JS, so a
    // negative would trap the unsigned initializer outright. Clamped to zero
    // it reaches the core's own validation, which is the one place that gets
    // to decide what is legal.
    private static func uint64(_ dict: [String: Any], _ keys: String...) -> UInt64? {
        for key in keys {
            // A JSON boolean also arrives as an NSNumber, so it is excluded by
            // its CoreFoundation type rather than by `is Bool`: Swift bridges
            // the numbers 0 and 1 to Bool as well, and testing that would
            // read a legitimate `1` as unset.
            if let value = dict[key] as? NSNumber, CFGetTypeID(value) != CFBooleanGetTypeID() {
                return UInt64(clamping: value.int64Value)
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
