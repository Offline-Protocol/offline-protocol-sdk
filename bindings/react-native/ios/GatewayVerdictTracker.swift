//
// GatewayVerdictTracker.swift
// OfflineProtocol
//
// Frames submitted to a gateway and not yet answered.
//

import Foundation

/// Tracks submitted frames until the gateway's verdict settles them.
///
/// The gateway answers every `SendMessage` with exactly one `MessageSent` or
/// `DeliveryError`, correlated by `message_id`. This holds the ids between the
/// two, so the manager can bound how many are outstanding, notice the ones a
/// gateway never answered, and settle every one of them when a connection
/// dies.
///
/// Three rules it exists to enforce, each of which was a real defect on some
/// implementation of this contract before it was written down:
///
/// 1. **An id already in flight is not sent again.** The core re-queues an
///    unconfirmed frame under the same id after its own acknowledgement
///    timeout, and a verdict can honestly take longer than that on a slow
///    backbone. Sending it twice forwards the frame twice and, when the second
///    copy times out, fails an id the gateway already confirmed.
/// 2. **Every id is settled exactly once.** `settle` returns whether this call
///    was the one that removed it, so a duplicate verdict cannot report a
///    second outcome for a frame the core has already moved on from.
/// 3. **Nothing is left waiting on a dead connection.** `drainAll` hands back
///    every outstanding id so the manager can fail them; a frame nobody
///    answers for costs the core its full 120-second expiry.
///
/// Thread-safe: the manager touches it from its send queue and from the socket
/// queue.
public final class GatewayVerdictTracker {

    private let lock = NSLock()
    private var inFlight: [String: TimeInterval] = [:]

    public init() {}

    /// Frames outstanding right now.
    public var count: Int {
        lock.lock()
        defer { lock.unlock() }
        return inFlight.count
    }

    /// Records `messageId` as submitted at `now`.
    ///
    /// Returns `false` when it was already in flight, in which case the caller
    /// must **not** send it again. Popping it from the core's outbox was
    /// enough: the attempt already outstanding is what settles that id, and
    /// the core's pending entry is refreshed by the pop.
    public func begin(_ messageId: String, now: TimeInterval) -> Bool {
        lock.lock()
        defer { lock.unlock() }
        if inFlight[messageId] != nil { return false }
        inFlight[messageId] = now
        return true
    }

    /// Settles `messageId`. Returns `false` if it was not outstanding, which
    /// is how a duplicate or unsolicited verdict is ignored.
    public func settle(_ messageId: String) -> Bool {
        lock.lock()
        defer { lock.unlock() }
        return inFlight.removeValue(forKey: messageId) != nil
    }

    /// Removes and returns every id submitted more than `timeout` ago.
    ///
    /// A gateway that answers nothing is a contract violation, but it is also
    /// indistinguishable from one whose socket is wedged, and the core cannot
    /// retry a frame nobody has failed. Removing them here is what turns
    /// silence back into a retry.
    public func expired(now: TimeInterval, timeout: TimeInterval) -> [String] {
        lock.lock()
        defer { lock.unlock() }
        let stale = inFlight.filter { now - $0.value > timeout }.map { $0.key }
        for id in stale { inFlight.removeValue(forKey: id) }
        return stale
    }

    /// Removes and returns everything outstanding, for a connection that is
    /// going away.
    public func drainAll() -> [String] {
        lock.lock()
        defer { lock.unlock() }
        let ids = Array(inFlight.keys)
        inFlight.removeAll()
        return ids
    }
}
