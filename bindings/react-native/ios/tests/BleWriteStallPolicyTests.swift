import XCTest
@testable import OfflineProtocol

final class BleWriteStallPolicyTests: XCTestCase {
    private let link = UUID()
    private let start = Date(timeIntervalSince1970: 1_000)

    /// Refusals are normal flow control. Only a link that refuses everything
    /// for the whole threshold is dead.
    func testALinkIsDeadOnlyAfterRefusingEverythingForTheThreshold() {
        let policy = BleWriteStallPolicy(stallSeconds: 15)
        XCTAssertFalse(policy.noteRefused(link, now: start))
        XCTAssertFalse(policy.noteRefused(link, now: start.addingTimeInterval(14)))
        XCTAssertTrue(policy.noteRefused(link, now: start.addingTimeInterval(15)))
    }

    /// A busy link refuses constantly while still moving; one accepted write
    /// or ready callback restarts the clock.
    func testAnyWriteTheLinkTakesEndsTheStall() {
        let policy = BleWriteStallPolicy(stallSeconds: 15)
        XCTAssertFalse(policy.noteRefused(link, now: start))
        policy.noteWritable(link)
        XCTAssertFalse(policy.noteRefused(link, now: start.addingTimeInterval(14)))
        XCTAssertFalse(policy.noteRefused(link, now: start.addingTimeInterval(28)))
        XCTAssertTrue(policy.noteRefused(link, now: start.addingTimeInterval(29)))
    }

    /// One dead link produces one teardown, and the reconnect starts clean.
    func testATeardownIsReportedOnceAndTheNextStallStartsOver() {
        let policy = BleWriteStallPolicy(stallSeconds: 15)
        XCTAssertFalse(policy.noteRefused(link, now: start))
        XCTAssertTrue(policy.noteRefused(link, now: start.addingTimeInterval(20)))
        XCTAssertFalse(policy.noteRefused(link, now: start.addingTimeInterval(21)))
        XCTAssertFalse(policy.noteRefused(link, now: start.addingTimeInterval(35)))
        XCTAssertTrue(policy.noteRefused(link, now: start.addingTimeInterval(36)))
    }

    /// Each link has its own clock.
    func testLinksStallIndependently() {
        let policy = BleWriteStallPolicy(stallSeconds: 15)
        let other = UUID()
        XCTAssertFalse(policy.noteRefused(link, now: start))
        XCTAssertFalse(policy.noteRefused(other, now: start.addingTimeInterval(10)))
        XCTAssertTrue(policy.noteRefused(link, now: start.addingTimeInterval(16)))
        XCTAssertFalse(policy.noteRefused(other, now: start.addingTimeInterval(16)))
    }

    /// A link that disconnects and reconnects under the same identifier
    /// starts clean, even if a refusal re-armed the clock after the teardown
    /// verdict and before the disconnect landed.
    func testForgetClearsAClockReArmedBetweenTheVerdictAndTheDisconnect() {
        let policy = BleWriteStallPolicy(stallSeconds: 15)
        XCTAssertFalse(policy.noteRefused(link, now: start))
        XCTAssertTrue(policy.noteRefused(link, now: start.addingTimeInterval(15)))
        // The redrain refuses again before the cancel reaches main.
        XCTAssertFalse(policy.noteRefused(link, now: start.addingTimeInterval(16)))
        policy.forget(link)
        // The reconnected link's first refusal starts a new clock.
        XCTAssertFalse(policy.noteRefused(link, now: start.addingTimeInterval(40)))
        XCTAssertFalse(policy.noteRefused(link, now: start.addingTimeInterval(54)))
    }

    /// Stopping the bridge forgets every link.
    func testResetForgetsEveryLink() {
        let policy = BleWriteStallPolicy(stallSeconds: 15)
        let other = UUID()
        XCTAssertFalse(policy.noteRefused(link, now: start))
        XCTAssertFalse(policy.noteRefused(other, now: start))
        policy.reset()
        XCTAssertFalse(policy.noteRefused(link, now: start.addingTimeInterval(20)))
        XCTAssertFalse(policy.noteRefused(other, now: start.addingTimeInterval(20)))
    }
}
