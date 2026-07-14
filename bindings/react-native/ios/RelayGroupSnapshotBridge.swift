//
// RelayGroupSnapshotBridge.swift
// OfflineProtocol
//
// Projects relay group snapshots into the SDK's stable typed payloads while
// also forwarding the original frame for application-owned extensions.
// Mirrors android/.../RelayGroupSnapshotBridge.kt -- keep in sync.
//

import Foundation

enum RelayGroupSnapshotBridge {
    @discardableResult
    static func dispatch(
        messageType: String,
        json: [String: Any],
        rawText: String,
        emitTyped: (_ prefix: String, _ payload: [String: Any]) -> Void,
        emitRaw: (String) -> Void
    ) -> Bool {
        switch messageType {
        case "GroupInfo":
            guard let groupId = json["group_id"] as? String, !groupId.isEmpty else {
                return true
            }

            let membersRaw = json["members"] as? [Any] ?? []
            // Preserve the bridge's existing per-entry tolerance: malformed
            // entries never discard the valid members.
            let members: [[String: String]] = membersRaw.compactMap { raw in
                guard let member = raw as? [String: Any],
                      let memberId = member["user_id"] as? String,
                      !memberId.isEmpty else { return nil }
                return [
                    "user_id": memberId,
                    "role": member["role"] as? String ?? "member",
                    "joined_at": member["joined_at"] as? String ?? ""
                ]
            }

            emitTyped("__GROUP_INFO__", [
                "group_id": groupId,
                "name": json["name"] as? String ?? "",
                "created_by": json["created_by"] as? String ?? "",
                "created_at": json["created_at"] as? String ?? "",
                "members": members
            ])
            // rawText -- not a re-serialized dictionary -- is the contract.
            emitRaw(rawText)
            return true

        case "UserGroups":
            guard let groupsRaw = json["groups"] as? [Any] else {
                return true
            }

            // Preserve the bridge's existing per-entry tolerance.
            let groups: [[String: String]] = groupsRaw.compactMap { raw in
                guard let group = raw as? [String: Any],
                      let groupId = group["group_id"] as? String,
                      !groupId.isEmpty else { return nil }
                return [
                    "group_id": groupId,
                    "name": group["name"] as? String ?? "",
                    "created_at": group["created_at"] as? String ?? ""
                ]
            }

            emitTyped("__USER_GROUPS__", ["groups": groups])
            emitRaw(rawText)
            return true

        default:
            return false
        }
    }
}
