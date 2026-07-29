//
// LegacyRelayMessageTests.swift
//
// Pins the reconstructed-Message shape against the Rust `Message`
// deserializer's requirements (see `serialized_control_frame` in
// `crates/offline-protocol-uniffi/src/lib.rs` — the Rust twin of this
// shape). Regression pin for the silent-drop bug where synthesized relay
// frames were missing `id`/`timestamp` and used `"Medium"` instead of the
// canonical lowercase priority. Mirrors android's LegacyRelayMessageTest —
// keep in sync.
//

import XCTest
@testable import OfflineProtocol

final class LegacyRelayMessageTests: XCTestCase {

    func testEveryFieldRequiredByTheRustDeserializerIsPresent() {
        let dict = LegacyRelayMessage.buildDict(
            senderId: "alice",
            recipientId: "bob",
            content: "hello",
            timestampMs: 1_700_000_000_000
        )

        // id, sender, recipient, app_id, priority, ttl, hop_count, content,
        // and timestamp have no serde default in the Rust Message struct —
        // a frame missing any of them fails deserialization and is silently
        // dropped by the transport.
        for key in [
            "id", "sender", "recipient", "content", "app_id",
            "priority", "ttl", "hop_count", "requires_ack", "timestamp"
        ] {
            XCTAssertNotNil(dict[key], "required field '\(key)' missing")
        }
    }

    func testPriorityIsTheCanonicalLowercaseVariant() {
        let dict = LegacyRelayMessage.buildDict(
            senderId: "alice",
            recipientId: "bob",
            content: "hello",
            timestampMs: 1_700_000_000_000
        )
        // rename_all = "lowercase" on MessagePriority: "Medium" fails.
        XCTAssertEqual(dict["priority"] as? String, "medium")
    }

    func testIdFallsBackToAUuidWhenAbsentAndPassesThroughWhenGiven() {
        let generated = LegacyRelayMessage.buildDict(
            senderId: "alice",
            recipientId: "bob",
            content: "hello",
            timestampMs: 1_700_000_000_000,
            messageId: ""
        )
        // MessageId deserializes via Uuid::parse_str — a non-UUID id fails.
        XCTAssertNotNil(UUID(uuidString: generated["id"] as? String ?? ""))

        let given = LegacyRelayMessage.buildDict(
            senderId: "alice",
            recipientId: "bob",
            content: "hello",
            timestampMs: 1_700_000_000_000,
            messageId: "6dd7f6f0-9d2c-4b6a-8f3e-2a1b0c9d8e7f"
        )
        XCTAssertEqual(given["id"] as? String, "6dd7f6f0-9d2c-4b6a-8f3e-2a1b0c9d8e7f")
    }

    func testReplyToMsgIncludedOnlyWhenNonEmpty() {
        let without = LegacyRelayMessage.buildDict(
            senderId: "alice",
            recipientId: "bob",
            content: "hello",
            timestampMs: 1_700_000_000_000,
            replyToMsg: ""
        )
        XCTAssertNil(without["reply_to_msg"])

        let with = LegacyRelayMessage.buildDict(
            senderId: "alice",
            recipientId: "bob",
            content: "hello",
            timestampMs: 1_700_000_000_000,
            replyToMsg: "7ee8f6f0-9d2c-4b6a-8f3e-2a1b0c9d8e7f"
        )
        XCTAssertEqual(with["reply_to_msg"] as? String, "7ee8f6f0-9d2c-4b6a-8f3e-2a1b0c9d8e7f")
    }

    func testScalarFieldsCarryTheWireValues() {
        let dict = LegacyRelayMessage.buildDict(
            senderId: "alice",
            recipientId: "bob",
            content: "hello",
            timestampMs: 1_700_000_000_000
        )
        XCTAssertEqual(dict["sender"] as? String, "alice")
        XCTAssertEqual(dict["recipient"] as? String, "bob")
        XCTAssertEqual(dict["content"] as? String, "hello")
        XCTAssertEqual(dict["timestamp"] as? Int64, 1_700_000_000_000)
        XCTAssertEqual(dict["hop_count"] as? Int, 0)
        XCTAssertEqual(dict["ttl"] as? Int, 8)
        XCTAssertEqual(dict["requires_ack"] as? Bool, true)
    }

    /// Bridge-synthesized frames (injectGroupInternalMessage) opt out of the
    /// ACK: nothing transmitted them, so no sender awaits a delivery
    /// confirmation — and the core addresses that ACK to the frame's
    /// `sender`, which for a relay answer is a placeholder, not a reachable
    /// peer. Regression pin for the phantom-peer bug, where every injected
    /// frame produced an undeliverable outbound DM to "relay".
    func testRequiresAckIsOptOutForSynthesizedFrames() {
        let dict = LegacyRelayMessage.buildDict(
            senderId: "relay",
            recipientId: "bob",
            content: "__GROUP_CREATED__{}",
            timestampMs: 1_700_000_000_000,
            requiresAck: false
        )
        XCTAssertEqual(dict["requires_ack"] as? Bool, false)
        // The opt-out must not disturb the required-field set.
        for key in [
            "id", "sender", "recipient", "content", "app_id",
            "priority", "ttl", "hop_count", "requires_ack", "timestamp"
        ] {
            XCTAssertNotNil(dict[key], "required field '\(key)' missing")
        }
    }
}
