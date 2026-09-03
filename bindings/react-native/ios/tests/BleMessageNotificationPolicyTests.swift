import XCTest
@testable import OfflineProtocol

final class BleMessageNotificationPolicyTests: XCTestCase {

    /// A freshly-discovered characteristic reports `isNotifying == false`.
    /// The first discovery pass must enable notifications so message
    /// fragments start reaching the reassembly buffer.
    func testEnablesNotificationsOnAFreshCharacteristic() {
        XCTAssertTrue(BleMessageNotificationPolicy.shouldEnableNotifications(isNotifying: false))
    }

    /// A characteristic surfaced from CoreBluetooth's cache during a
    /// re-discovery reports `isNotifying == true` when the subscription is
    /// still live. Calling `setNotifyValue(true, ...)` on it would produce
    /// another redundant `didUpdateNotificationStateFor` event without any
    /// change in wire behaviour, so the policy declines.
    func testSuppressesNotificationsWhenAlreadySubscribed() {
        XCTAssertFalse(BleMessageNotificationPolicy.shouldEnableNotifications(isNotifying: true))
    }

    /// Successive discovery callbacks on the same live characteristic must
    /// produce exactly one `setNotifyValue` call: the first (fresh discovery)
    /// enables notifications, subsequent re-discoveries against the now-live
    /// subscription are no-ops. This pins the invariant the delegate loop
    /// relies on — one `setNotifyValue` per subscription cycle, not one per
    /// characteristic-discovery event.
    func testSecondDiscoveryOnALiveSubscriptionIsANoOp() {
        var setNotifyCallCount = 0
        var isNotifying = false

        let simulateDiscovery: () -> Void = {
            if BleMessageNotificationPolicy.shouldEnableNotifications(isNotifying: isNotifying) {
                setNotifyCallCount += 1
                // CoreBluetooth flips isNotifying after the delegate accepts
                // the subscription; the simulation reflects that so a second
                // discovery observes the live state.
                isNotifying = true
            }
        }

        simulateDiscovery()
        simulateDiscovery()
        simulateDiscovery()

        XCTAssertEqual(setNotifyCallCount, 1)
        XCTAssertTrue(isNotifying)
    }

    /// After a real disconnect and reconnect the characteristic returns to
    /// `isNotifying == false` (CoreBluetooth resets it on link teardown).
    /// The next discovery must re-enable notifications so message reception
    /// resumes.
    func testReEnablesNotificationsAfterADisconnect() {
        // First subscription cycle.
        var isNotifying = false
        XCTAssertTrue(BleMessageNotificationPolicy.shouldEnableNotifications(isNotifying: isNotifying))
        isNotifying = true

        // A live re-discovery observes the still-live subscription.
        XCTAssertFalse(BleMessageNotificationPolicy.shouldEnableNotifications(isNotifying: isNotifying))

        // Simulated disconnect: CoreBluetooth resets the flag.
        isNotifying = false

        // A discovery after the reconnect must re-subscribe.
        XCTAssertTrue(BleMessageNotificationPolicy.shouldEnableNotifications(isNotifying: isNotifying))
    }
}
