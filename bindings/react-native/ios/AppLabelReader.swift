import Foundation

/// Reads the app label from a config: `appLabel` since 0.35, with `appId`
/// and `app_id`, its old names, still read.
///
/// Two of them with different values is refused rather than one silently
/// winning: either could be the one the app meant, and a wrong pick moves it
/// to another storage namespace and a new identity. Foundation-only so the
/// SwiftPM suite covers it; mirrors `ProtocolConfigParser.appLabel` on
/// Android, keep the two in sync.
enum AppLabelReader {
    static func read(_ raw: [String: Any]) throws -> String {
        let present = ["appLabel", "appId", "app_id"].compactMap { key in
            (raw[key] as? String).map { (key, $0) }
        }
        guard let first = present.first else { return "" }
        if let other = present.first(where: { $0.1 != first.1 }) {
            throw NSError(
                domain: "OfflineProtocol",
                code: -1,
                userInfo: [NSLocalizedDescriptionKey:
                    "\(first.0) \"\(first.1)\" and \(other.0) \"\(other.1)\" disagree; " +
                    "appId is the old name of appLabel, set only appLabel"]
            )
        }
        return first.1
    }
}
