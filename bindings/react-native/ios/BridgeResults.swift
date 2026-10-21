import Foundation

/// Converts [ProtocolEngineHostBridge] return values to React Native promise payloads.
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
            if string.hasPrefix("{") || string.hasPrefix("["),
               let data = string.data(using: .utf8),
               let json = try? JSONSerialization.jsonObject(with: data) {
                return json
            }
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
}
