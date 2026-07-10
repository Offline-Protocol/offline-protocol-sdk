//
// RelayTimestamps.swift
// OfflineProtocol
//
// Parses the relay's last-seen timestamps to Unix ms.
//
// Accepts ISO-8601 or epoch milliseconds (the relay has sent both shapes);
// returns nil when absent or unparseable — a last-seen display must not
// invent a timestamp. Mirrors android/.../RelayTimestamps.kt — keep in sync.
//

import Foundation

enum RelayTimestamps {
    static func parseToMsOrNull(_ timestampStr: String) -> Int64? {
        guard !timestampStr.isEmpty else { return nil }
        if let epochMs = Int64(timestampStr) {
            return epochMs
        }
        let formatter = ISO8601DateFormatter()
        formatter.formatOptions = [.withInternetDateTime, .withFractionalSeconds]
        if let date = formatter.date(from: timestampStr) {
            return Int64(date.timeIntervalSince1970 * 1000)
        }
        formatter.formatOptions = [.withInternetDateTime]
        if let date = formatter.date(from: timestampStr) {
            return Int64(date.timeIntervalSince1970 * 1000)
        }
        return nil
    }
}
