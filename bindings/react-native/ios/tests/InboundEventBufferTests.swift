import XCTest
@testable import OfflineProtocol

/// Pins the semantics the inbound message hold depends on: every message
/// survives on its own key, arrival order holds, the cap is a real capacity,
/// and a session edge refuses what was in flight for the old one.
final class InboundEventBufferTests: XCTestCase {

    func testTwoMessagesWithDifferentIdsBothSurvive() {
        let buffer = InboundEventBuffer()
        let generation = buffer.currentGeneration()
        XCTAssertTrue(buffer.hold(key: "message_received:m1", eventJson: "{\"message_id\":\"m1\"}", generation: generation))
        XCTAssertTrue(buffer.hold(key: "message_received:m2", eventJson: "{\"message_id\":\"m2\"}", generation: generation))

        let drained = buffer.drain()
        XCTAssertEqual(drained.map { $0.key }, ["message_received:m1", "message_received:m2"])
        XCTAssertEqual(drained.map { $0.eventJson }, ["{\"message_id\":\"m1\"}", "{\"message_id\":\"m2\"}"])
        XCTAssertTrue(buffer.isEmpty)
    }

    func testReholdingAKeyReplacesInPlace() {
        let buffer = InboundEventBuffer()
        let generation = buffer.currentGeneration()
        buffer.hold(key: "message_received:m1", eventJson: "old", generation: generation)
        buffer.hold(key: "message_received:m2", eventJson: "{}", generation: generation)
        buffer.hold(key: "message_received:m1", eventJson: "new", generation: generation)

        let drained = buffer.drain()
        XCTAssertEqual(drained.map { $0.key }, ["message_received:m1", "message_received:m2"])
        XCTAssertEqual(drained[0].eventJson, "new")
    }

    func testCapDropsTheOldest() {
        let buffer = InboundEventBuffer(maxEntries: 256)
        let generation = buffer.currentGeneration()
        for i in 0..<257 {
            buffer.hold(key: "message_received:m\(i)", eventJson: "{}", generation: generation)
        }
        XCTAssertEqual(buffer.count, 256)
        let keys = buffer.drain().map { $0.key }
        XCTAssertEqual(keys.first, "message_received:m1")
        XCTAssertEqual(keys.last, "message_received:m256")
    }

    func testRestorePutsUndeliveredBackAtTheHeadAndSkipsRelatchedKeys() {
        let buffer = InboundEventBuffer()
        let generation = buffer.currentGeneration()
        buffer.hold(key: "message_received:m1", eventJson: "{}", generation: generation)
        buffer.hold(key: "message_received:m2", eventJson: "{}", generation: generation)
        let drained = buffer.drain()
        buffer.hold(key: "message_received:m2", eventJson: "newer", generation: generation)
        buffer.hold(key: "message_received:m3", eventJson: "{}", generation: generation)

        buffer.restore(drained)

        let keys = buffer.drain()
        XCTAssertEqual(keys.map { $0.key }, ["message_received:m1", "message_received:m2", "message_received:m3"])
        XCTAssertEqual(keys[1].eventJson, "newer", "a key held again keeps the newer copy")
    }

    func testGenerationBumpRefusesInFlightHoldsAndRestores() {
        let buffer = InboundEventBuffer()
        let stale = buffer.currentGeneration()
        buffer.hold(key: "message_received:m1", eventJson: "{}", generation: stale)
        let drained = buffer.drain()

        buffer.bumpGeneration()

        XCTAssertFalse(buffer.hold(key: "message_received:m2", eventJson: "{}", generation: stale))
        buffer.restore(drained)
        XCTAssertTrue(buffer.isEmpty, "nothing from the old session may reach the new one")
        XCTAssertTrue(buffer.hold(key: "message_received:m3", eventJson: "{}", generation: buffer.currentGeneration()))
    }

    func testKeyIsDerivedFromTypeAndMessageId() {
        let buffer = InboundEventBuffer()
        XCTAssertEqual(
            buffer.key(forEventJson: "{\"type\":\"message_received\",\"message_id\":\"abc\"}"),
            "message_received:abc"
        )
        XCTAssertEqual(
            buffer.key(forEventJson: "{\"type\":\"file_received\",\"file_id\":\"f1\"}"),
            "file_received:f1"
        )
        XCTAssertEqual(
            buffer.key(forEventJson: "{\"type\":\"message_decryption_failed\",\"message_id\":\"d1\"}"),
            "message_decryption_failed:d1"
        )
        XCTAssertNil(buffer.key(forEventJson: "{\"type\":\"internet_status_changed\"}"),
                     "a periodic event is never held")
        XCTAssertNil(buffer.key(forEventJson: "not json"))
        // No id at all: keyed by arrival rather than dropped.
        let a = buffer.key(forEventJson: "{\"type\":\"message_received\"}")
        let b = buffer.key(forEventJson: "{\"type\":\"message_received\"}")
        XCTAssertNotNil(a)
        XCTAssertNotEqual(a, b)
    }

    func testBufferedTypesMatchTheContract() {
        XCTAssertEqual(
            InboundEventBuffer.bufferedEventTypes,
            ["message_received", "file_received", "message_decryption_failed"]
        )
        XCTAssertEqual(InboundEventBuffer.defaultMaxEntries, 256)
    }
}
