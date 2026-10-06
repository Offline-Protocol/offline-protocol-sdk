import XCTest
@testable import OfflineProtocol

/// `BleDensityPolicy`: two phones in a room full of other Bluetooth devices
/// passed each other over for minutes, because every advert in range counted
/// as mesh density and the pass-over was fixed per peripheral. Mirrors
/// android/src/test/.../BleDensityPolicyTest.kt.
final class BleDensityPolicyTests: XCTestCase {
    private let low = 10
    private let high = 50

    func testDensityCountsDistinctCandidatesNotCallbacks() {
        var seen: [String: Date] = [:]
        let start = Date(timeIntervalSince1970: 1_000)
        var count = 0
        // One peer advertising ten times a second for five seconds.
        for tenth in 0..<50 {
            count = BleDensityPolicy.recordAndCount(&seen, id: "peer-1", now: start.addingTimeInterval(Double(tenth) / 10), window: 5)
        }
        XCTAssertEqual(count, 1)
        XCTAssertEqual(BleDensityPolicy.recordAndCount(&seen, id: "peer-2", now: start.addingTimeInterval(5), window: 5), 2)
    }

    func testCandidatesNotSeenWithinTheWindowStopCounting() {
        var seen: [String: Date] = [:]
        let start = Date(timeIntervalSince1970: 1_000)
        _ = BleDensityPolicy.recordAndCount(&seen, id: "old", now: start, window: 5)
        XCTAssertEqual(BleDensityPolicy.recordAndCount(&seen, id: "new", now: start.addingTimeInterval(5.001), window: 5), 1)
    }

    func testASparseMeshPassesNobodyOver() {
        XCTAssertEqual(BleDensityPolicy.skipShare(meshPeerCount: low, lowThreshold: low, highThreshold: high), 0)
        for i in 0..<500 {
            XCTAssertFalse(BleDensityPolicy.shouldSkip(id: "peer-\(i)", meshPeerCount: 2, now: Date(), lowThreshold: low, highThreshold: high))
        }
    }

    func testTheShareRisesWithDensityAndStopsAtTheCap() {
        XCTAssertEqual(BleDensityPolicy.skipShare(meshPeerCount: 30, lowThreshold: low, highThreshold: high), 0.4, accuracy: 1e-9)
        XCTAssertEqual(BleDensityPolicy.skipShare(meshPeerCount: 500, lowThreshold: low, highThreshold: high), BleDensityPolicy.maxSkipShare)
    }

    func testAPeerPassedOverInOneSlotIsReconsideredInALaterOne() {
        // At the cap 80% are passed over per slot; over 100 slots every
        // peripheral comes up, which a fixed per-peripheral hash never did.
        for i in 0..<200 {
            let id = UUID(uuid: (0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, UInt8(i / 256), UInt8(i % 256))).uuidString
            let considered = (0..<100).filter { s in
                !BleDensityPolicy.shouldSkip(id: id, meshPeerCount: 500, now: Date(timeIntervalSince1970: Double(s) * BleDensityPolicy.skipSlot), lowThreshold: low, highThreshold: high)
            }.count
            XCTAssertGreaterThan(considered, 0, "\(id) was passed over in all 100 slots")
        }
    }

    func testTheSharePassedOverInASlotIsTheConfiguredShare() {
        let skipped = (0..<2_000).filter { i in
            BleDensityPolicy.shouldSkip(id: "peer-\(i)", meshPeerCount: 30, now: Date(timeIntervalSince1970: 0), lowThreshold: low, highThreshold: high)
        }.count
        // 0.4 of 2000, with room for the hash.
        XCTAssertTrue((700...900).contains(skipped), "skipped \(skipped) of 2000")
    }

    func testWithinASlotTheDecisionHolds() {
        let slotStart = 3 * BleDensityPolicy.skipSlot
        let first = BleDensityPolicy.shouldSkip(id: "51:AC:A5:39:6C:00", meshPeerCount: 30, now: Date(timeIntervalSince1970: slotStart), lowThreshold: low, highThreshold: high)
        for second in stride(from: 0.0, to: BleDensityPolicy.skipSlot, by: 1.0) {
            XCTAssertEqual(first, BleDensityPolicy.shouldSkip(id: "51:AC:A5:39:6C:00", meshPeerCount: 30, now: Date(timeIntervalSince1970: slotStart + second), lowThreshold: low, highThreshold: high))
        }
    }

    func testTheBucketMatchesAndroid() {
        // BleDensityPolicyTest.kt pins the same values. The two hashes are
        // written by hand, and this keeps them to the same bits. It is not a
        // runtime agreement: Android keys on the peer's MAC and iOS on its own
        // CBPeripheral identifier, never the same string.
        XCTAssertEqual(BleDensityPolicy.bucket(id: "51:AC:A5:39:6C:00", slot: 7), 0.133, accuracy: 1e-9)
        XCTAssertEqual(BleDensityPolicy.bucket(id: "51:AC:A5:39:6C:00", slot: 0), 0.314, accuracy: 1e-9)
    }
}
