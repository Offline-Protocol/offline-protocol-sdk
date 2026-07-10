//
// RelayRateLimiter.swift
// OfflineProtocol
//
// Client-side mirror of the relay's per-connection token bucket (30 burst,
// 10 refill/s). Every relay-bound frame takes a token before the socket
// write: URLSessionWebSocketTask's send() only proves a local write, so
// without this a poll batch or a large registration's member deltas can
// burst past the server bucket — the relay drops the overflow *after* the
// local write "succeeded", which would let the bridge confirm (and the
// translator commit) state the relay never recorded.
//
// Capacity/refill sit slightly under the relay's documented budget so a
// frame this limiter doesn't meter (authentication) never tips the server
// bucket. A `false` from tryAcquire always means "defer the frame to a
// later tick", never "drop it".
//
// Callers pass `nowMs` explicitly (testability). Thread-safe. Mirrors
// android RelayRateLimiter.kt — keep in sync.
//

import Foundation

final class RelayRateLimiter {

    /// Relay burst budget is 30; keep headroom for unmetered frames.
    static let defaultCapacity = 28

    /// Relay refill is 10/s; same headroom.
    static let defaultRefillPerSecond = 9.0

    private let capacity: Int
    private let refillPerSecond: Double
    private let lock = NSLock()
    private var tokens: Double
    private var lastRefillAtMs: Int64 = 0

    init(capacity: Int = RelayRateLimiter.defaultCapacity,
         refillPerSecond: Double = RelayRateLimiter.defaultRefillPerSecond) {
        self.capacity = capacity
        self.refillPerSecond = refillPerSecond
        self.tokens = Double(capacity)
    }

    /// Takes one token, refilling first.
    func tryAcquire(nowMs: Int64) -> Bool {
        lock.lock()
        defer { lock.unlock() }
        refillLocked(nowMs: nowMs)
        if tokens >= 1.0 {
            tokens -= 1.0
            return true
        }
        return false
    }

    /// Returns a token acquired for a frame that was never written.
    func refund() {
        lock.lock()
        defer { lock.unlock() }
        tokens = min(Double(capacity), tokens + 1.0)
    }

    /// Empties the bucket. Called when the relay answers `RateLimited`: the
    /// local mirror was too optimistic, so force a full refill interval of
    /// quiet before the next frame.
    func drain(nowMs: Int64) {
        lock.lock()
        defer { lock.unlock() }
        refillLocked(nowMs: nowMs)
        tokens = 0.0
    }

    private func refillLocked(nowMs: Int64) {
        if lastRefillAtMs == 0 {
            lastRefillAtMs = nowMs
            return
        }
        let elapsedMs = nowMs - lastRefillAtMs
        if elapsedMs < 0 {
            // Clock went backwards (wall-clock step, or a caller switching
            // time sources). No minting — but the baseline must resync, or
            // refill stays frozen until the clock re-passes the old mark and
            // the whole outbound path is silenced for the step duration.
            lastRefillAtMs = nowMs
            return
        }
        if elapsedMs == 0 { return }
        tokens = min(Double(capacity), tokens + Double(elapsedMs) / 1000.0 * refillPerSecond)
        lastRefillAtMs = nowMs
    }
}
