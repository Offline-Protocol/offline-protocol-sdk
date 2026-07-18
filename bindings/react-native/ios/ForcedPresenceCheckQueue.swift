//
// ForcedPresenceCheckQueue.swift
// Mirrors android/src/main/java/com/offlineprotocol/ForcedPresenceCheckQueue.kt — keep in sync.
//

import Foundation

/// Parked forced presence checks (`checkPresence(force: true)`): the
/// park / expire / fail-fast / drain policy, kept dispatch-free so the
/// SwiftPM harness can test it — the DispatchQueue shell (the queue hop,
/// the retry tick) stays in InternetManager.
///
/// Not thread-safe: the owner confines every call to one queue (the
/// bridge's messageQueue). Callers pass `nowMs` explicitly (testability),
/// from the same monotonic source as the entry's deadline. Each entry's
/// completion fires exactly once: on every non-park decision here, or
/// after the owner re-attempts an entry returned by `takeAll()`, or on
/// `drainAll()`.
final class ForcedPresenceCheckQueue {

    /// Parked entries are app-level promises awaiting an ~8s deadline;
    /// more than this many concurrent forced checks means the app is
    /// calling in a loop. New checks are rejected (resolved false) at
    /// capacity instead of growing the queue without bound — existing
    /// entries keep their earlier deadlines and are never evicted.
    static let defaultCapacity = 32

    struct Entry {
        let userId: String
        let deadlineMs: Int64
        let completion: (Bool) -> Void
    }

    private let capacity: Int
    private var parked: [Entry] = []

    init(capacity: Int = ForcedPresenceCheckQueue.defaultCapacity) {
        self.capacity = capacity
    }

    var isEmpty: Bool { parked.isEmpty }

    /// Decides an unsendable check's fate: fail fast on a stopping/stopped
    /// transport (no reconnect is coming), expire at/past its deadline,
    /// reject at capacity, otherwise park. Every non-park outcome resolves
    /// the completion (false) before returning. Returns true iff the entry
    /// parked — the owner must then ensure a retry tick is scheduled.
    func parkOrExpire(_ entry: Entry, transportStopped: Bool, nowMs: Int64) -> Bool {
        if transportStopped {
            entry.completion(false)
            return false
        }
        if nowMs >= entry.deadlineMs {
            entry.completion(false)
            return false
        }
        if parked.count >= capacity {
            entry.completion(false)
            return false
        }
        parked.append(entry)
        return true
    }

    /// Removes and returns every parked entry, oldest first. The owner
    /// re-attempts each (a retry tick or the authenticated edge);
    /// still-unsendable entries come back via `parkOrExpire`.
    func takeAll() -> [Entry] {
        if parked.isEmpty { return [] }
        let all = parked
        parked.removeAll()
        return all
    }

    /// Resolves every parked entry false (explicit transport stop).
    func drainAll() {
        for entry in takeAll() {
            entry.completion(false)
        }
    }
}
