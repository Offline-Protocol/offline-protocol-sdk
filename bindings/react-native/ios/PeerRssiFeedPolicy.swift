import Foundation

/// Which scan results feed a signal reading to the core. Mirrors the feed
/// half of android/.../PeerRssiFeedPolicy.kt; keep in sync. (iOS cannot
/// advertise service data, so it has no scan-response half.)
///
/// Every failure here is silent: a throttle that never admits looks exactly
/// like no peer in range.
internal enum PeerRssiFeedPolicy {
    /// A little over the core's 250 ms event cap. The two are measured on
    /// different clocks with a queue hop between them, so an equal interval
    /// let the core see a gap just under 250 ms, drop the reading, and halve
    /// the rate.
    static let interval: TimeInterval = 0.26

    private static let maxTracked = 100

    /// Whether a scan result for `id` at `now` goes to the core, recording it
    /// if so. A result with no link and no service data never goes: the core
    /// could not attribute it, and the call would only take its locks.
    static func admit(_ lastFedAt: inout [UUID: Date], id: UUID, now: Date, linked: Bool, hasServiceData: Bool) -> Bool {
        guard linked || hasServiceData else { return false }
        if let last = lastFedAt[id], now.timeIntervalSince(last) < interval { return false }
        // ponytail: cleared rather than aged out; costs one early reading.
        if lastFedAt.count >= maxTracked { lastFedAt.removeAll() }
        lastFedAt[id] = now
        return true
    }
}
