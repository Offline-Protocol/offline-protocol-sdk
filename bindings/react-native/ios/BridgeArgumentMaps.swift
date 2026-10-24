#if canImport(Flutter)
import Flutter
#endif
import Foundation

/// Argument decoding for the Flutter method channel. Mirrors Android's
/// `FlutterBridgeArgs.kt` and the `ProtocolEngineHostBridge` argument helpers:
/// the Dart side sends named, camelCase keys, a missing or empty string is an
/// error rather than a silent default, and a number that cannot be what the
/// method needs is refused by name instead of trapping in a narrowing
/// conversion.
enum BridgeArgumentMaps {
    /// The map form of the method arguments. A method that takes no arguments
    /// never calls this, so a `nil` from Dart is only an error where a value is
    /// actually read.
    static func map(_ arguments: Any?, for method: String = "") throws -> [String: Any] {
        if let map = arguments as? [String: Any] {
            return map
        }
        if let map = arguments as? NSDictionary {
            var out: [String: Any] = [:]
            for (key, value) in map {
                out[String(describing: key)] = value
            }
            return out
        }
        let suffix = method.isEmpty ? "" : " (\(method))"
        throw HostError.invalidArgument("Expected a map argument\(suffix)")
    }

    static func requireString(_ map: [String: Any], _ key: String) throws -> String {
        guard let value = map[key] as? String, !value.isEmpty else {
            throw HostError.invalidArgument("Missing or empty '\(key)'")
        }
        return value
    }

    static func optionalString(_ map: [String: Any], _ key: String) -> String? {
        guard let value = map[key] as? String, !value.isEmpty else { return nil }
        return value
    }

    static func optionalBool(_ map: [String: Any], _ key: String) -> Bool? {
        (map[key] as? NSNumber)?.boolValue
    }

    static func requireBool(_ map: [String: Any], _ key: String) throws -> Bool {
        guard let value = optionalBool(map, key) else {
            throw HostError.invalidArgument("Missing boolean '\(key)'")
        }
        return value
    }

    static func requireInt(_ map: [String: Any], _ key: String) throws -> Int {
        guard let number = map[key] as? NSNumber else {
            throw HostError.invalidArgument("Missing int '\(key)'")
        }
        return try int(number, key)
    }

    static func optionalInt(_ map: [String: Any], _ key: String) throws -> Int? {
        guard let value = map[key], !(value is NSNull) else { return nil }
        guard let number = value as? NSNumber else {
            throw HostError.invalidArgument("Invalid int '\(key)'")
        }
        return try int(number, key)
    }

    static func requireDouble(_ map: [String: Any], _ key: String) throws -> Double {
        guard let number = map[key] as? NSNumber else {
            throw HostError.invalidArgument("Missing number '\(key)'")
        }
        return number.doubleValue
    }

    /// An unsigned 32-bit value from a Dart int or double.
    static func requireUInt32(_ map: [String: Any], _ key: String) throws -> UInt32 {
        try ProtocolEngineHost.uint32(try requireDouble(map, key), key)
    }

    /// An unsigned 64-bit value from a Dart int or double. Refused when
    /// negative, non-finite or above 2^63: none of those can be a size or a
    /// count, and narrowing one would trap.
    static func requireUInt64(_ map: [String: Any], _ key: String) throws -> UInt64 {
        let value = try requireDouble(map, key)
        guard value.isFinite, value >= 0, value < 9_223_372_036_854_775_808.0 else {
            throw HostError.invalidArgument("'\(key)' must be a non-negative number")
        }
        return UInt64(value)
    }

    /// Absent is an empty list; present but not a list is an error.
    static func stringList(_ map: [String: Any], _ key: String) throws -> [String] {
        guard let raw = map[key], !(raw is NSNull) else { return [] }
        if let list = raw as? [Any] {
            return list.compactMap { $0 as? String }
        }
        throw HostError.invalidArgument("'\(key)' must be a list of strings")
    }

    /// Absent is an empty list. Accepts a list of ints (what Dart's
    /// `List<int>` arrives as) or a `Uint8List`.
    static func byteList(_ map: [String: Any], _ key: String) throws -> [UInt8] {
        guard let raw = map[key], !(raw is NSNull) else { return [] }
        return try bytes(raw, key)
    }

    static func requireByteList(_ map: [String: Any], _ key: String) throws -> [UInt8] {
        guard let raw = map[key], !(raw is NSNull) else {
            throw HostError.invalidArgument("Missing '\(key)'")
        }
        return try bytes(raw, key)
    }

    static func requireBase64(_ map: [String: Any], _ key: String) throws -> [UInt8] {
        [UInt8](try ProtocolEngineHost.base64Data(try requireString(map, key), key))
    }

    /// Bytes go back to Dart as a list of ints, as Android's `map { it.toInt() }`
    /// does. A `[UInt8]` handed to the codec would depend on how it bridges.
    static func ints(_ bytes: [UInt8]) -> [Int] {
        bytes.map { Int($0) }
    }

    /// A string-keyed JSON object, parsed from a string or taken as a map.
    static func jsonObject(_ text: String, _ what: String) throws -> [String: Any] {
        guard let data = text.data(using: .utf8),
              let object = (try? JSONSerialization.jsonObject(with: data)) as? [String: Any] else {
            throw HostError.invalidArgument("Invalid JSON for \(what)")
        }
        return object
    }

    private static func int(_ number: NSNumber, _ key: String) throws -> Int {
        let value = number.doubleValue
        guard value.isFinite, value >= Double(Int.min), value <= Double(Int.max) else {
            throw HostError.invalidArgument("Invalid int '\(key)'")
        }
        return number.intValue
    }

    private static func bytes(_ raw: Any, _ key: String) throws -> [UInt8] {
        #if canImport(Flutter)
        if let typed = raw as? FlutterStandardTypedData {
            return [UInt8](typed.data)
        }
        #endif
        if let list = raw as? [Any] {
            return try list.map { element in
                guard let number = element as? NSNumber else {
                    throw HostError.invalidArgument("Invalid byte in '\(key)'")
                }
                // Truncating, as Kotlin's `toUByte()` is: a byte list is a
                // list of ints that fit in eight bits.
                return UInt8(truncatingIfNeeded: number.intValue)
            }
        }
        throw HostError.invalidArgument("'\(key)' must be a byte list")
    }
}
