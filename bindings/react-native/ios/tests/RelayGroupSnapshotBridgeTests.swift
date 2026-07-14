//
// RelayGroupSnapshotBridgeTests.swift
//
// Pins lossless GroupInfo/UserGroups dual emission. Mirrors Android's
// RelayGroupSnapshotBridgeTest -- keep in sync.
//

import XCTest
@testable import OfflineProtocol

final class RelayGroupSnapshotBridgeTests: XCTestCase {
    func testGroupInfoEmitsTypedProjectionAndVerbatimRawFrame() throws {
        let raw = #"""
        {
          "future_top_level": {"ratio": 25.0},
          "pending_join_requests": [
            {"token":"invite-token","joiner_username":"dora","key_package":[1,2,3],"timestamp":"2026-07-14T10:00:00Z","future_request_field":{"v":1}}
          ],
          "avatar_url": "https://cdn.example/group.png",
          "members": [
            {"user_id":"alice","role":"admin","joined_at":"2026-07-01T00:00:00Z","profile":{"display_name":"Alice"}},
            "malformed-entry",
            {"user_id":"bob","role":"member","joined_at":"2026-07-02T00:00:00Z","future_member_field":true},
            {"user_id":"charlie","joined_at":"2026-07-03T00:00:00Z"},
            {"role":"member"}
          ],
          "description": "Planning group",
          "created_at": "2026-07-01T00:00:00Z",
          "created_by": "alice",
          "name": "Trip",
          "group_id": "g1",
          "type": "GroupInfo"
        }
        """#
        let parsed = try XCTUnwrap(
            JSONSerialization.jsonObject(with: Data(raw.utf8)) as? [String: Any]
        )
        var typed: [(prefix: String, payload: [String: Any])] = []
        var rawFrames: [String] = []

        let handled = RelayGroupSnapshotBridge.dispatch(
            messageType: "GroupInfo",
            json: parsed,
            rawText: raw,
            emitTyped: { typed.append(($0, $1)) },
            emitRaw: { rawFrames.append($0) }
        )

        XCTAssertTrue(handled)
        XCTAssertEqual(typed.count, 1)
        XCTAssertEqual(typed[0].prefix, "__GROUP_INFO__")
        XCTAssertEqual(typed[0].payload["group_id"] as? String, "g1")
        XCTAssertEqual(typed[0].payload["name"] as? String, "Trip")
        let members = try XCTUnwrap(typed[0].payload["members"] as? [[String: String]])
        XCTAssertEqual(members.count, 3)
        XCTAssertEqual(members[0]["user_id"], "alice")
        XCTAssertEqual(members[0]["role"], "admin")
        XCTAssertEqual(members[1]["user_id"], "bob")
        XCTAssertEqual(members[1]["role"], "member")
        XCTAssertEqual(members[2]["user_id"], "charlie")
        XCTAssertEqual(members[2]["role"], "member")

        XCTAssertEqual(rawFrames.count, 1)
        XCTAssertEqual(Data(rawFrames[0].utf8), Data(raw.utf8))
        let forwarded = try XCTUnwrap(
            JSONSerialization.jsonObject(with: Data(rawFrames[0].utf8)) as? [String: Any]
        )
        XCTAssertEqual(forwarded["description"] as? String, "Planning group")
        XCTAssertEqual(forwarded["avatar_url"] as? String, "https://cdn.example/group.png")
        XCTAssertNotNil(forwarded["future_top_level"])
        let pending = try XCTUnwrap((forwarded["pending_join_requests"] as? [[String: Any]])?.first)
        XCTAssertEqual(pending["token"] as? String, "invite-token")
        XCTAssertEqual(pending["joiner_username"] as? String, "dora")
        XCTAssertEqual((pending["key_package"] as? [Any])?.count, 3)
        XCTAssertEqual(pending["timestamp"] as? String, "2026-07-14T10:00:00Z")
        XCTAssertNotNil(pending["future_request_field"])
        let rawMembers = try XCTUnwrap(forwarded["members"] as? [Any])
        let firstMember = try XCTUnwrap(rawMembers.first as? [String: Any])
        XCTAssertNotNil(firstMember["profile"])
    }

    func testUserGroupsEmitsTypedProjectionAndPreservesProfileMembershipAndExtensions() throws {
        let raw = #"""
        {
          "type": "UserGroups",
          "profile": {"username":"alice","avatar_url":"https://cdn.example/alice.png","future_profile_field":25.0},
          "groups": [
            {"group_id":"g1","name":"Trip","created_at":"2026-07-01T00:00:00Z","membership":{"role":"admin","joined_at":"2026-07-01T00:00:00Z"},"future_group_field":{"enabled":true}},
            {"group_id":"g2","name":"Work","created_at":"2026-07-02T00:00:00Z","membership":{"role":"member","joined_at":"2026-07-03T00:00:00Z"}}
          ],
          "future_top_level": ["kept"]
        }
        """#
        let parsed = try XCTUnwrap(
            JSONSerialization.jsonObject(with: Data(raw.utf8)) as? [String: Any]
        )
        var typed: [(prefix: String, payload: [String: Any])] = []
        var rawFrames: [String] = []

        let handled = RelayGroupSnapshotBridge.dispatch(
            messageType: "UserGroups",
            json: parsed,
            rawText: raw,
            emitTyped: { typed.append(($0, $1)) },
            emitRaw: { rawFrames.append($0) }
        )

        XCTAssertTrue(handled)
        XCTAssertEqual(typed.count, 1)
        XCTAssertEqual(typed[0].prefix, "__USER_GROUPS__")
        let groups = try XCTUnwrap(typed[0].payload["groups"] as? [[String: String]])
        XCTAssertEqual(groups.count, 2)
        XCTAssertEqual(groups[0]["group_id"], "g1")
        XCTAssertEqual(groups[0]["name"], "Trip")
        XCTAssertEqual(groups[1]["group_id"], "g2")

        XCTAssertEqual(rawFrames.count, 1)
        XCTAssertEqual(Data(rawFrames[0].utf8), Data(raw.utf8))
        let forwarded = try XCTUnwrap(
            JSONSerialization.jsonObject(with: Data(rawFrames[0].utf8)) as? [String: Any]
        )
        XCTAssertNotNil(forwarded["profile"])
        XCTAssertNotNil(forwarded["future_top_level"])
        let rawGroups = try XCTUnwrap(forwarded["groups"] as? [[String: Any]])
        let membership = try XCTUnwrap(rawGroups[0]["membership"] as? [String: Any])
        XCTAssertEqual(membership["role"] as? String, "admin")
        XCTAssertNotNil(rawGroups[0]["future_group_field"])
    }

    func testMalformedRecognizedSnapshotsKeepExistingNoEventBehavior() {
        var typedCount = 0
        var rawCount = 0
        let emitTyped: (String, [String: Any]) -> Void = { _, _ in typedCount += 1 }
        let emitRaw: (String) -> Void = { _ in rawCount += 1 }

        XCTAssertTrue(RelayGroupSnapshotBridge.dispatch(
            messageType: "GroupInfo",
            json: ["type": "GroupInfo", "group_id": "", "description": "kept nowhere"],
            rawText: "group-info-raw",
            emitTyped: emitTyped,
            emitRaw: emitRaw
        ))
        XCTAssertTrue(RelayGroupSnapshotBridge.dispatch(
            messageType: "UserGroups",
            json: ["type": "UserGroups", "groups": ["not": "an array"]],
            rawText: "user-groups-raw",
            emitTyped: emitTyped,
            emitRaw: emitRaw
        ))

        XCTAssertEqual(typedCount, 0)
        XCTAssertEqual(rawCount, 0)
    }

    func testUnrelatedFramesAreNotClaimed() {
        let handled = RelayGroupSnapshotBridge.dispatch(
            messageType: "GroupError",
            json: ["type": "GroupError", "reason": "nope"],
            rawText: "raw",
            emitTyped: { _, _ in XCTFail("unexpected typed emission") },
            emitRaw: { _ in XCTFail("unexpected raw emission") }
        )

        XCTAssertFalse(handled)
    }
}
