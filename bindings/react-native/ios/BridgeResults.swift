import Foundation

/// Converts [ProtocolEngineHostBridge] return values to React Native promise payloads.
///
/// Each method resolves the shape it resolved before the shared bridge existed,
/// because `src/index.ts` was written against it. A `String` is resolved as the
/// string, whatever it contains: `getTopology` and `getMessageStats` return JSON
/// that `src/index.ts` passes to `JSON.parse`, and `JSON.parse` on an object
/// throws. `parseJsonResult` is the one way to turn a JSON string into an
/// object, and the generated adapter calls it only for methods the manifest marks
/// `parseJsonResult`. Mirrors android/ BridgeResults.kt.
enum BridgeResults {
    static func arrayFromNSNumberList(_ list: [Any]?) -> [Int]? {
        guard let list else { return nil }
        return list.compactMap { ($0 as? NSNumber)?.intValue }
    }

    static func argumentMap(from dict: [String: Any]?) -> [String: Any] {
        dict ?? [:]
    }

    static func toReactNative(_ value: Any?) -> Any? {
        switch value {
        case nil, is NSNull:
            return nil
        case let string as String:
            return string
        case let number as NSNumber:
            return number
        case let map as [String: Any]:
            return map
        case let list as [Any]:
            return list
        default:
            return String(describing: value)
        }
    }

    /// Resolves a JSON object or array string as the object or array, and any
    /// other value as `toReactNative` does. A string that is not valid JSON is
    /// resolved as the string, so a malformed result reaches JavaScript rather
    /// than failing the call.
    static func parseJsonResult(_ value: Any?) -> Any? {
        if let string = value as? String {
            let trimmed = string.drop { $0.isWhitespace }
            if trimmed.hasPrefix("{") || trimmed.hasPrefix("["),
               let data = string.data(using: .utf8),
               let json = try? JSONSerialization.jsonObject(with: data) {
                return json
            }
        }
        return toReactNative(value)
    }
}
