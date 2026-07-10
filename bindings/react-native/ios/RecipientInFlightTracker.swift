//
// RecipientInFlightTracker.swift
// OfflineProtocol
//
// Tracks wire-level in-flight message ids per recipient so the relay's
// recipient-keyed failure signals (DeliveryError / ConnectionRequestError —
// neither carries a message_id) can be correlated back to the SDK message
// ids still awaiting an outcome.
//
// Recipient-keyed correlation is the best available and safe by
// construction: everything in flight to an offline peer failed.
//

import Foundation

final class RecipientInFlightTracker {
    static let defaultTtlMs: Int64 = 60_000
    static let defaultMaxPerRecipient = 32

    private struct InFlight {
        let messageId: String
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
    func recordSent(recipient: String, messageId: String, nowMs: Int64) {
        guard !recipient.isEmpty, !messageId.isEmpty else { return }
        lock.lock()
        defer { lock.unlock() }
        var queue = byRecipient[recipient] ?? []
        queue.append(InFlight(messageId: messageId, sentAtMs: nowMs))
        if queue.count > maxPerRecipient {
            queue.removeFirst(queue.count - maxPerRecipient)
        }
        byRecipient[recipient] = queue
    }

    /// Resolves one in-flight entry on the relay's `MessageSent` answer: the
    /// relay accepted and forwarded that frame, so it must not be swept into
    /// a later recipient-keyed `DeliveryError` (which would false-fail a
    /// delivered message — for a welcome, parking a lifecycle the peer
    /// actually received). Removes the exact `messageId` when the relay
    /// echoed ours; otherwise removes the oldest entry for the recipient —
    /// sends per recipient are FIFO on one socket and the relay answers in
    /// order, so oldest-first is the sound correlation.
    func resolveOnRelayAccepted(recipient: String, messageId: String?, nowMs: Int64) {
        guard !recipient.isEmpty else { return }
        lock.lock()
        defer { lock.unlock() }
        guard var queue = byRecipient[recipient] else { return }
        queue.removeAll { nowMs - $0.sentAtMs > ttlMs }
        if !queue.isEmpty {
            if let messageId = messageId, queue.contains(where: { $0.messageId == messageId }) {
                queue.removeAll { $0.messageId == messageId }
            } else {
                queue.removeFirst()
            }
        }
        if queue.isEmpty {
            byRecipient.removeValue(forKey: recipient)
        } else {
            byRecipient[recipient] = queue
        }
    }

    /// Removes and returns every live (non-expired) in-flight id for a recipient.
    func drainRecipient(_ recipient: String, nowMs: Int64) -> [String] {
        lock.lock()
        defer { lock.unlock() }
        guard let queue = byRecipient.removeValue(forKey: recipient) else { return [] }
        return queue.filter { nowMs - $0.sentAtMs <= ttlMs }.map { $0.messageId }
    }

    /// Drops entries older than the TTL; called from the poll tick.
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
