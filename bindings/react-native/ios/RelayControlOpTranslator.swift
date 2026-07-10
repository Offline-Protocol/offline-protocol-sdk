//
// RelayControlOpTranslator.swift
// OfflineProtocol
//
// Translates the SDK's server-plane control frames (tagged by the core via
// InternetMessage.controlOp) into the relay's native JSON protocol.
// Mirrors android/.../RelayControlOpTranslator.kt — keep the two in sync.
//
// The relay does not intercept content prefixes — a self-addressed
// __GRP_RELAY_REG__/__GRP_RELAY_BCAST__ frame sent as an opaque SendMessage
// is just echoed back — so this translator is the "relay adapter" the core's
// relay-optimized group path was designed against.
//
// Known v1 limitation: a group rename re-registers, but the relay's
// idempotent sync never updates the stored name (ON CONFLICT DO NOTHING).
//

import Foundation

final class RelayControlOpTranslator {

    /// Cross-layer contract: substring of the relay server's admin-denial
    /// GroupError reasons ("Only admins can add members" / "Only admins can
    /// remove members"). If the relay rewords these, non-admin devices
    /// without the core's `is_admin` hint fall back to re-learning the
    /// denial each connection — noisy but safe. Keep in sync with the relay
    /// source (see docs/relay-transport-parity-spec.md).
    static let adminDeniedReasonMarker = "Only admins"

    enum Translation {
        /// Send these relay-native frames instead of the original message.
        /// Invoke the commit ONLY after every frame was written to the
        /// socket: it publishes the translator's optimistic state
        /// (registration diff base). Skipping it on a partial write makes
        /// the next register re-send the missing deltas instead of assuming
        /// them applied — otherwise that member is silently missing from
        /// relay fan-out for the rest of the connection.
        case replace([[String: Any]], (() -> Void)?)
        /// Send the original message verbatim, then these frames best-effort.
        /// Same commit contract as `replace`, covering the LeaveGroup dedup.
        case tap([[String: Any]], (() -> Void)?)
        /// No translation — send the original message verbatim.
        case passThrough
    }

    private let selfId: String
    /// Last membership committed as registered with the relay, per group.
    private var registeredMembers: [String: Set<String>] = [:]
    /// Groups whose member deltas the relay denied (we are not the admin).
    private var memberDeltasDenied: Set<String> = []
    /// Groups for which a relay-native LeaveGroup was already committed.
    private var leaveSent: Set<String> = []
    private let lock = NSLock()

    init(selfId: String) {
        self.selfId = selfId
    }

    func translate(controlOp: String, controlPayload: String, recipientId: String) -> Translation {
        lock.lock()
        defer { lock.unlock() }

        let payload = parseJson(controlPayload)

        switch controlOp {
        case "conn_req":
            guard let payload = payload else { return .passThrough }
            var frame: [String: Any] = [
                "type": "SendConnectionRequest",
                "recipient": recipientId,
                "sender_name": (payload["sender_name"] as? String) ?? selfId
            ]
            if let keyPackage = payload["key_package"] as? [Any] {
                frame["key_package"] = keyPackage
            }
            return .replace([frame], nil)

        case "conn_acc":
            guard let payload = payload else { return .passThrough }
            var frame: [String: Any] = [
                "type": "AcceptConnectionRequest",
                "requester_id": recipientId,
                "accepter_name": (payload["accepted_by_name"] as? String) ?? selfId
            ]
            if let keyPackage = payload["key_package"] as? [Any] {
                frame["key_package"] = keyPackage
            }
            return .replace([frame], nil)

        case "conn_rej":
            return .replace([[
                "type": "RejectConnectionRequest",
                "requester_id": recipientId
            ]], nil)

        case "conn_can":
            return .replace([[
                "type": "CancelConnectionRequest",
                "recipient": recipientId
            ]], nil)

        case "group_relay_register":
            guard let payload = payload,
                  let groupId = payload["group_id"] as? String, !groupId.isEmpty else {
                return .passThrough
            }
            return translateRegisterLocked(groupId: groupId, payload: payload)

        case "group_relay_broadcast":
            guard let payload = payload,
                  let groupId = payload["group_id"] as? String, !groupId.isEmpty else {
                return .passThrough
            }
            var frame: [String: Any] = [
                "type": "SendGroupMessage",
                "group_id": groupId,
                "content": (payload["ciphertext"] as? String) ?? ""
            ]
            if let replyTo = payload["reply_to"] as? String, !replyTo.isEmpty {
                frame["reply_to_msg"] = replyTo
            }
            return .replace([frame], nil)

        case "group_mls_leave":
            guard let payload = payload,
                  let groupId = payload["group_id"] as? String, !groupId.isEmpty,
                  let leavingMember = payload["leaving_member"] as? String,
                  leavingMember == selfId,
                  !leaveSent.contains(groupId) else {
                return .tap([], nil)
            }
            return .tap([[
                "type": "LeaveGroup",
                "group_id": groupId
            ]], { [weak self] in self?.commitLeave(groupId: groupId) })

        default:
            return .passThrough
        }
    }

    /// Feed relay GroupError answers so admin-denied groups stop producing member deltas.
    func onGroupError(groupId: String, reason: String) {
        guard !groupId.isEmpty else { return }
        lock.lock()
        defer { lock.unlock() }
        if reason.range(of: Self.adminDeniedReasonMarker, options: .caseInsensitive) != nil {
            memberDeltasDenied.insert(groupId)
            // The optimistic membership snapshot was not applied server-side.
            registeredMembers.removeValue(forKey: groupId)
        }
    }

    /// Per-connection state: call on disconnect.
    func reset() {
        lock.lock()
        defer { lock.unlock() }
        registeredMembers.removeAll()
        memberDeltasDenied.removeAll()
        leaveSent.removeAll()
    }

    private func translateRegisterLocked(groupId: String, payload: [String: Any]) -> Translation {
        let rawName = payload["group_name"] as? String
        let name = rawName.flatMap { $0.isEmpty ? nil : $0 } ?? groupId
        let members = (payload["members"] as? [Any])?
            .compactMap { $0 as? String }
            .filter { !$0.isEmpty } ?? []

        // A register proves membership again: a rejoin after a committed
        // leave must be allowed to send LeaveGroup again later.
        leaveSent.remove(groupId)

        // The core's admin hint: explicitly-not-admin devices never send
        // member deltas (the relay would deny each with a group-scoped
        // GroupError). Absent hint = unknown, fall back to send-and-learn.
        let notAdmin = (payload["is_admin"] as? Bool) == false

        var frames: [[String: Any]] = [[
            "type": "CreateGroup",
            "group_id": groupId,
            "name": name
        ]]

        // Member deltas: the relay adds the creator itself, and self-adds are
        // redundant, so the self id never appears in a delta. Sorted for a
        // deterministic wire order across platforms.
        var commit: (() -> Void)? = nil
        if !notAdmin && !memberDeltasDenied.contains(groupId) {
            let desired = Set(members.filter { $0 != selfId })
            let known = registeredMembers[groupId] ?? []
            for added in desired.subtracting(known).sorted() {
                frames.append([
                    "type": "AddGroupMember",
                    "group_id": groupId,
                    "username": added
                ])
            }
            for removed in known.subtracting(desired).sorted() {
                frames.append([
                    "type": "RemoveGroupMember",
                    "group_id": groupId,
                    "username": removed
                ])
            }
            // Committed only after the frames are actually written.
            commit = { [weak self] in
                self?.commitRegisteredMembers(groupId: groupId, members: desired)
            }
        }
        return .replace(frames, commit)
    }

    private func commitRegisteredMembers(groupId: String, members: Set<String>) {
        lock.lock()
        defer { lock.unlock() }
        // A GroupError may have marked the group admin-denied while the
        // frames were in flight; the denial wins.
        if !memberDeltasDenied.contains(groupId) {
            registeredMembers[groupId] = members
        }
    }

    private func commitLeave(groupId: String) {
        lock.lock()
        defer { lock.unlock() }
        leaveSent.insert(groupId)
        registeredMembers.removeValue(forKey: groupId)
        memberDeltasDenied.remove(groupId)
    }

    private func parseJson(_ string: String) -> [String: Any]? {
        guard !string.isEmpty,
              let data = string.data(using: .utf8),
              let json = try? JSONSerialization.jsonObject(with: data) as? [String: Any] else {
            return nil
        }
        return json
    }
}
