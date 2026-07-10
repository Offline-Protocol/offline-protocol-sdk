//
// RelayControlOpTranslatorTests.swift
// Mirrors android/src/test/java/com/offlineprotocol/RelayControlOpTranslatorTest.kt
//

import XCTest
@testable import OfflineProtocol

final class RelayControlOpTranslatorTests: XCTestCase {

    private func frames(_ t: RelayControlOpTranslator.Translation) -> [[String: Any]] {
        switch t {
        case .replace(let frames): return frames
        case .tap(let frames): return frames
        case .passThrough: return []
        }
    }

    private func isPassThrough(_ t: RelayControlOpTranslator.Translation) -> Bool {
        if case .passThrough = t { return true }
        return false
    }

    func testConnectionOpsTranslateToRelayNativeFrames() {
        let translator = RelayControlOpTranslator(selfId: "alice")

        let req = frames(translator.translate(
            controlOp: "conn_req",
            controlPayload: #"{"sender_name":"Alice","timestamp_ms":1,"key_package":[1,2,3]}"#,
            recipientId: "bob"
        ))
        XCTAssertEqual(req.count, 1)
        XCTAssertEqual(req[0]["type"] as? String, "SendConnectionRequest")
        XCTAssertEqual(req[0]["recipient"] as? String, "bob")
        XCTAssertEqual(req[0]["sender_name"] as? String, "Alice")
        XCTAssertEqual((req[0]["key_package"] as? [Any])?.count, 3)

        let acc = frames(translator.translate(
            controlOp: "conn_acc",
            controlPayload: #"{"accepted_by_name":"Alice","timestamp_ms":1}"#,
            recipientId: "bob"
        ))
        XCTAssertEqual(acc[0]["type"] as? String, "AcceptConnectionRequest")
        XCTAssertEqual(acc[0]["requester_id"] as? String, "bob")
        XCTAssertEqual(acc[0]["accepter_name"] as? String, "Alice")
        XCTAssertNil(acc[0]["key_package"])

        let rej = frames(translator.translate(controlOp: "conn_rej", controlPayload: "", recipientId: "bob"))
        XCTAssertEqual(rej[0]["type"] as? String, "RejectConnectionRequest")
        XCTAssertEqual(rej[0]["requester_id"] as? String, "bob")

        let can = frames(translator.translate(controlOp: "conn_can", controlPayload: "", recipientId: "bob"))
        XCTAssertEqual(can[0]["type"] as? String, "CancelConnectionRequest")
        XCTAssertEqual(can[0]["recipient"] as? String, "bob")
    }

    func testRegisterTranslatesToCreateGroupPlusMemberDeltas() {
        let translator = RelayControlOpTranslator(selfId: "alice")

        let first = frames(translator.translate(
            controlOp: "group_relay_register",
            controlPayload: #"{"group_id":"g1","group_name":"Trip","members":["alice","bob","carol"]}"#,
            recipientId: "alice"
        ))
        XCTAssertEqual(first[0]["type"] as? String, "CreateGroup")
        XCTAssertEqual(first[0]["group_id"] as? String, "g1")
        XCTAssertEqual(first[0]["name"] as? String, "Trip")
        // Self never appears in deltas (the relay adds the creator itself).
        let added = Set(first.dropFirst().map {
            "\($0["type"] as? String ?? ""):\($0["username"] as? String ?? "")"
        })
        XCTAssertEqual(added, ["AddGroupMember:bob", "AddGroupMember:carol"])

        // Re-registration after a membership change sends only the deltas.
        let second = frames(translator.translate(
            controlOp: "group_relay_register",
            controlPayload: #"{"group_id":"g1","members":["alice","bob","dave"]}"#,
            recipientId: "alice"
        ))
        XCTAssertEqual(second[0]["type"] as? String, "CreateGroup")
        // Name falls back to group_id when the payload omits it.
        XCTAssertEqual(second[0]["name"] as? String, "g1")
        let deltas = Set(second.dropFirst().map {
            "\($0["type"] as? String ?? ""):\($0["username"] as? String ?? "")"
        })
        XCTAssertEqual(deltas, ["AddGroupMember:dave", "RemoveGroupMember:carol"])
    }

    func testAdminDeniedGroupsStopProducingMemberDeltas() {
        let translator = RelayControlOpTranslator(selfId: "bob")

        _ = translator.translate(
            controlOp: "group_relay_register",
            controlPayload: #"{"group_id":"g1","members":["alice","bob","carol"]}"#,
            recipientId: "bob"
        )
        translator.onGroupError(groupId: "g1", reason: "Only admins can add members")

        let after = frames(translator.translate(
            controlOp: "group_relay_register",
            controlPayload: #"{"group_id":"g1","members":["alice","bob","carol","dave"]}"#,
            recipientId: "bob"
        ))
        // CreateGroup still goes out (idempotent member sync → GroupCreated
        // ack keeps relay_synced fresh), but no more deltas from a non-admin.
        XCTAssertEqual(after.count, 1)
        XCTAssertEqual(after[0]["type"] as? String, "CreateGroup")
    }

    func testBroadcastTranslatesToSendGroupMessage() {
        let translator = RelayControlOpTranslator(selfId: "alice")
        let bcast = frames(translator.translate(
            controlOp: "group_relay_broadcast",
            controlPayload: #"{"group_id":"g1","ciphertext":"AAECAw==","epoch":4,"reply_to":"m-9"}"#,
            recipientId: "alice"
        ))
        XCTAssertEqual(bcast.count, 1)
        XCTAssertEqual(bcast[0]["type"] as? String, "SendGroupMessage")
        XCTAssertEqual(bcast[0]["group_id"] as? String, "g1")
        XCTAssertEqual(bcast[0]["content"] as? String, "AAECAw==")
        XCTAssertEqual(bcast[0]["reply_to_msg"] as? String, "m-9")
    }

    func testLeaveIsATapSentOncePerGroupForSelfOnly() {
        let translator = RelayControlOpTranslator(selfId: "alice")

        let tap = translator.translate(
            controlOp: "group_mls_leave",
            controlPayload: #"{"group_id":"g1","leaving_member":"alice"}"#,
            recipientId: "bob"
        )
        guard case .tap(let leaveFrames) = tap else {
            return XCTFail("leave must be a tap")
        }
        XCTAssertEqual(leaveFrames.count, 1)
        XCTAssertEqual(leaveFrames[0]["type"] as? String, "LeaveGroup")
        XCTAssertEqual(leaveFrames[0]["group_id"] as? String, "g1")

        // Second per-member leave notification: no duplicate LeaveGroup.
        let second = translator.translate(
            controlOp: "group_mls_leave",
            controlPayload: #"{"group_id":"g1","leaving_member":"alice"}"#,
            recipientId: "carol"
        )
        XCTAssertTrue(frames(second).isEmpty)

        // Someone else leaving is not our LeaveGroup to send.
        let other = translator.translate(
            controlOp: "group_mls_leave",
            controlPayload: #"{"group_id":"g2","leaving_member":"bob"}"#,
            recipientId: "carol"
        )
        XCTAssertTrue(frames(other).isEmpty)
    }

    func testMalformedPayloadAndUnknownOpFallBackToPassThrough() {
        let translator = RelayControlOpTranslator(selfId: "alice")
        XCTAssertTrue(isPassThrough(
            translator.translate(controlOp: "conn_req", controlPayload: "not-json", recipientId: "bob")
        ))
        XCTAssertTrue(isPassThrough(
            translator.translate(controlOp: "some_future_op", controlPayload: "{}", recipientId: "bob")
        ))
        XCTAssertTrue(isPassThrough(
            translator.translate(controlOp: "group_relay_register", controlPayload: #"{"members":[]}"#, recipientId: "alice")
        ))
    }

    func testResetForgetsRegistrationDiffState() {
        let translator = RelayControlOpTranslator(selfId: "alice")
        _ = translator.translate(
            controlOp: "group_relay_register",
            controlPayload: #"{"group_id":"g1","members":["alice","bob"]}"#,
            recipientId: "alice"
        )
        translator.reset()

        // After reconnect the full membership registers again.
        let again = frames(translator.translate(
            controlOp: "group_relay_register",
            controlPayload: #"{"group_id":"g1","members":["alice","bob"]}"#,
            recipientId: "alice"
        ))
        XCTAssertEqual(again.count, 2)
        XCTAssertEqual(again[1]["type"] as? String, "AddGroupMember")
        XCTAssertEqual(again[1]["username"] as? String, "bob")
    }
}
