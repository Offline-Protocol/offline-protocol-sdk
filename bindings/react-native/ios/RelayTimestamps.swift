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
    /// Below this an epoch can only be seconds (1e11 ms is March 1973, 1e11 s
    /// is year ~5100); above it, milliseconds. Without the split, a
    /// seconds-shaped `last_seen` renders as January 1970.
    private static let epochSecondsCutoff: Int64 = 100_000_000_000

    static func parseToMsOrNull(_ timestampStr: String) -> Int64? {
        guard !timestampStr.isEmpty else { return nil }
        if let epoch = Int64(timestampStr) {
            return normalizeEpochToMs(epoch)
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

    /// Normalizes a numeric epoch (seconds or milliseconds) to milliseconds.
    static func normalizeEpochToMs(_ value: Int64) -> Int64 {
        return (value >= 1 && value < epochSecondsCutoff) ? value * 1000 : value
    }
}
