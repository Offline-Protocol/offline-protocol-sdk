//
// OutboundFragmentQueue.swift
// OfflineProtocol
//

import Foundation

/// FIFO buffer of outbound BLE fragments that could not be sent immediately —
/// either because the recipient has no open connection, or because the
/// previous write hit flow control.
///
/// This is the iOS mirror of Android's `OutboundFragmentQueue.kt` — keep the
/// two in sync.
///
/// ### Thread-safety contract
///
/// Operations that are **compound** — `enqueue` and `flush` — must run on the
/// owning queue (`BleManager.fragmentQueue`), enforced at runtime via
/// `queueCheck`; that serial queue is what keeps a recipient's stream ordered.
/// `removeAll` and `clear` are exempt and safe from **any** thread: they are
/// single indivisible operations, and peer eviction and transport teardown
/// both run on the main queue, where a hop onto the owning queue would mean
/// the very `dispatch_sync` this class exists to delete.
///
/// One instance departs from that: `BleManager.notifyFragments`, the NOTIFY
/// egress queue, is enqueued on `fragmentQueue` and flushed on main, because
/// `CBPeripheralManager.updateValue` must run on main while the drain has to
/// count what it just enqueued. It passes no `queueCheck`. The split holds
/// because main is its only flusher and `flush` never takes a fragment out of
/// the queue before it has been sent: a concurrent `enqueue` appends behind
/// it, so the stream stays in order, and `isBackedUp` and the cap count every
/// fragment still waiting. A flush that took the queue out to send it would
/// hide those fragments from both, the drain would pull past the mark, and
/// the next `enqueue` would discard the lot.
///
/// The `NSLock` exists so **readers on other threads** — chiefly the metrics
/// refresher — can take a snapshot without `dispatch_sync`-ing onto the
/// owning queue, which is what made main-thread readers inherit the latency
/// of whatever UniFFI call that queue happened to be inside (OFF-2123).
///
/// The lock is never held across `send` or `onDropped`: both re-enter
/// `BleManager` (a CoreBluetooth write, a diagnostic delegate hop), and
/// holding a lock across those is how a mutex becomes a hang.
///
/// ### Overflow policy
///
/// When `enqueue` would push the per-recipient queue past `maxPerPeer`, the
/// entire queue for that recipient is discarded before the new fragment is
/// appended — the same policy as Android and as the inbound side
/// (`InboundFragmentBuffer`). Dropping just the oldest fragments (the previous
/// iOS policy) was unsafe: fragments are slices of a single application
/// message, so evicting slice 0 of a five-slice message leaves four orphan
/// slices that reassemble into garbage at the receiver. The fragments are
/// opaque at this layer — no message ids come down from Rust — so the queue
/// cannot cut at a message boundary; the whole queue is the only cut that
/// never splits one. We lose messages that hadn't started delivery yet, and
/// the sender's higher layer (ack-driven retry, data-sync anti-entropy)
/// re-drives them.
///
/// Overflow should be rare: `isBackedUp` is the drain loop's signal to stop
/// pulling from the Rust core well before the cap is reached.
final class OutboundFragmentQueue: @unchecked Sendable {

    enum DropReason {
        case capped
        case expired
    }

    private struct Entry {
        let data: Data
        let timestamp: Date
        /// Identifies this entry across an unlocked `send`, so `flush` removes
        /// the fragment it sent and never one that replaced it after a
        /// `removeAll` or an overflow discard.
        let seq: UInt64
    }

    private let lock = NSLock()
    private var queues: [String: [Entry]] = [:]
    private var nextSeq: UInt64 = 0

    private let queueCheck: () -> Void
    private let maxPerPeer: Int
    private let timeout: TimeInterval
    private let clock: () -> Date
    private let onDropped: (String, DropReason, Int) -> Void

    init(
        queueCheck: @escaping () -> Void = {},
        maxPerPeer: Int = OutboundFragmentQueue.defaultMaxPerPeer,
        timeout: TimeInterval = OutboundFragmentQueue.defaultTimeout,
        clock: @escaping () -> Date = Date.init,
        onDropped: @escaping (String, DropReason, Int) -> Void = { _, _, _ in }
    ) {
        self.queueCheck = queueCheck
        self.maxPerPeer = maxPerPeer
        self.timeout = timeout
        self.clock = clock
        self.onDropped = onDropped
    }

    // MARK: - Cross-thread reads (no queue affinity)

    /// Aggregate fragment count across all recipients. Safe from any thread.
    func totalCount() -> Int {
        lock.lock()
        defer { lock.unlock() }
        return queues.values.reduce(0) { $0 + $1.count }
    }

    /// Snapshot of recipients with outstanding fragments. Safe from any thread.
    func recipientIds() -> [String] {
        lock.lock()
        defer { lock.unlock() }
        return Array(queues.keys)
    }

    /// True if `recipientId` has at least one fragment already waiting.
    /// Safe from any thread.
    func hasPending(_ recipientId: String) -> Bool {
        lock.lock()
        defer { lock.unlock() }
        return !(queues[recipientId]?.isEmpty ?? true)
    }

    /// True once `recipientId`'s queue has filled past a high-water mark
    /// (3/4 of `maxPerPeer`). The drain loop uses this as a backpressure
    /// signal to STOP pulling more fragments out of the Rust core — which is
    /// a destructive pop — into this bounded queue. Without it, when
    /// CoreBluetooth's write buffer paces sends slower than the loop can pull,
    /// the loop spins the whole Rust backlog into the queue, overflows
    /// `maxPerPeer`, and `enqueue` discards the queue mid-message. Holding the
    /// backlog in Rust (the proper unbounded, ordered buffer) instead keeps
    /// delivery lossless; the next drain resumes pulling once `flush` has
    /// drained the queue back below the mark. Mirrors Android's `isBackedUp`.
    /// Safe from any thread.
    func isBackedUp(_ recipientId: String) -> Bool {
        lock.lock()
        defer { lock.unlock() }
        return (queues[recipientId]?.count ?? 0) >= maxPerPeer * 3 / 4
    }

    // MARK: - Owning-queue mutations

    /// Append a fragment for `recipientId`. If doing so would exceed
    /// `maxPerPeer` the entire per-recipient queue is discarded first (see the
    /// overflow policy above); `onDropped` is invoked once with the number of
    /// fragments evicted, then the new fragment starts a fresh queue.
    func enqueue(_ recipientId: String, _ data: Data) {
        queueCheck()
        var dropped = 0
        lock.lock()
        if let count = queues[recipientId]?.count, count >= maxPerPeer {
            dropped = count
            queues[recipientId] = nil
        }
        // Mutate through the subscript rather than via a local copy: binding
        // the array to a `var` takes a second reference, so every append would
        // deep-copy a queue holding up to `maxPerPeer` fragments.
        nextSeq &+= 1
        queues[recipientId, default: []].append(Entry(data: data, timestamp: clock(), seq: nextSeq))
        lock.unlock()

        if dropped > 0 {
            onDropped(recipientId, .capped, dropped)
        }
    }

    /// Drop every fragment queued for `recipientId` — used when the peer has
    /// been evicted and the bytes can never be delivered. Returns the count
    /// dropped so callers can surface a diagnostic.
    /// Safe from any thread — see the exemption in the class-level contract.
    @discardableResult
    func removeAll(_ recipientId: String) -> Int {
        lock.lock()
        defer { lock.unlock() }
        return queues.removeValue(forKey: recipientId)?.count ?? 0
    }

    /// Drop every queue. Used by transport stop. Safe from any thread.
    func clear() {
        lock.lock()
        queues.removeAll()
        lock.unlock()
    }

    /// Walk every recipient's queue, evicting entries older than `timeout` and
    /// then attempting to send the remainder in FIFO order via `send`. If
    /// `send` returns false the fragment is left in place (so the next flush
    /// retries it) and iteration for that recipient stops — but other
    /// recipients continue to drain.
    ///
    /// A fragment stays in the queue until `send` has accepted it: the head is
    /// read under the lock, sent without it, and only then removed, and only
    /// if it is still the head (a `removeAll` or an overflow discard on
    /// another thread may have replaced it meanwhile). So every count this
    /// class answers, `isBackedUp` and the cap in `enqueue` included, sees
    /// the fragments a flush is still working through.
    ///
    /// Expiry is per-fragment, not per-message, and is reported through
    /// `onDropped` whenever anything expires — these are opaque fragment
    /// bytes from the Rust fragmenter with no message grouping at this layer,
    /// so on a stall longer than `timeout` the early fragments of a
    /// multi-fragment message can be dropped while later ones survive, tearing
    /// it. Bounded, not silent: the receiver's idle reassembly times out the
    /// partial and the sender's higher layer (Welcome retransmit / ack-driven
    /// retry) re-sends the whole message.
    ///
    /// - Returns: true if at least one recipient still had unsent fragments
    ///   when the flush finished — the caller's "stalled writer" signal.
    @discardableResult
    func flush(send: (String, Data) -> Bool) -> Bool {
        queueCheck()
        var hasUnsent = false
        let now = clock()

        for recipientId in recipientIds() {
            lock.lock()
            var expired = 0
            if let queue = queues[recipientId] {
                let kept = queue.filter { now.timeIntervalSince($0.timestamp) < timeout }
                expired = queue.count - kept.count
                queues[recipientId] = kept.isEmpty ? nil : kept
            }
            lock.unlock()
            // Reported whenever anything expired, not only when the whole queue
            // did. A partial expiry is the case that actually tears a message
            // (see the note above), so it is the one worth seeing in telemetry.
            if expired > 0 {
                onDropped(recipientId, .expired, expired)
            }

            while true {
                lock.lock()
                let head = queues[recipientId]?.first
                lock.unlock()
                guard let head else { break }

                // The lock is not held here: `send` is a CoreBluetooth write.
                guard send(recipientId, head.data) else {
                    hasUnsent = true
                    break
                }

                lock.lock()
                // In place through the subscript, as in `enqueue`, so the
                // removal does not copy the array.
                if queues[recipientId]?.first?.seq == head.seq {
                    queues[recipientId]?.removeFirst()
                    if queues[recipientId]?.isEmpty == true {
                        queues[recipientId] = nil
                    }
                }
                lock.unlock()
            }
        }

        return hasUnsent
    }

    static let defaultMaxPerPeer = 100
    static let defaultTimeout: TimeInterval = 30.0
}
