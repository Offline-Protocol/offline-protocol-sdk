//
// BleWriteStallPolicy.swift
// OfflineProtocol
//
// Decides when a central link whose write buffer never drains is dead and
// must be torn down so it can reconnect.
//
// A write-without-response is refused while CoreBluetooth's buffer for the
// peripheral is full, and `peripheralIsReady(toSendWriteWithoutResponse:)`
// says when it has room again. Under load that cycle turns over in
// milliseconds. In the 2026-10-19 volume rerun it stopped: the buffer stayed
// full for 17 minutes while the same link kept delivering the peer's
// notifications, so nothing ever reported the link as down, every fragment
// queued for the peer expired, and the peer's acknowledgements never left.
// The link recovered only when an unrelated connection to the peer's next
// advertising identity happened to complete.
//
// The rule is time without a successful write, not a count of refusals: a
// refusal is normal flow control, and a busy link refuses constantly while
// still moving. Any accepted write, or a ready callback, ends the stall.
// Tearing a link down costs one reconnect, so the threshold sits far above
// any real backpressure wait and well below the 30 s fragment expiry.
//
// Safe from any thread: refusals are seen on the fragment queue and ready
// callbacks on main.
//

import Foundation

final class BleWriteStallPolicy {
    /// How long a link may refuse every write before it is declared dead.
    static let defaultStallSeconds: TimeInterval = 15

    private let stallSeconds: TimeInterval
    private let lock = NSLock()
    private var blockedSince: [UUID: Date] = [:]

    init(stallSeconds: TimeInterval = BleWriteStallPolicy.defaultStallSeconds) {
        self.stallSeconds = stallSeconds
    }

    /// Records a refused write on `peripheral`. Returns true once the link
    /// has refused everything for the threshold or longer: the caller tears
    /// it down. The stall is forgotten when this returns true, so one dead
    /// link produces one teardown.
    func noteRefused(_ peripheral: UUID, now: Date = Date()) -> Bool {
        lock.lock()
        defer { lock.unlock() }
        guard let since = blockedSince[peripheral] else {
            blockedSince[peripheral] = now
            return false
        }
        guard now.timeIntervalSince(since) >= stallSeconds else { return false }
        blockedSince[peripheral] = nil
        return true
    }

    /// The link took a write, or said it has room: it is moving.
    func noteWritable(_ peripheral: UUID) {
        lock.lock()
        blockedSince[peripheral] = nil
        lock.unlock()
    }

    /// The link is gone, or has just connected: a reconnect starts from a
    /// clean slate. Called from `didConnect` and `didDisconnectPeripheral`.
    ///
    /// Without it the clock outlived the teardown. The cancel that ends a
    /// stalled link is an asynchronous hop to main, and the redrain floor can
    /// refuse another write in between, re-arming the clock for an identifier
    /// CoreBluetooth keeps across the reconnect. The fresh link then met one
    /// refusal before its first accepted write and was torn down at once.
    func forget(_ peripheral: UUID) {
        noteWritable(peripheral)
    }

    /// Forgets every link: the bridge stopped, or the radio went away.
    func reset() {
        lock.lock()
        blockedSince.removeAll()
        lock.unlock()
    }
}
