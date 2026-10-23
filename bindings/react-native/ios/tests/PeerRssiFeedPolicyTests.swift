import XCTest
@testable import OfflineProtocol

/// `PeerRssiFeedPolicy`: whether a reading reaches the core at all. A
/// throttle that never admits looks exactly like no peer in range. Mirrors
/// the feed half of android/src/test/.../PeerRssiFeedPolicyTest.kt.
final class PeerRssiFeedPolicyTests: XCTestCase {
    private let a = UUID()
    private let b = UUID()
    private let start = Date(timeIntervalSince1970: 1_000)

    func testOneReadingPerIntervalPerPeripheral() {
        var fed: [UUID: Date] = [:]
        XCTAssertTrue(PeerRssiFeedPolicy.admit(&fed, id: a, now: start, linked: true, hasServiceData: false))
        XCTAssertFalse(PeerRssiFeedPolicy.admit(&fed, id: a, now: start.addingTimeInterval(0.1), linked: true, hasServiceData: false))
        XCTAssertTrue(PeerRssiFeedPolicy.admit(&fed, id: b, now: start.addingTimeInterval(0.1), linked: false, hasServiceData: true),
                      "another peripheral is not held back")
        // Just past the interval: Date arithmetic is floating point.
        XCTAssertTrue(PeerRssiFeedPolicy.admit(&fed, id: a, now: start.addingTimeInterval(PeerRssiFeedPolicy.interval + 0.001), linked: true, hasServiceData: false))
    }

    func testAResultTheCoreCouldNotAttributeIsNeverFed() {
        var fed: [UUID: Date] = [:]
        XCTAssertFalse(PeerRssiFeedPolicy.admit(&fed, id: a, now: start, linked: false, hasServiceData: false))
        XCTAssertTrue(PeerRssiFeedPolicy.admit(&fed, id: a, now: start.addingTimeInterval(0.001), linked: true, hasServiceData: false),
                      "and does not use up the peripheral's slot")
    }

    func testTheIntervalLeavesRoomUnderTheCoreCap() {
        // The core drops anything under 250 ms; equal intervals halved the rate.
        XCTAssertGreaterThan(PeerRssiFeedPolicy.interval, 0.25)
    }
}
