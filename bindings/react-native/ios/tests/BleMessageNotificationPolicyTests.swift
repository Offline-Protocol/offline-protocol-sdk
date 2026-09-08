import XCTest
@testable import OfflineProtocol

/// Covers `BleMessageNotificationPolicy` only.
///
/// The two cases below are the whole input domain; the two after them replay
/// that decision across a scripted sequence of discovery callbacks. Those two
/// drive their own `isNotifying` local, so they document how the flag moves
/// under CoreBluetooth rather than proving anything the first two do not — the
/// simulation is the fixture, not the subject.
///
/// That `BleManager` routes its one `setNotifyValue` call through this policy,
/// and that the handshake reads sit behind the announce gate, is pinned in
/// `react_native_ios_ble_discovery_is_idempotent_on_a_live_link` in the uniffi
/// crate. Nothing here can reach the delegate: it sits behind CoreBluetooth and
/// CI only typechecks it.
final class BleMessageNotificationPolicyTests: XCTestCase {

    /// A freshly-discovered characteristic reports `isNotifying == false`.
    /// The first discovery pass must enable notifications so message
    /// fragments start reaching the reassembly buffer.
    func testEnablesNotificationsOnAFreshCharacteristic() {
        XCTAssertTrue(BleMessageNotificationPolicy.shouldEnableNotifications(isNotifying: false))
    }

    /// A characteristic surfaced from CoreBluetooth's cache during a
    /// re-discovery reports `isNotifying == true` when the subscription is
    /// still live. Calling `setNotifyValue(true, ...)` on it re-emits the
    /// central's own subscribe diagnostic and, if CoreBluetooth forwards the
    /// CCCD write, re-runs the peer's inbound admission path — all without any
    /// change in wire behaviour, so the policy declines.
    func testSuppressesNotificationsWhenAlreadySubscribed() {
        XCTAssertFalse(BleMessageNotificationPolicy.shouldEnableNotifications(isNotifying: true))
    }

    /// Successive discovery callbacks on one subscription cycle yield exactly
    /// one `setNotifyValue`. The fixture moves `isNotifying` the way
    /// CoreBluetooth does — set once the subscription is accepted, and left
    /// alone thereafter — to show that the policy's own answer is what makes
    /// repeated discovery cheap, one call per subscription cycle rather than
    /// one per discovery event.
    func testSecondDiscoveryOnALiveSubscriptionIsANoOp() {
        var setNotifyCallCount = 0
        var isNotifying = false

        let simulateDiscovery: () -> Void = {
            if BleMessageNotificationPolicy.shouldEnableNotifications(isNotifying: isNotifying) {
                setNotifyCallCount += 1
                // CoreBluetooth flips isNotifying after the delegate accepts
                // the subscription; the fixture reflects that so a second
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
    /// resumes. This is why the delegate asks the characteristic rather than
    /// asking whether the peer is already announced: the reset is what makes
    /// the subscription self-healing, and no bookkeeping in the bridge
    /// observes it.
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
