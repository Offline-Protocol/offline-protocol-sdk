//
// InboundFragmentBuffer.swift
// OfflineProtocol
//

import Foundation

/// FIFO buffer for inbound BLE fragments that arrive before the sender's
/// stable device ID is known. Fragments are keyed by the connection's
/// CoreBluetooth identifier (stable for the lifetime of a single LL
/// connection, and therefore RPA-safe within that window). Once the reverse
/// GATT read resolves the device ID, `BleManager` calls `drain` to hand the
/// buffered bytes to the protocol in order.
///
/// This is the iOS mirror of Android's `InboundFragmentBuffer.kt` — keep the
/// two in sync.
///
/// ### Thread-safety contract
///
/// Operations that are **compound** — `enqueue`, `enqueueIfPending`, `drain`,
/// `evictExpired` — must run on the owning queue (`BleManager.fragmentQueue`).
/// Not for data integrity, which the lock already provides, but for the FIFO
/// ordering #59 established: the check-then-append in `enqueueIfPending` and
/// the remove-then-forward in `drain` are only ordered with respect to each
/// other because that queue is serial.
///
/// `removeAll` and `clear` are exempt and safe from **any** thread. They are
/// single, indivisible operations whose result does not depend on ordering
/// against an enqueue — the peer is going away either way. That exemption is
/// load-bearing: peer eviction and transport teardown both run on the main
/// queue, and forcing them onto the owning queue would mean a `dispatch_sync`,
/// which is precisely the main-thread stall this class exists to delete.
///
/// The `NSLock` exists for a different reason: so **readers on other threads**
/// — the main-queue connection monitor, the metrics refresher — can take a
/// snapshot without `dispatch_sync`-ing onto the owning queue. That `sync` was
/// the single largest source of main-thread hangs in OFF-2123: `fragmentQueue`
/// performs UniFFI calls that can block on the core protocol mutex for
/// seconds, and every main-thread reader inherited that latency. The lock is
/// never held across a UniFFI call — only across a dictionary operation.
///
/// The queue contract is enforced at runtime via `queueCheck`, the iOS
/// analogue of Android's `mainThreadCheck`; tests inject a no-op so the class
/// can be exercised without a real dispatch queue.
///
/// ### Overflow policy
///
/// When `enqueue` would push a per-peer buffer past `maxPerPeer`, the entire
/// buffer for that peer is discarded before the new fragment is appended.
/// Dropping the oldest fragment is unsafe: fragments are slices of a single
/// application message, and evicting slice 0 of a five-slice message leaves
/// four orphan slices that the reassembler would stitch into garbage.
/// Whole-buffer drop gives clean backpressure at message-boundary
/// granularity — we lose messages that hadn't finished arriving, but we never
/// hand the protocol a torn message.
final class InboundFragmentBuffer: @unchecked Sendable {

    enum DropReason {
        case cappedPerPeer
        case expired
    }

    private struct Entry {
        let data: Data
        let timestamp: Date
    }

    private let lock = NSLock()
    private var buffers: [UUID: [Entry]] = [:]

    private let queueCheck: () -> Void
    private let maxPerPeer: Int
    private let timeout: TimeInterval
    private let clock: () -> Date
    private let onDropped: (UUID, DropReason, Int) -> Void

    /// - Parameters:
    ///   - queueCheck: asserts the caller is on the owning queue. Invoked by
    ///     mutating operations only; tests inject a no-op.
    ///   - maxPerPeer: per-peer fragment cap before the whole-buffer drop.
    ///   - timeout: idle window, reset by each arriving fragment, before a
    ///     partial buffer is evicted. This holds fragments only until the
    ///     sender's device-id resolves — a GATT connect+read throttled to one
    ///     attempt per `MIN_RECONNECT_INTERVAL` (5s) and prone to GATT retries.
    ///     At 5s a first-contact multi-fragment MLS Welcome arriving in a burst
    ///     could be evicted before resolution completed, losing the Welcome
    ///     before it ever reached the Rust reassembler. 15s comfortably exceeds
    ///     the reverse-resolution worst case while staying under the 30s Rust
    ///     reassembly window; memory stays bounded by `maxPerPeer`.
    init(
        queueCheck: @escaping () -> Void = {},
        maxPerPeer: Int = InboundFragmentBuffer.defaultMaxPerPeer,
        timeout: TimeInterval = InboundFragmentBuffer.defaultTimeout,
        clock: @escaping () -> Date = Date.init,
        onDropped: @escaping (UUID, DropReason, Int) -> Void = { _, _, _ in }
    ) {
        self.queueCheck = queueCheck
        self.maxPerPeer = maxPerPeer
        self.timeout = timeout
        self.clock = clock
        self.onDropped = onDropped
    }

    // MARK: - Cross-thread reads (no queue affinity)

    /// Aggregate fragment count across all peers. Safe from any thread.
    func totalCount() -> Int {
        lock.lock()
        defer { lock.unlock() }
        return buffers.values.reduce(0) { $0 + $1.count }
    }

    /// Snapshot of peers with outstanding buffers. Safe from any thread.
    ///
    /// Used by the main-queue connection monitor to re-kick device-id
    /// resolution for peers whose reverse read stalled — the call site that
    /// used to `dispatch_sync` onto the owning queue.
    func pendingIds() -> [UUID] {
        lock.lock()
        defer { lock.unlock() }
        return Array(buffers.keys)
    }

    /// True if `id` has at least one buffered fragment. Safe from any thread.
    func hasPending(_ id: UUID) -> Bool {
        lock.lock()
        defer { lock.unlock() }
        return !(buffers[id]?.isEmpty ?? true)
    }

    // MARK: - Owning-queue mutations

    /// Append a fragment for `id`. If the per-peer cap is hit, the entire
    /// per-peer buffer is cleared first (see the class-level rationale),
    /// `onDropped` is invoked with the evicted count, and the new fragment is
    /// appended to a fresh empty buffer.
    func enqueue(_ id: UUID, _ data: Data) {
        queueCheck()
        var dropped = 0
        lock.lock()
        // Mutate through the subscript rather than via a local copy: binding
        // the array to a `var` takes a second reference, so every append would
        // deep-copy a buffer that holds up to `maxPerPeer` fragments.
        if (buffers[id]?.count ?? 0) >= maxPerPeer {
            dropped = buffers[id]?.count ?? 0
            buffers[id]?.removeAll(keepingCapacity: true)
        }
        buffers[id, default: []].append(Entry(data: data, timestamp: clock()))
        lock.unlock()

        // Reported outside the lock: `onDropped` reaches the diagnostic
        // delegate, and holding a lock across a bridge callback is how a
        // simple mutex turns into the hang this class exists to remove.
        if dropped > 0 {
            onDropped(id, .cappedPerPeer, dropped)
        }
    }

    /// Maintain FIFO ordering for a peer's inbound stream: if `id` already has
    /// buffered fragments waiting for device-id resolution, append `data` and
    /// return true so the caller skips direct processing. Otherwise return
    /// false so the caller can process it immediately.
    ///
    /// Check-and-append is one operation because callers rely on it being
    /// indivisible with respect to `drain`; both run on the owning queue.
    @discardableResult
    func enqueueIfPending(_ id: UUID, _ data: Data) -> Bool {
        queueCheck()
        guard hasPending(id) else { return false }
        enqueue(id, data)
        return true
    }

    /// Remove and return every fragment buffered for `id`, in FIFO order.
    /// Empty bucket ⇒ empty array.
    func drain(_ id: UUID) -> [Data] {
        queueCheck()
        lock.lock()
        defer { lock.unlock() }
        guard let removed = buffers.removeValue(forKey: id) else { return [] }
        return removed.map(\.data)
    }

    /// Drop every fragment buffered for `id` without forwarding — used when
    /// the peer has been evicted and the bytes can never be delivered.
    /// Returns the count dropped so callers can surface a diagnostic.
    /// Safe from any thread — see the exemption in the class-level contract.
    @discardableResult
    func removeAll(_ id: UUID) -> Int {
        lock.lock()
        defer { lock.unlock() }
        return buffers.removeValue(forKey: id)?.count ?? 0
    }

    /// Drop every buffer. Used by transport stop. Safe from any thread.
    func clear() {
        lock.lock()
        buffers.removeAll()
        lock.unlock()
    }

    /// Walk every peer's buffer and evict whole buffers that have gone idle
    /// for `timeout` (no fragment received within the window). Returns the
    /// total number of fragments evicted, for diagnostics.
    ///
    /// Eviction is at WHOLE-BUFFER granularity keyed on the NEWEST entry —
    /// not per-fragment. A per-fragment sweep tears a still-arriving
    /// multi-fragment message: while device-id resolution is pending the
    /// sender keeps streaming fragments, but the sweep drops fragment 0 the
    /// moment it ages past the window while fragments 1..n are still in
    /// flight, handing the reassembler a permanent hole. Keying on the newest
    /// fragment means a buffer survives as long as bytes are still arriving,
    /// and is dropped wholesale only once the peer truly goes silent —
    /// matching the whole-buffer invariant the overflow policy documents.
    @discardableResult
    func evictExpired() -> Int {
        queueCheck()
        let now = clock()
        var evictions: [(UUID, Int)] = []

        lock.lock()
        // Snapshot the keys so the dictionary is never mutated mid-iteration.
        for id in Array(buffers.keys) {
            guard let buffer = buffers[id] else { continue }
            guard let newest = buffer.map(\.timestamp).max() else {
                buffers.removeValue(forKey: id)
                continue
            }
            if now.timeIntervalSince(newest) >= timeout {
                buffers.removeValue(forKey: id)
                evictions.append((id, buffer.count))
            }
        }
        lock.unlock()

        for (id, count) in evictions where count > 0 {
            onDropped(id, .expired, count)
        }
        return evictions.reduce(0) { $0 + $1.1 }
    }

    static let defaultMaxPerPeer = 100
    static let defaultTimeout: TimeInterval = 15.0
}
