import XCTest
@testable import OfflineProtocol

/// Pins what the TypeScript layer's gap recovery relies on (#514): a number it
/// expected and never saw is served back with the exact event handed over
/// under it, or is absent because it aged out, never served wrong.
final class EventReplayRingTests: XCTestCase {

    private func ev(_ seq: Int) -> String {
        "{\"type\":\"message_received\",\"message_id\":\"m\(seq)\"}"
    }

    func testAMissingNumberIsServedWithItsOwnEvent() {
        let ring = EventReplayRing()
        (0...4).forEach { ring.record(seq: $0, eventJson: ev($0)) }
        XCTAssertEqual(ring.replay(fromSeq: 2, toSeq: 2), [EventReplayRing.Entry(seq: 2, eventJson: ev(2))])
    }

    func testARangeComesBackInOrder() {
        let ring = EventReplayRing()
        (0...9).forEach { ring.record(seq: $0, eventJson: ev($0)) }
        XCTAssertEqual(ring.replay(fromSeq: 3, toSeq: 6).map { $0.seq }, [3, 4, 5, 6])
    }

    func testPastTheCapacityTheOldestGoes() {
        let ring = EventReplayRing(capacity: 4)
        (0...9).forEach { ring.record(seq: $0, eventJson: ev($0)) }
        XCTAssertEqual(ring.count, 4)
        XCTAssertEqual(ring.replay(fromSeq: 4, toSeq: 7).map { $0.seq }, [6, 7])
    }

    func testAnEmptyOrInvertedRangeReturnsNothing() {
        let ring = EventReplayRing()
        (0...3).forEach { ring.record(seq: $0, eventJson: ev($0)) }
        XCTAssertTrue(ring.replay(fromSeq: 3, toSeq: 2).isEmpty)
        XCTAssertTrue(ring.replay(fromSeq: 10, toSeq: 12).isEmpty)
    }

    func testASignalReadingIsNeitherNumberedNorKept() {
        // One numbered per quarter second per peer would push the messages
        // this ring exists for out of it within seconds.
        XCTAssertFalse(EventReplayRing.isReplayed(#"{"type":"peer_rssi","peer_id":"p","transport":"BLE","rssi":-50}"#))
        XCTAssertTrue(EventReplayRing.isReplayed(#"{"type":"message_received","message_id":"m"}"#))
        XCTAssertTrue(EventReplayRing.isReplayed(#"{"type":"neighbor_discovered","peer_id":"p","transport":"BLE","rssi":-50}"#))
        XCTAssertTrue(EventReplayRing.isReplayed(nil), "an event with no JSON keeps today's numbering")
    }
}
