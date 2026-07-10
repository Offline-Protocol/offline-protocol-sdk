//
// RecipientInFlightTracker.swift
// OfflineProtocol
//
// Tracks wire-level in-flight message ids per recipient so the relay's
// recipient-keyed failure signals (DeliveryError / ConnectionRequestError —
// neither carries a message_id) can be correlated back to the SDK message
// ids still awaiting an outcome.
//
// Entries are plane-tagged: DeliveryError answers only data-plane frames
// (SendMessage) and ConnectionRequestError only connection-request ops, so
// each signal must drain only its own plane — a DeliveryError sweeping a
// conn_req (or vice versa) would fail a message the relay never reported on.
// Group-scoped ops (CreateGroup / SendGroupMessage / LeaveGroup) are never
// recorded: their error channel is the group-scoped GroupError, not these
// recipient-keyed signals.
//
// Recipient-keyed correlation is the best available and safe by
// construction: everything in flight on that plane to an offline peer failed.
//
// Mirrors android/.../RecipientInFlightTracker.kt — keep the two in sync.
//

import Foundation

final class RecipientInFlightTracker {
    static let defaultTtlMs: Int64 = 60_000
    static let defaultMaxPerRecipient = 32

    /// Which relay failure signal can answer the entry.
    enum Plane {
        /// Normal traffic (SendMessage) — answered by MessageSent/DeliveryError.
        case data
        /// conn_req/conn_acc/conn_rej/conn_can primaries — answered by
        /// ConnectionRequestError.
        case connReq
    }

    private struct InFlight {
        let messageId: String
        let plane: Plane
        let sentAtMs: Int64
    }

    private let ttlMs: Int64
    private let maxPerRecipient: Int
    private var byRecipient: [String: [InFlight]] = [:]
    private let lock = NSLock()

    init(ttlMs: Int64 = RecipientInFlightTracker.defaultTtlMs,
         maxPerRecipient: Int = RecipientInFlightTracker.defaultMaxPerRecipient) {
        self.ttlMs = ttlMs
        self.maxPerRecipient = maxPerRecipient
    }

    /// Records a wire send; entries beyond the per-recipient cap drop oldest-first.
    /// Callers record BEFORE the socket write so a fast relay answer can never
    /// outrun the entry; a failed write must `unrecord` its exact entry.
    func recordSent(recipient: String, messageId: String, plane: Plane, nowMs: Int64) {
        guard !recipient.isEmpty, !messageId.isEmpty else { return }
        lock.lock()
        defer { lock.unlock() }
        var queue = byRecipient[recipient] ?? []
        queue.append(InFlight(messageId: messageId, plane: plane, sentAtMs: nowMs))
        if queue.count > maxPerRecipient {
            queue.removeFirst(queue.count - maxPerRecipient)
        }
        byRecipient[recipient] = queue
    }

    /// Removes the exact entry a failed socket write recorded optimistically —
    /// the failure path owns the outcome (internetSendFailed), so the entry
    /// must not linger to be double-failed by a later recipient-keyed signal.
    func unrecord(recipient: String, messageId: String) {
        guard !recipient.isEmpty, !messageId.isEmpty else { return }
        lock.lock()
        defer { lock.unlock() }
        guard var queue = byRecipient[recipient] else { return }
        if let index = queue.firstIndex(where: { $0.messageId == messageId }) {
            queue.remove(at: index)
        }
        if queue.isEmpty {
            byRecipient.removeValue(forKey: recipient)
        } else {
            byRecipient[recipient] = queue
        }
    }

    /// Resolves one DATA in-flight entry on the relay's `MessageSent` answer:
    /// the relay accepted and forwarded that frame, so it must not be swept
    /// into a later recipient-keyed `DeliveryError` (which would false-fail a
    /// delivered message — for a welcome, parking a lifecycle the peer
    /// actually received). Removes the exact `messageId` when the relay
    /// echoed ours; otherwise removes the oldest DATA entry for the recipient —
    /// sends per recipient are FIFO on one socket and the relay answers in
    /// order, so oldest-first is the sound correlation. CONN_REQ entries are
    /// never touched: `MessageSent` only answers SendMessage frames.
    func resolveOnRelayAccepted(recipient: String, messageId: String?, nowMs: Int64) {
        guard !recipient.isEmpty else { return }
        lock.lock()
        defer { lock.unlock() }
        guard var queue = byRecipient[recipient] else { return }
        queue.removeAll { nowMs - $0.sentAtMs > ttlMs }
        if let messageId = messageId,
           queue.contains(where: { $0.plane == .data && $0.messageId == messageId }) {
            queue.removeAll { $0.plane == .data && $0.messageId == messageId }
        } else if let oldestData = queue.firstIndex(where: { $0.plane == .data }) {
            queue.remove(at: oldestData)
        }
        if queue.isEmpty {
            byRecipient.removeValue(forKey: recipient)
        } else {
            byRecipient[recipient] = queue
        }
    }

    /// Removes and returns every live (non-expired) in-flight id for a
    /// recipient on the given plane; the other plane's entries stay tracked.
    func drainRecipient(_ recipient: String, plane: Plane, nowMs: Int64) -> [String] {
        lock.lock()
        defer { lock.unlock() }
        guard var queue = byRecipient[recipient] else { return [] }
        let drained = queue.filter { $0.plane == plane && nowMs - $0.sentAtMs <= ttlMs }
        queue.removeAll { $0.plane == plane }
        if queue.isEmpty {
            byRecipient.removeValue(forKey: recipient)
        } else {
            byRecipient[recipient] = queue
        }
        return drained.map { $0.messageId }
    }

    /// Drops entries older than the TTL (regardless of plane); called from
    /// the poll tick.
    func prune(nowMs: Int64) {
        lock.lock()
        defer { lock.unlock() }
        for (recipient, queue) in byRecipient {
            let live = queue.filter { nowMs - $0.sentAtMs <= ttlMs }
            if live.isEmpty {
                byRecipient.removeValue(forKey: recipient)
            } else {
                byRecipient[recipient] = live
            }
        }
    }

    /// Forgets everything — the socket died and the transport layer owns the outcome.
    func clear() {
        lock.lock()
        defer { lock.unlock() }
        byRecipient.removeAll()
    }
}
