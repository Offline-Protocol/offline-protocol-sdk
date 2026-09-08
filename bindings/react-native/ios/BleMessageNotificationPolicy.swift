import Foundation

/// Decision helper for enabling GATT notifications on the message
/// characteristic during characteristic discovery.
///
/// The characteristic-discovery callback fires more than once for the same
/// peripheral without an intervening disconnect: the connection monitor
/// re-invokes `discoverCharacteristics` on peripherals it does not find in the
/// connection registry, and iOS state-restoration paths call it again on
/// relaunch. Both land on a characteristic that is often already subscribed.
///
/// A redundant `setNotifyValue(true, ...)` is not a silent no-op. `BleManager`
/// implements no `didUpdateNotificationStateFor`, so the call is not observed
/// there, but it is observed twice elsewhere: it re-emits the "Enabled
/// notifications for message characteristic" diagnostic, so a link that
/// subscribed once reads in the field logs as a link that re-subscribes on
/// every sweep (146 such events at roughly a 5 s cadence on one live
/// subscription is what surfaced this); and if CoreBluetooth forwards the CCCD
/// write rather than swallowing it, the *peer* runs `didSubscribeTo` again,
/// which re-enters its whole inbound admission path — a
/// `shouldAcceptInboundConnection` decision that can evict a different peer,
/// then a rebalance — once per sweep for as long as the link is up.
///
/// The decision is `isNotifying` rather than any bookkeeping this file could
/// consult, because `isNotifying` is present-tense evidence about the link
/// itself: a real disconnect resets it, and so does a service invalidation that
/// hands back fresh characteristic objects on a peer the app still considers
/// established. Both cases re-subscribe on the next discovery pass without
/// anything having to remember them.
///
/// This helper factors the decision out of the delegate method so the guard is
/// testable in isolation without a live `CBCharacteristic`. That the delegate
/// actually routes through it is pinned separately, by
/// `react_native_ios_ble_discovery_is_idempotent_on_a_live_link` in the uniffi
/// crate, because no Swift test can reach `BleManager`.
internal enum BleMessageNotificationPolicy {
    /// Whether the app should issue `setNotifyValue(true, ...)` for the
    /// message characteristic given its current subscription state.
    ///
    /// Callers pass `isNotifying` from `CBCharacteristic.isNotifying` so the
    /// decision stays a pure function of the observed state and this file
    /// never has to import CoreBluetooth. Returns `true` only when the
    /// characteristic is not already subscribed; a live subscription is left
    /// alone.
    static func shouldEnableNotifications(isNotifying: Bool) -> Bool {
        return !isNotifying
    }
}
