import Foundation

/// Decision helper for enabling GATT notifications on the message
/// characteristic during characteristic discovery.
///
/// The characteristic-discovery callback can fire more than once for the same
/// peripheral without an intervening disconnect: the connection monitor's
/// periodic sweep re-invokes `discoverCharacteristics` on peripherals it
/// believes are unclaimed, and iOS state-restoration paths call it again on
/// relaunch. iOS emits `didUpdateNotificationStateFor` on every
/// `setNotifyValue` call, even when the subscription is unchanged, so
/// unconditionally re-enabling notifications on an already-subscribed
/// characteristic produces a runaway stream of redundant notification-state
/// events (observed in the field: 146 subscribed=YES events at ~5s cadence on
/// a single live subscription).
///
/// This helper factors the decision out of the delegate method so the guard
/// is testable in isolation without a live `CBCharacteristic`.
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
