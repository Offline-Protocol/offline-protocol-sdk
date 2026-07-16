//
// PresenceWatchPolicy.swift
// OfflineProtocol
//
// Decides which peers to query for relay presence (CheckPresence) each tick.
//
// Watch sources: recipients the relay reported unreachable (DeliveryError)
// plus the core watchlist (peers with undelivered or
// session-unproven MLS welcomes) merged at tick time. A peer leaves the set
// on an online presence answer, on inbound traffic from the peer, or after
// the idle TTL.
//
// Queries rotate round-robin so a large watch set is fully covered across
// ticks while staying far under the relay's per-connection rate limit
// (token bucket: burst 30, 10/s).
//

import Foundation

final class PresenceWatchPolicy {
    static let defaultIdleTtlMs: Int64 = 10 * 60_000
    static let defaultMaxQueriesPerTick = 10
    static let defaultTickInterval: TimeInterval = 20.0

    private let idleTtlMs: Int64
    private let maxQueriesPerTick: Int
    private var lastRelevantAtMs: [String: Int64] = [:]
    private var rotation: [String] = []
    private let lock = NSLock()

    init(idleTtlMs: Int64 = PresenceWatchPolicy.defaultIdleTtlMs,
         maxQueriesPerTick: Int = PresenceWatchPolicy.defaultMaxQueriesPerTick) {
        self.idleTtlMs = idleTtlMs
        self.maxQueriesPerTick = maxQueriesPerTick
    }

    /// Adds a peer to the watch set (or refreshes its idle clock).
    func watch(_ peerId: String, nowMs: Int64) {
        guard !peerId.isEmpty else { return }
        lock.lock()
        defer { lock.unlock() }
        watchLocked(peerId, nowMs: nowMs)
    }

    /// Removes a peer (online answer or inbound traffic proved reachability).
    func unwatch(_ peerId: String) {
        lock.lock()
        defer { lock.unlock() }
        unwatchLocked(peerId)
    }

    func watchedPeers() -> Set<String> {
        lock.lock()
        defer { lock.unlock() }
        return Set(lastRelevantAtMs.keys)
    }

    /// Merges the core watchlist (authoritatively still-pending peers refresh
    /// their idle clock), evicts idle entries, and returns up to
    /// `maxQueriesPerTick` peers to query this tick, round-robin.
    func peersToQuery(coreWatchlist: [String], nowMs: Int64) -> [String] {
        lock.lock()
        defer { lock.unlock() }
        for peer in coreWatchlist where !peer.isEmpty {
            watchLocked(peer, nowMs: nowMs)
        }
        let expired = lastRelevantAtMs.filter { nowMs - $0.value > idleTtlMs }.map { $0.key }
        for peer in expired {
            unwatchLocked(peer)
        }

        guard !rotation.isEmpty else { return [] }
        let count = min(maxQueriesPerTick, rotation.count)
        var result: [String] = []
        result.reserveCapacity(count)
        for _ in 0..<count {
            let peer = rotation.removeFirst()
            rotation.append(peer)
            result.append(peer)
        }
        return result
    }

    func clear() {
        lock.lock()
        defer { lock.unlock() }
        lastRelevantAtMs.removeAll()
        rotation.removeAll()
    }

    private func watchLocked(_ peerId: String, nowMs: Int64) {
        if lastRelevantAtMs.updateValue(nowMs, forKey: peerId) == nil {
            rotation.append(peerId)
        }
    }

    private func unwatchLocked(_ peerId: String) {
        if lastRelevantAtMs.removeValue(forKey: peerId) != nil {
            rotation.removeAll { $0 == peerId }
        }
    }
}
