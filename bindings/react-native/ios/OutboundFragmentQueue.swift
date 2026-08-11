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
/// two in sync. See the overflow note below for the one place they currently
/// disagree.
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
/// `enqueue` drops the **oldest** fragments when the per-recipient cap is
/// exceeded. This is preserved verbatim from the pre-extraction inline
/// implementation, and it is a known divergence from Android, which discards
/// the whole per-recipient queue instead: fragments are slices of a single
/// application message, so evicting slice 0 of a five-slice message leaves
/// four orphan slices that reassemble into garbage at the receiver. The
/// inbound side (`InboundFragmentBuffer`) already does the whole-buffer drop.
/// Converging the outbound side is a behaviour change and is deliberately
/// **not** bundled into the OFF-2123 threading fix — tracked separately.
final class OutboundFragmentQueue: @unchecked Sendable {

    enum DropReason {
        case capped
        case expired
    }

    private struct Entry {
        let data: Data
        let timestamp: Date
    }

    private let lock = NSLock()
    private var queues: [String: [Entry]] = [:]

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

    // MARK: - Owning-queue mutations

    /// Append a fragment for `recipientId`, dropping the oldest entries if the
    /// per-recipient cap is exceeded (see the overflow note above).
    func enqueue(_ recipientId: String, _ data: Data) {
        queueCheck()
        var dropped = 0
        lock.lock()
        // Mutate through the subscript rather than via a local copy: binding
        // the array to a `var` takes a second reference, so every append would
        // deep-copy a queue holding up to `maxPerPeer` fragments.
        queues[recipientId, default: []].append(Entry(data: data, timestamp: clock()))
        let count = queues[recipientId]?.count ?? 0
        if count > maxPerPeer {
            dropped = count - maxPerPeer
            queues[recipientId]?.removeFirst(dropped)
        }
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
    /// Expiry is per-fragment, not per-message: these are opaque fragment
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
            // Take the whole queue out under the lock so `send` — a
            // CoreBluetooth write — never runs with the lock held.
            lock.lock()
            guard var queue = queues.removeValue(forKey: recipientId) else {
                lock.unlock()
                continue
            }
            lock.unlock()

            let beforeExpiry = queue.count
            queue.removeAll { now.timeIntervalSince($0.timestamp) >= timeout }
            let wholeQueueExpired = queue.isEmpty && beforeExpiry > 0

            var sentAll = true
            while let first = queue.first {
                if send(recipientId, first.data) {
                    queue.removeFirst()
                } else {
                    sentAll = false
                    break
                }
            }

            if !queue.isEmpty {
                lock.lock()
                // Anything enqueued while we were sending must stay behind the
                // older fragments being put back, or the stream reorders.
                queues[recipientId] = queue + (queues[recipientId] ?? [])
                lock.unlock()
                if !sentAll {
                    hasUnsent = true
                }
            }

            if wholeQueueExpired {
                onDropped(recipientId, .expired, beforeExpiry)
            }
        }

        return hasUnsent
    }

    static let defaultMaxPerPeer = 100
    static let defaultTimeout: TimeInterval = 30.0
}
