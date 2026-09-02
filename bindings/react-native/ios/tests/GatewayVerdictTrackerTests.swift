import XCTest

@testable import OfflineProtocol

final class GatewayVerdictTrackerTests: XCTestCase {

    func testAFrameIsTrackedUntilItIsSettled() {
        let tracker = GatewayVerdictTracker()
        XCTAssertEqual(tracker.count, 0)

        XCTAssertTrue(tracker.begin("abc", now: 0))
        XCTAssertEqual(tracker.count, 1)

        XCTAssertTrue(tracker.settle("abc"))
        XCTAssertEqual(tracker.count, 0)
    }

    /// The core re-queues an unconfirmed frame under the same id after its own
    /// acknowledgement timeout, and a verdict can honestly take longer than
    /// that over a radio backbone. Sending it again forwards the frame twice
    /// and, when the second copy times out, fails an id the gateway already
    /// confirmed.
    func testAnIdAlreadyInFlightIsRefused() {
        let tracker = GatewayVerdictTracker()
        XCTAssertTrue(tracker.begin("abc", now: 0))
        XCTAssertFalse(tracker.begin("abc", now: 30), "the same id must not be sent twice")
        XCTAssertEqual(tracker.count, 1, "and the first attempt still owns the slot")
    }

    /// A duplicate verdict must not report a second outcome for a frame the
    /// core has already moved past.
    func testSettlingTwiceReportsOnlyOnce() {
        let tracker = GatewayVerdictTracker()
        _ = tracker.begin("abc", now: 0)
        XCTAssertTrue(tracker.settle("abc"))
        XCTAssertFalse(tracker.settle("abc"))
    }

    func testSettlingSomethingUnknownIsIgnored() {
        let tracker = GatewayVerdictTracker()
        XCTAssertFalse(tracker.settle("never-sent"))
    }

    /// A gateway that answers nothing is indistinguishable from a wedged
    /// socket, and the core cannot retry a frame nobody failed. The sweep is
    /// what turns silence back into a retry.
    func testFramesOlderThanTheTimeoutAreExpired() {
        let tracker = GatewayVerdictTracker()
        _ = tracker.begin("old", now: 0)
        _ = tracker.begin("recent", now: 55)

        let expired = tracker.expired(now: 61, timeout: 60)

        XCTAssertEqual(expired, ["old"])
        XCTAssertEqual(tracker.count, 1, "the recent frame is still outstanding")
    }

    func testExpiryIsExclusiveOfTheBoundary() {
        let tracker = GatewayVerdictTracker()
        _ = tracker.begin("edge", now: 0)
        XCTAssertTrue(tracker.expired(now: 60, timeout: 60).isEmpty)
        XCTAssertEqual(tracker.expired(now: 60.001, timeout: 60), ["edge"])
    }

    /// A connection going away owes an outcome on everything it was carrying.
    /// A frame nobody reports on waits out the core's own 120s expiry.
    func testDrainingHandsBackEverythingOutstanding() {
        let tracker = GatewayVerdictTracker()
        _ = tracker.begin("a", now: 0)
        _ = tracker.begin("b", now: 0)

        XCTAssertEqual(tracker.drainAll().sorted(), ["a", "b"])
        XCTAssertEqual(tracker.count, 0)
        XCTAssertTrue(tracker.drainAll().isEmpty)
    }

    /// The manager touches this from its send queue and from the socket queue,
    /// so `begin` has to be a single atomic decision: two threads racing the
    /// same id must not both be told to send it.
    func testConcurrentBeginsAdmitExactlyOne() {
        let tracker = GatewayVerdictTracker()
        let admitted = NSMutableArray()
        let lock = NSLock()
        let group = DispatchGroup()

        for _ in 0..<64 {
            DispatchQueue.global().async(group: group) {
                if tracker.begin("contended", now: 0) {
                    lock.lock()
                    admitted.add(1)
                    lock.unlock()
                }
            }
        }
        group.wait()

        XCTAssertEqual(admitted.count, 1)
        XCTAssertEqual(tracker.count, 1)
    }
}
