//
// RelayControlOpTranslatorTests.swift
// Mirrors android/src/test/java/com/offlineprotocol/RelayControlOpTranslatorTest.kt
//

import XCTest
@testable import OfflineProtocol

final class RelayControlOpTranslatorTests: XCTestCase {

    private func frames(_ t: RelayControlOpTranslator.Translation) -> [[String: Any]] {
        switch t {
        case .replace(let frames, _): return frames
        case .tap(let frames, _): return frames
        case .passThrough: return []
        }
    }

    /// Simulates InternetManager writing every frame successfully.
    private func commit(_ t: RelayControlOpTranslator.Translation) {
        switch t {
        case .replace(_, let commit): commit?()
        case .tap(_, let commit): commit?()
        case .passThrough: break
        }
    }

    private func isPassThrough(_ t: RelayControlOpTranslator.Translation) -> Bool {
        if case .passThrough = t { return true }
        return false
    }

    /// Connection ops are no longer server-plane: they ship verbatim as
    /// signed SendMessage frames, so the translator must pass them through
    /// untouched (they should never even be tagged by the core).
    func testConnectionOpsPassThroughVerbatim() {
        let translator = RelayControlOpTranslator(selfId: "alice", selfAddress: { nil })
        for (op, payload) in [
            ("conn_req", #"{"sender_name":"Alice","timestamp_ms":1,"key_package":[1,2,3]}"#),
            ("conn_acc", #"{"accepted_by_name":"Alice","timestamp_ms":1}"#),
            ("conn_rej", ""),
            ("conn_can", "")
        ] {
            let translation = translator.translate(
                controlOp: op,
                controlPayload: payload,
                recipientId: "bob"
            )
            XCTAssertTrue(
                isPassThrough(translation),
                "\(op) must pass through as a verbatim SendMessage"
            )
        }
    }

    func testRegisterTranslatesToCreateGroupPlusMemberDeltas() {
        let translator = RelayControlOpTranslator(selfId: "alice", selfAddress: { nil })

        let firstTranslation = translator.translate(
            controlOp: "group_relay_register",
            controlPayload: #"{"group_id":"g1","group_name":"Trip","members":["alice","bob","carol"]}"#,
            recipientId: "alice"
        )
        let first = frames(firstTranslation)
        XCTAssertEqual(first[0]["type"] as? String, "CreateGroup")
        XCTAssertEqual(first[0]["group_id"] as? String, "g1")
        XCTAssertEqual(first[0]["name"] as? String, "Trip")
        // Self never appears in deltas (the relay adds the creator itself),
        // and deltas are sorted for a deterministic wire order.
        let added = first.dropFirst().map {
            "\($0["type"] as? String ?? ""):\($0["username"] as? String ?? "")"
        }
        XCTAssertEqual(added, ["AddGroupMember:bob", "AddGroupMember:carol"])
        commit(firstTranslation)

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
        let translator = RelayControlOpTranslator(selfId: "bob", selfAddress: { nil })

        commit(translator.translate(
            controlOp: "group_relay_register",
            controlPayload: #"{"group_id":"g1","members":["alice","bob","carol"]}"#,
            recipientId: "bob"
        ))
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
        let translator = RelayControlOpTranslator(selfId: "alice", selfAddress: { nil })
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
        // `epoch` is deliberately not forwarded — OpenMLS reads the epoch from
        // the ciphertext header, so the payload copy is informational only.
        XCTAssertNil(bcast[0]["epoch"])
    }

    func testBroadcastStampsLogicalMessageId() {
        let translator = RelayControlOpTranslator(selfId: "alice", selfAddress: { nil })
        let bcast = frames(translator.translate(
            controlOp: "group_relay_broadcast",
            controlPayload: #"{"group_id":"g1","ciphertext":"AAECAw==","epoch":4,"message_id":"11111111-2222-3333-4444-555555555555"}"#,
            recipientId: "alice"
        ))
        XCTAssertEqual(bcast.count, 1)
        // The logical id is what the v2 relay echoes to members, keys its
        // push dedup by, and names in the settled delivery report the core
        // correlates on — dropping it here would break all three.
        XCTAssertEqual(
            bcast[0]["message_id"] as? String,
            "11111111-2222-3333-4444-555555555555"
        )
    }

    func testBroadcastOmitsMessageIdWhenAbsent() {
        let translator = RelayControlOpTranslator(selfId: "alice", selfAddress: { nil })
        let bcast = frames(translator.translate(
            controlOp: "group_relay_broadcast",
            controlPayload: #"{"group_id":"g1","ciphertext":"AAECAw==","epoch":4}"#,
            recipientId: "alice"
        ))
        XCTAssertEqual(bcast.count, 1)
        XCTAssertNil(bcast[0]["message_id"])
    }

    func testBroadcastForwardsForwardInfo() {
        let translator = RelayControlOpTranslator(selfId: "alice", selfAddress: { nil })
        let bcast = frames(translator.translate(
            controlOp: "group_relay_broadcast",
            controlPayload: #"{"group_id":"g1","ciphertext":"AAECAw==","epoch":4,"forward_info":{"original_sender":"dave","forwarded_at":123}}"#,
            recipientId: "alice"
        ))
        XCTAssertEqual(bcast.count, 1)
        let forwardInfo = bcast[0]["forward_info"] as? [String: Any]
        XCTAssertNotNil(forwardInfo, "forward_info must ride the relay frame, not be dropped")
        XCTAssertEqual(forwardInfo?["original_sender"] as? String, "dave")
    }

    func testBroadcastOmitsForwardInfoWhenAbsent() {
        let translator = RelayControlOpTranslator(selfId: "alice", selfAddress: { nil })
        let bcast = frames(translator.translate(
            controlOp: "group_relay_broadcast",
            controlPayload: #"{"group_id":"g1","ciphertext":"AAECAw==","epoch":4}"#,
            recipientId: "alice"
        ))
        XCTAssertEqual(bcast.count, 1)
        XCTAssertNil(bcast[0]["forward_info"])
    }

    func testLeaveIsATapSentOncePerGroupForSelfOnly() {
        let translator = RelayControlOpTranslator(selfId: "alice", selfAddress: { nil })

        let tap = translator.translate(
            controlOp: "group_mls_leave",
            controlPayload: #"{"group_id":"g1","leaving_member":"alice"}"#,
            recipientId: "bob"
        )
        guard case .tap(let leaveFrames, _) = tap else {
            return XCTFail("leave must be a tap")
        }
        XCTAssertEqual(leaveFrames.count, 1)
        XCTAssertEqual(leaveFrames[0]["type"] as? String, "LeaveGroup")
        XCTAssertEqual(leaveFrames[0]["group_id"] as? String, "g1")
        commit(tap)

        // Second per-member leave notification: still a tap (the verbatim
        // send must go out) with no duplicate LeaveGroup.
        let second = translator.translate(
            controlOp: "group_mls_leave",
            controlPayload: #"{"group_id":"g1","leaving_member":"alice"}"#,
            recipientId: "carol"
        )
        guard case .tap(let secondFrames, _) = second else {
            return XCTFail("deduped leave must stay a tap, not degrade to passThrough")
        }
        XCTAssertTrue(secondFrames.isEmpty)

        // Someone else leaving is not our LeaveGroup to send.
        let other = translator.translate(
            controlOp: "group_mls_leave",
            controlPayload: #"{"group_id":"g2","leaving_member":"bob"}"#,
            recipientId: "carol"
        )
        guard case .tap(let otherFrames, _) = other else {
            return XCTFail("third-party leave must stay a tap")
        }
        XCTAssertTrue(otherFrames.isEmpty)
    }

    func testNotAdminHintSkipsMemberDeltasUpFront() {
        let translator = RelayControlOpTranslator(selfId: "bob", selfAddress: { nil })

        // The core's is_admin=false hint: no deltas are ever attempted, so
        // the relay never answers the group-scoped denials that would revoke
        // relay_synced and surface as app-visible group_error on reconnect.
        let translation = translator.translate(
            controlOp: "group_relay_register",
            controlPayload: #"{"group_id":"g1","members":["alice","bob","carol"],"is_admin":false}"#,
            recipientId: "bob"
        )
        guard case .replace = translation else {
            return XCTFail("register must stay a replace")
        }
        let sent = frames(translation)
        XCTAssertEqual(sent.count, 1)
        XCTAssertEqual(sent[0]["type"] as? String, "CreateGroup")

        // is_admin=true keeps the normal delta behavior.
        let admin = frames(translator.translate(
            controlOp: "group_relay_register",
            controlPayload: #"{"group_id":"g2","members":["alice","bob"],"is_admin":true}"#,
            recipientId: "bob"
        ))
        XCTAssertEqual(admin.count, 2)
        XCTAssertEqual(admin[1]["type"] as? String, "AddGroupMember")
        XCTAssertEqual(admin[1]["username"] as? String, "alice")
    }

    func testLeaveAfterRejoinSendsLeaveGroupAgain() {
        let translator = RelayControlOpTranslator(selfId: "alice", selfAddress: { nil })

        commit(translator.translate(
            controlOp: "group_mls_leave",
            controlPayload: #"{"group_id":"g1","leaving_member":"alice"}"#,
            recipientId: "bob"
        ))

        // Rejoining re-registers the group: that proves membership again and
        // must re-arm the leave dedup.
        commit(translator.translate(
            controlOp: "group_relay_register",
            controlPayload: #"{"group_id":"g1","members":["alice","bob"]}"#,
            recipientId: "alice"
        ))

        let secondLeave = translator.translate(
            controlOp: "group_mls_leave",
            controlPayload: #"{"group_id":"g1","leaving_member":"alice"}"#,
            recipientId: "bob"
        )
        guard case .tap(let leaveFrames, _) = secondLeave else {
            return XCTFail("leave after rejoin must be a tap")
        }
        XCTAssertEqual(leaveFrames.count, 1)
        XCTAssertEqual(leaveFrames[0]["type"] as? String, "LeaveGroup")
        XCTAssertEqual(leaveFrames[0]["group_id"] as? String, "g1")
    }

    func testAdminDenialDuringFlightWinsOverCommit() {
        let translator = RelayControlOpTranslator(selfId: "bob", selfAddress: { nil })

        // Frames produced, but the relay's denial lands before the caller
        // commits (delegate queue races the send chain): the commit must not
        // record the membership snapshot the relay refused.
        let inFlight = translator.translate(
            controlOp: "group_relay_register",
            controlPayload: #"{"group_id":"g1","members":["alice","bob"]}"#,
            recipientId: "bob"
        )
        translator.onGroupError(groupId: "g1", reason: "Only admins can add members")
        commit(inFlight)

        let after = frames(translator.translate(
            controlOp: "group_relay_register",
            controlPayload: #"{"group_id":"g1","members":["alice","bob"]}"#,
            recipientId: "bob"
        ))
        // Denied: CreateGroup only, and no snapshot was committed.
        XCTAssertEqual(after.count, 1)
        XCTAssertEqual(after[0]["type"] as? String, "CreateGroup")
    }

    /// Parity pin against the Rust op registry
    /// (test_internet_control_op_registry_is_closed in
    /// crates/offline-protocol/src/protocol/tests/mod.rs): every op the core
    /// can emit must translate to a relay-native shape (.replace/.tap),
    /// never .passThrough. A new Rust op must be handled here AND in the
    /// Kotlin translator AND the spec table before it ships — an unhandled
    /// op degrades to an opaque SendMessage the relay merely echoes/forwards.
    func testEveryCoreControlOpTranslatesToRelayNative() {
        let translator = RelayControlOpTranslator(selfId: "alice", selfAddress: { nil })

        // Ordered: g1 is registered before the ops that reference it.
        let cases: [(op: String, payload: String, recipient: String)] = [
            ("group_relay_register", #"{"group_id":"g1","group_name":"Trip","members":["alice","bob"]}"#, "alice"),
            ("group_relay_broadcast", #"{"group_id":"g1","ciphertext":"AAECAw==","epoch":1}"#, "alice"),
            ("group_mls_leave", #"{"group_id":"g1","leaving_member":"alice"}"#, "bob"),
        ]

        for c in cases {
            let translation = translator.translate(
                controlOp: c.op,
                controlPayload: c.payload,
                recipientId: c.recipient
            )
            commit(translation)
            XCTAssertFalse(
                isPassThrough(translation),
                "core op '\(c.op)' must translate to a relay-native shape, got passThrough"
            )
        }
    }

    func testMalformedPayloadAndUnknownOpFallBackToPassThrough() {
        let translator = RelayControlOpTranslator(selfId: "alice", selfAddress: { nil })
        XCTAssertTrue(isPassThrough(
            translator.translate(controlOp: "group_relay_broadcast", controlPayload: "not-json", recipientId: "alice")
        ))
        XCTAssertTrue(isPassThrough(
            translator.translate(controlOp: "some_future_op", controlPayload: "{}", recipientId: "bob")
        ))
        XCTAssertTrue(isPassThrough(
            translator.translate(controlOp: "group_relay_register", controlPayload: #"{"members":[]}"#, recipientId: "alice")
        ))
    }

    func testUncommittedRegistrationResendsDeltas() {
        let translator = RelayControlOpTranslator(selfId: "alice", selfAddress: { nil })

        // The frames were produced but never fully written (a best-effort
        // delta dropped): the commit must not run, so the next registration
        // re-sends the missing deltas instead of assuming them applied.
        _ = translator.translate(
            controlOp: "group_relay_register",
            controlPayload: #"{"group_id":"g1","members":["alice","bob"]}"#,
            recipientId: "alice"
        )

        let retry = frames(translator.translate(
            controlOp: "group_relay_register",
            controlPayload: #"{"group_id":"g1","members":["alice","bob"]}"#,
            recipientId: "alice"
        ))
        XCTAssertEqual(retry.count, 2)
        XCTAssertEqual(retry[1]["type"] as? String, "AddGroupMember")
        XCTAssertEqual(retry[1]["username"] as? String, "bob")
    }

    func testResetForgetsRegistrationDiffState() {
        let translator = RelayControlOpTranslator(selfId: "alice", selfAddress: { nil })
        commit(translator.translate(
            controlOp: "group_relay_register",
            controlPayload: #"{"group_id":"g1","members":["alice","bob"]}"#,
            recipientId: "alice"
        ))
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

    func testRegisterCommitAfterResetIsANoOp() {
        let translator = RelayControlOpTranslator(selfId: "alice", selfAddress: { nil })

        // A chain that settles after a disconnect reset must not commit a
        // phantom snapshot into the NEXT connection's diff base — the relay
        // never received the buffered deltas, and a poisoned base would make
        // the reconnect's register skip them permanently.
        let staleTranslation = translator.translate(
            controlOp: "group_relay_register",
            controlPayload: #"{"group_id":"g1","members":["alice","bob"]}"#,
            recipientId: "alice"
        )
        translator.reset()
        commit(staleTranslation)

        let again = frames(translator.translate(
            controlOp: "group_relay_register",
            controlPayload: #"{"group_id":"g1","members":["alice","bob"]}"#,
            recipientId: "alice"
        ))
        XCTAssertEqual(again.count, 2)
        XCTAssertEqual(again[1]["type"] as? String, "AddGroupMember")
        XCTAssertEqual(again[1]["username"] as? String, "bob")
    }

    func testLeaveCommitAfterResetIsANoOp() {
        let translator = RelayControlOpTranslator(selfId: "alice", selfAddress: { nil })

        let staleTranslation = translator.translate(
            controlOp: "group_mls_leave",
            controlPayload: #"{"group_id":"g1","leaving_member":"alice"}"#,
            recipientId: "bob"
        )
        translator.reset()
        commit(staleTranslation)

        // The stale LeaveGroup never reached the relay's registry on the
        // new connection; the dedup must not swallow the retry.
        let retry = frames(translator.translate(
            controlOp: "group_mls_leave",
            controlPayload: #"{"group_id":"g1","leaving_member":"alice"}"#,
            recipientId: "bob"
        ))
        XCTAssertEqual(retry.count, 1)
        XCTAssertEqual(retry[0]["type"] as? String, "LeaveGroup")
    }

    func testNonAdminReasonGroupErrorDoesNotSuppressDeltas() {
        let translator = RelayControlOpTranslator(selfId: "alice", selfAddress: { nil })

        // Only the relay's admin-denial wording flips the suppression, even
        // when the error correlates to outstanding deltas; an unrelated group
        // error (bad member id, transient state) must not silently stop
        // membership sync.
        commit(translator.translate(
            controlOp: "group_relay_register",
            controlPayload: #"{"group_id":"g1","members":["alice","bob"]}"#,
            recipientId: "alice"
        ))
        translator.onGroupError(groupId: "g1", reason: "User not found")

        let registration = frames(translator.translate(
            controlOp: "group_relay_register",
            controlPayload: #"{"group_id":"g1","members":["alice","bob","carol"]}"#,
            recipientId: "alice"
        ))
        XCTAssertEqual(registration.count, 2)
        XCTAssertEqual(registration[1]["type"] as? String, "AddGroupMember")
        XCTAssertEqual(registration[1]["username"] as? String, "carol")
    }

    func testUncorrelatedAdminDenialDoesNotSuppressDeltas() {
        let translator = RelayControlOpTranslator(selfId: "bob", selfAddress: { nil })

        // An admin-denial GroupError with NO member deltas outstanding from
        // this translator answers someone else's operation (an app
        // raw-channel op, another admin's edit) — honoring it would
        // permanently silence membership sync for the group.
        translator.onGroupError(groupId: "g1", reason: "Only admins can add members")

        let registration = frames(translator.translate(
            controlOp: "group_relay_register",
            controlPayload: #"{"group_id":"g1","members":["alice","bob"]}"#,
            recipientId: "bob"
        ))
        XCTAssertEqual(registration.count, 2)
        XCTAssertEqual(registration[1]["type"] as? String, "AddGroupMember")
        XCTAssertEqual(registration[1]["username"] as? String, "alice")
    }

    func testRequestIdCarryingDenialIsNeverOurs() {
        let translator = RelayControlOpTranslator(selfId: "bob", selfAddress: { nil })

        // The translator never tags request_id, so a request_id-echoing
        // GroupError answers an app raw-channel frame — even mid-window it
        // must neither suppress deltas nor consume the window.
        _ = translator.translate(
            controlOp: "group_relay_register",
            controlPayload: #"{"group_id":"g1","members":["alice","bob"]}"#,
            recipientId: "bob"
        )
        translator.onGroupError(
            groupId: "g1",
            reason: "Only admins can add members",
            requestId: "req-42"
        )

        // Not suppressed: the uncommitted registration re-sends its delta.
        let retry = frames(translator.translate(
            controlOp: "group_relay_register",
            controlPayload: #"{"group_id":"g1","members":["alice","bob"]}"#,
            recipientId: "bob"
        ))
        XCTAssertEqual(retry.count, 2)
        XCTAssertEqual(retry[1]["type"] as? String, "AddGroupMember")

        // The window survived the disowned error: a real (request_id-less)
        // denial still lands.
        translator.onGroupError(groupId: "g1", reason: "Only admins can add members")
        XCTAssertEqual(
            frames(translator.translate(
                controlOp: "group_relay_register",
                controlPayload: #"{"group_id":"g1","members":["alice","bob"]}"#,
                recipientId: "bob"
            )).count,
            1
        )
    }

    func testDeltaWindowClosesOnFirstGroupScopedAnswer() {
        let translator = RelayControlOpTranslator(selfId: "bob", selfAddress: { nil })

        // One answer per window: the first group-scoped GroupError closes it,
        // so a later admin-denial with nothing outstanding is uncorrelated
        // and falls back to send-and-learn (noisy but safe) instead of
        // suppressing on someone else's error.
        commit(translator.translate(
            controlOp: "group_relay_register",
            controlPayload: #"{"group_id":"g1","members":["alice","bob"]}"#,
            recipientId: "bob"
        ))
        translator.onGroupError(groupId: "g1", reason: "User not found")
        translator.onGroupError(groupId: "g1", reason: "Only admins can add members")

        let registration = frames(translator.translate(
            controlOp: "group_relay_register",
            controlPayload: #"{"group_id":"g1","members":["alice","bob","carol"]}"#,
            recipientId: "bob"
        ))
        XCTAssertEqual(registration.count, 2)
        XCTAssertEqual(registration[1]["type"] as? String, "AddGroupMember")
        XCTAssertEqual(registration[1]["username"] as? String, "carol")
    }

    func testSuccessAnswerClosesTheDenialWindow() {
        let translator = RelayControlOpTranslator(selfId: "bob", selfAddress: { nil })

        // The register succeeded (GroupCreated answered it): the success
        // answer must close the delta window too, or it stays armed for the
        // rest of the connection and a later unrelated error quoting the
        // denial phrase (e.g. a user-authored group name) would be honored
        // as OUR denial and suppress membership sync.
        commit(translator.translate(
            controlOp: "group_relay_register",
            controlPayload: #"{"group_id":"g1","members":["alice","bob"]}"#,
            recipientId: "bob"
        ))
        translator.onGroupAnswered(groupId: "g1")
        translator.onGroupError(groupId: "g1", reason: "Only admins can add members")

        let registration = frames(translator.translate(
            controlOp: "group_relay_register",
            controlPayload: #"{"group_id":"g1","members":["alice","bob","carol"]}"#,
            recipientId: "bob"
        ))
        XCTAssertEqual(registration.count, 2)
        XCTAssertEqual(registration[1]["type"] as? String, "AddGroupMember")
        XCTAssertEqual(registration[1]["username"] as? String, "carol")
    }

    func testResetClosesTheDenialWindow() {
        let translator = RelayControlOpTranslator(selfId: "bob", selfAddress: { nil })

        _ = translator.translate(
            controlOp: "group_relay_register",
            controlPayload: #"{"group_id":"g1","members":["alice","bob"]}"#,
            recipientId: "bob"
        )
        translator.reset()

        // The outstanding delta died with the connection: a phrase-quoting
        // error on the next connection is not its answer.
        translator.onGroupError(groupId: "g1", reason: "Only admins can add members")

        let after = frames(translator.translate(
            controlOp: "group_relay_register",
            controlPayload: #"{"group_id":"g1","members":["alice","bob"]}"#,
            recipientId: "bob"
        ))
        XCTAssertEqual(after.count, 2)
        XCTAssertEqual(after[1]["type"] as? String, "AddGroupMember")
    }

    func testStringIsAdminHintTreatedAsAbsent() {
        let translator = RelayControlOpTranslator(selfId: "bob", selfAddress: { nil })

        // A string encoding is not boolean-like on either platform: treated
        // as an absent hint (send-and-learn), not as a denial — mirrors the
        // string half of Kotlin's numericAndStringIsAdminHintsMatchSwiftSemantics.
        let stringFalse = frames(translator.translate(
            controlOp: "group_relay_register",
            controlPayload: #"{"group_id":"g3","members":["alice","bob"],"is_admin":"false"}"#,
            recipientId: "bob"
        ))
        XCTAssertEqual(stringFalse.count, 2)
    }

    func testIsAdminHintAcceptsNumericEncoding() {
        let translator = RelayControlOpTranslator(selfId: "bob", selfAddress: { nil })

        // JSON 0/1 arrive as NSNumber integers; the hint must behave like
        // the boolean encoding on both platforms (NSNumber boolValue).
        let hinted = frames(translator.translate(
            controlOp: "group_relay_register",
            controlPayload: #"{"group_id":"g1","members":["alice","bob"],"is_admin":0}"#,
            recipientId: "bob"
        ))
        XCTAssertEqual(hinted.count, 1)
        XCTAssertEqual(hinted[0]["type"] as? String, "CreateGroup")

        let admin = frames(translator.translate(
            controlOp: "group_relay_register",
            controlPayload: #"{"group_id":"g2","members":["alice","bob"],"is_admin":1}"#,
            recipientId: "bob"
        ))
        XCTAssertEqual(admin.count, 2)
        XCTAssertEqual(admin[1]["type"] as? String, "AddGroupMember")
    }

    func testRolePromotionReenablesMemberDeltas() {
        let translator = RelayControlOpTranslator(selfId: "bob", selfAddress: { nil })
        // The denial must correlate to deltas this translator sent.
        commit(translator.translate(
            controlOp: "group_relay_register",
            controlPayload: #"{"group_id":"g1","members":["alice","bob"]}"#,
            recipientId: "bob"
        ))
        translator.onGroupError(groupId: "g1", reason: "Only admins can add members")

        // Denied: registration is CreateGroup only.
        let denied = frames(translator.translate(
            controlOp: "group_relay_register",
            controlPayload: #"{"group_id":"g1","members":["alice","bob"]}"#,
            recipientId: "bob"
        ))
        XCTAssertEqual(denied.count, 1)

        // Someone else's promotion — or a demotion — changes nothing.
        translator.onRoleChanged(groupId: "g1", member: "carol", newRole: "admin")
        translator.onRoleChanged(groupId: "g1", member: "bob", newRole: "member")
        XCTAssertEqual(
            frames(translator.translate(
                controlOp: "group_relay_register",
                controlPayload: #"{"group_id":"g1","members":["alice","bob"]}"#,
                recipientId: "bob"
            )).count,
            1
        )

        // This device's promotion to admin re-enables the deltas.
        translator.onRoleChanged(groupId: "g1", member: "bob", newRole: "admin")
        let promoted = frames(translator.translate(
            controlOp: "group_relay_register",
            controlPayload: #"{"group_id":"g1","members":["alice","bob"]}"#,
            recipientId: "bob"
        ))
        XCTAssertEqual(promoted.count, 2)
        XCTAssertEqual(promoted[1]["type"] as? String, "AddGroupMember")
        XCTAssertEqual(promoted[1]["username"] as? String, "alice")
    }

    // MARK: - Self-identity across both namespaces
    //
    // Core-fed payloads (members, leaving_member) name this device by its
    // derived `off1…` address; relay-fed answers (GroupRoleChanged) name it
    // by account name. Every test above pins the profile half with a nil
    // address; these pin the address half and the interaction.

    private static let selfAddr = "off1qqqqself0000000000000000000000000000000000000000000000000"
    private static let peerAddr = "off1qqqqpeer00000000000000000000000000000000000000000000000000"

    /// The registration roster is the MLS roster — addresses. Filtering it
    /// against the profile alone strips nothing, so the SDK emitted an
    /// AddGroupMember naming its own address: self-add, wrong under every
    /// namespace the relay might settle on.
    func testRegisterStripsSelfByAddressNotJustProfile() {
        let translator = RelayControlOpTranslator(
            selfId: "alice",
            selfAddress: { Self.selfAddr }
        )

        let registration = translator.translate(
            controlOp: "group_relay_register",
            controlPayload: #"{"group_id":"g1","members":["\#(Self.selfAddr)","\#(Self.peerAddr)"]}"#,
            recipientId: Self.selfAddr
        )
        let sent = frames(registration)
        let delta = sent.dropFirst().map { $0["username"] as? String ?? "" }
        XCTAssertEqual(
            delta, [Self.peerAddr],
            "self must be stripped from the roster by address; only the peer is a real delta"
        )
        XCTAssertFalse(
            delta.contains(Self.selfAddr),
            "the SDK must never send an AddGroupMember naming its own address"
        )
    }

    /// The profile half must survive the address half: a roster still naming
    /// this device by profile (a relay-shaped roster) strips too.
    func testRegisterStillStripsSelfByProfileWhenAddressIsKnown() {
        let translator = RelayControlOpTranslator(
            selfId: "alice",
            selfAddress: { Self.selfAddr }
        )
        let sent = frames(translator.translate(
            controlOp: "group_relay_register",
            controlPayload: #"{"group_id":"g1","members":["alice","bob"]}"#,
            recipientId: "alice"
        ))
        XCTAssertEqual(sent.dropFirst().map { $0["username"] as? String ?? "" }, ["bob"])
    }

    /// `leaving_member` is the core's local_id — an address. Without the
    /// address half this never fired, so the relay kept us in the group
    /// registry after we left and went on fanning the group out to us.
    func testLeaveFiresWhenLeavingMemberIsOurAddress() {
        let translator = RelayControlOpTranslator(
            selfId: "alice",
            selfAddress: { Self.selfAddr }
        )

        let tap = translator.translate(
            controlOp: "group_mls_leave",
            controlPayload: #"{"group_id":"g1","leaving_member":"\#(Self.selfAddr)"}"#,
            recipientId: Self.peerAddr
        )
        guard case .tap(let leaveFrames, _) = tap else {
            return XCTFail("leave must be a tap")
        }
        XCTAssertEqual(leaveFrames.count, 1, "our own leave must produce a relay-native LeaveGroup")
        XCTAssertEqual(leaveFrames[0]["type"] as? String, "LeaveGroup")
        commit(tap)

        // Another member's leave — named by address — is still not ours to
        // send. Widening the match must not have widened it to everyone.
        let other = translator.translate(
            controlOp: "group_mls_leave",
            controlPayload: #"{"group_id":"g2","leaving_member":"\#(Self.peerAddr)"}"#,
            recipientId: Self.peerAddr
        )
        guard case .tap(let otherFrames, _) = other else {
            return XCTFail("third-party leave must stay a tap")
        }
        XCTAssertTrue(
            otherFrames.isEmpty,
            "a peer's address must never be read as self — that would deregister us from a group we are still in"
        )
    }

    /// The relay names the promoted account by username, so the profile half
    /// carries this site; the address half is forward-compatibility for a
    /// relay whose group path later moves to address space.
    func testRolePromotionMatchesSelfInEitherNamespace() {
        for (label, promoted) in [("profile", "bob"), ("address", Self.selfAddr)] {
            let translator = RelayControlOpTranslator(
                selfId: "bob",
                selfAddress: { Self.selfAddr }
            )
            commit(translator.translate(
                controlOp: "group_relay_register",
                controlPayload: #"{"group_id":"g1","members":["alice","bob"]}"#,
                recipientId: "bob"
            ))
            translator.onGroupError(groupId: "g1", reason: "Only admins can add members")
            XCTAssertEqual(
                frames(translator.translate(
                    controlOp: "group_relay_register",
                    controlPayload: #"{"group_id":"g1","members":["alice","bob"]}"#,
                    recipientId: "bob"
                )).count,
                1,
                "\(label): denial must suppress deltas first"
            )

            translator.onRoleChanged(groupId: "g1", member: promoted, newRole: "admin")
            XCTAssertEqual(
                frames(translator.translate(
                    controlOp: "group_relay_register",
                    controlPayload: #"{"group_id":"g1","members":["alice","bob"]}"#,
                    recipientId: "bob"
                )).count,
                2,
                "\(label): our own promotion must re-enable member deltas"
            )
        }
    }

    /// An unrelated account's promotion must not re-enable our deltas, in
    /// either namespace.
    func testRolePromotionOfAnotherAddressIsIgnored() {
        let translator = RelayControlOpTranslator(
            selfId: "bob",
            selfAddress: { Self.selfAddr }
        )
        commit(translator.translate(
            controlOp: "group_relay_register",
            controlPayload: #"{"group_id":"g1","members":["alice","bob"]}"#,
            recipientId: "bob"
        ))
        translator.onGroupError(groupId: "g1", reason: "Only admins can add members")

        translator.onRoleChanged(groupId: "g1", member: Self.peerAddr, newRole: "admin")
        XCTAssertEqual(
            frames(translator.translate(
                controlOp: "group_relay_register",
                controlPayload: #"{"group_id":"g1","members":["alice","bob"]}"#,
                recipientId: "bob"
            )).count,
            1,
            "another member's promotion must leave our denial in place"
        )
    }

    /// Before MLS identity exists (encryption disabled, or pre-init) the
    /// provider returns nil and the translator must behave exactly as it did
    /// when it only knew the profile — no crash, no accidental self-match.
    func testAbsentAddressDegradesToProfileOnlyMatching() {
        for provider in [{ nil as String? }, { "" as String? }] {
            let translator = RelayControlOpTranslator(selfId: "alice", selfAddress: provider)
            let sent = frames(translator.translate(
                controlOp: "group_relay_register",
                controlPayload: #"{"group_id":"g1","members":["alice","bob"]}"#,
                recipientId: "alice"
            ))
            XCTAssertEqual(sent.dropFirst().map { $0["username"] as? String ?? "" }, ["bob"])

            // An empty roster id must not match an empty address. (Empty
            // members are dropped upstream too — this pins both guards.)
            let withEmpty = frames(translator.translate(
                controlOp: "group_relay_register",
                controlPayload: #"{"group_id":"g2","members":["alice","","bob"]}"#,
                recipientId: "alice"
            ))
            XCTAssertEqual(withEmpty.dropFirst().map { $0["username"] as? String ?? "" }, ["bob"])
        }
    }

    /// The address is resolved per call, never captured at construction:
    /// the translator outlives MLS init and identity rebuilds.
    func testAddressIsResolvedPerCallNotCachedAtConstruction() {
        var current: String? = nil
        let translator = RelayControlOpTranslator(selfId: "alice", selfAddress: { current })

        // Pre-identity: the address is unknown, so it cannot be stripped.
        let before = frames(translator.translate(
            controlOp: "group_relay_register",
            controlPayload: #"{"group_id":"g1","members":["\#(Self.selfAddr)","\#(Self.peerAddr)"]}"#,
            recipientId: "alice"
        ))
        XCTAssertEqual(before.count, 3, "unknown address cannot be filtered out")

        // MLS init lands: the very next translation must see it.
        current = Self.selfAddr
        let after = frames(translator.translate(
            controlOp: "group_relay_register",
            controlPayload: #"{"group_id":"g2","members":["\#(Self.selfAddr)","\#(Self.peerAddr)"]}"#,
            recipientId: "alice"
        ))
        XCTAssertEqual(after.dropFirst().map { $0["username"] as? String ?? "" }, [Self.peerAddr])
    }

    func testServerPlaneFirewallBlocksRelayAnswerPrefixesOnly() {
        // Relay-answer frames a peer must never originate: the core trusts
        // these from the internet path (__GROUP_CREATED__ can mark a group
        // relay-synced), and the relay forwards peer content verbatim.
        let forged = [
            #"__GROUP_CREATED__{"group_id":"g1","name":"x"}"#,
            #"__GROUP_MEMBER_ADDED__{"group_id":"g1","user_id":"mallory"}"#,
            #"__GROUP_MEMBER_REMOVED__{"group_id":"g1","user_id":"bob"}"#,
            #"__GROUP_INFO__{"group_id":"g1"}"#,
            #"__USER_GROUPS__{"groups":[]}"#,
            #"__GROUP_ERROR__{"reason":"x","group_id":"g1"}"#
        ]
        for frame in forged {
            XCTAssertTrue(
                RelayControlOpTranslator.isForgedServerPlaneAnswer(frame),
                "expected forged frame to be blocked: \(frame)"
            )
        }

        // Legitimate peer traffic must keep flowing — group fan-out,
        // typing, MLS control, plain text, and prefix-shaped user content
        // that is not an exact server-plane prefix.
        let legit = [
            #"__GROUP_MSG__{"group_id":"g1","content":"c"}"#,
            #"__TYPING__{"conversation_id":"c1","is_typing":true}"#,
            "__GRP_MLS_WELCOME__abc",
            #"__CONN_REQ__{"sender_name":"bob"}"#,
            "hello __GROUP_CREATED__ mid-string",
            "__GROUP_CREATED_X__ not the prefix",
            ""
        ]
        for frame in legit {
            XCTAssertFalse(
                RelayControlOpTranslator.isForgedServerPlaneAnswer(frame),
                "expected legitimate frame to pass: \(frame)"
            )
        }
    }
}
