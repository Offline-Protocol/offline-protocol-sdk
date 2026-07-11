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
// State commits are deferred: a translation's commit must be invoked by the
// caller ONLY after every frame was written to the socket. Commits are
// additionally generation-guarded: a commit whose translation predates a
// reset() is a no-op, so a chain that settles after a disconnect (or
// RateLimited) cannot write a phantom snapshot into the next connection's
// diff base. A GroupRoleChanged promotion of this device (onRoleChanged)
// re-enables member deltas an earlier admin denial suppressed.
//
// Known v1 limitation: a group rename re-registers, but the relay's
// idempotent sync never updates the stored name (ON CONFLICT DO NOTHING).
//

import Foundation

final class RelayControlOpTranslator {

    /// Cross-layer contract: substring of the relay server's admin-denial
    /// GroupError reasons ("Only admins can add members" / "Only admins can
    /// remove members"). The marker alone is NOT enough to suppress deltas —
    /// it only counts when the GroupError correlates to member deltas this
    /// translator actually has outstanding (see `onGroupError`), so an
    /// admin-denial answering some other actor's operation can never
    /// permanently silence membership sync. If the relay rewords these,
    /// non-admin devices without the core's `is_admin` hint fall back to
    /// re-learning the denial each connection — noisy but safe. Keep in sync
    /// with the relay source (see docs/relay-transport-parity-spec.md).
    static let adminDeniedReasonMarker = "Only admins"

    /// SDK content prefixes only the relay server (bridged by
    /// InternetManager's injectGroupInternalMessage) may originate — never a
    /// peer. The relay forwards peer message content verbatim, so without
    /// this firewall any peer could deliver a crafted `__GROUP_CREATED__`
    /// over the authenticated internet path and (in concert with a spoofed
    /// registration window) mark a group relay-synced against a relay that
    /// never registered it — black-holing broadcasts on a store-less relay.
    /// `__GROUP_MSG__` is deliberately absent: it is legitimate peer/relay
    /// group traffic. Mirrors the Kotlin translator — keep in sync.
    private static let serverPlaneAnswerPrefixes = [
        "__GROUP_CREATED__",
        "__GROUP_MEMBER_ADDED__",
        "__GROUP_MEMBER_REMOVED__",
        "__GROUP_INFO__",
        "__USER_GROUPS__",
        "__GROUP_ERROR__"
    ]

    /// True when peer-delivered message content must be dropped because it
    /// impersonates a relay server answer. Called by the `MessageReceived`
    /// ingest path with the inner SDK content.
    static func isForgedServerPlaneAnswer(_ content: String) -> Bool {
        return serverPlaneAnswerPrefixes.contains { content.hasPrefix($0) }
    }

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
    /// Bumped by reset(). Commit closures capture the generation of their
    /// translation and no-op if it moved: state written after a reset would
    /// describe frames sent on a connection (or inside a rate budget) the
    /// relay already discarded.
    private var generation: Int64 = 0
    /// Last membership committed as registered with the relay, per group.
    private var registeredMembers: [String: Set<String>] = [:]
    /// Groups whose member deltas the relay denied (we are not the admin).
    private var memberDeltasDenied: Set<String> = []
    /// Groups with member deltas outstanding: the window between a
    /// translation that produced AddGroupMember/RemoveGroupMember frames and
    /// the next group-scoped answer. Only a GroupError landing inside this
    /// window may be read as OUR admin denial — the translator never tags
    /// request_id, so a group-scoped error with no outstanding deltas belongs
    /// to some other actor (an app raw-channel op, another admin's edit).
    private var outstandingMemberDeltas: Set<String> = []
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
            let gen = generation
            return .tap([[
                "type": "LeaveGroup",
                "group_id": groupId
            ]], { [weak self] in
                self?.commitLeave(generationAtTranslate: gen, groupId: groupId)
            })

        default:
            return .passThrough
        }
    }

    /// Feed relay GroupError answers so admin-denied groups stop producing
    /// member deltas. The denial is honored ONLY when it correlates to member
    /// deltas this translator has outstanding: a `request_id`-carrying error
    /// answers an app raw-channel frame (this translator never tags
    /// request_id, so it cannot be ours), and without an open per-group delta
    /// window the error belongs to some other actor's operation — treating it
    /// as ours would permanently suppress membership sync for the group.
    func onGroupError(groupId: String, reason: String, requestId: String? = nil) {
        guard !groupId.isEmpty else { return }
        if let requestId = requestId, !requestId.isEmpty { return }
        lock.lock()
        defer { lock.unlock() }
        guard outstandingMemberDeltas.contains(groupId) else { return }
        // The next group-scoped answer closes the window, denial or not.
        outstandingMemberDeltas.remove(groupId)
        if reason.range(of: Self.adminDeniedReasonMarker, options: .caseInsensitive) != nil {
            memberDeltasDenied.insert(groupId)
            // The optimistic membership snapshot was not applied server-side.
            registeredMembers.removeValue(forKey: groupId)
        }
    }

    /// Feed relay *success* answers on the group channel (GroupCreated,
    /// GroupMemberAdded, GroupMemberRemoved): any group-scoped answer closes
    /// the admin-denial correlation window, success included — `onGroupError`
    /// already closes it for errors. Without this, a successful
    /// register-with-deltas leaves the window open for the whole connection,
    /// and a later request_id-less GroupError merely *quoting* the denial
    /// phrase (e.g. a user-authored group name) would be honored as OUR
    /// denial and suppress membership sync until reconnect. Mirrors
    /// android RelayControlOpTranslator.kt — keep the two in sync.
    func onGroupAnswered(groupId: String) {
        guard !groupId.isEmpty else { return }
        lock.lock()
        defer { lock.unlock() }
        outstandingMemberDeltas.remove(groupId)
    }

    /// Feed relay `GroupRoleChanged` frames: a promotion of this device to
    /// admin re-enables member deltas an earlier denial suppressed —
    /// otherwise a mid-connection promotion keeps membership edits away from
    /// the relay until the next reconnect. (The denial already dropped the
    /// group's committed snapshot, so the next register recomputes the full
    /// delta set.)
    func onRoleChanged(groupId: String, userId: String, newRole: String) {
        guard !groupId.isEmpty, userId == selfId else { return }
        guard newRole.caseInsensitiveCompare("admin") == .orderedSame else { return }
        lock.lock()
        defer { lock.unlock() }
        memberDeltasDenied.remove(groupId)
    }

    /// Per-connection state: call on disconnect.
    func reset() {
        lock.lock()
        defer { lock.unlock() }
        generation += 1
        registeredMembers.removeAll()
        memberDeltasDenied.removeAll()
        outstandingMemberDeltas.removeAll()
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
        // Read via NSNumber boolValue, not `as? Bool`: JSONSerialization
        // hands back NSNumber for both JSON booleans and 0/1 integers, and
        // the SE-0170 bridge's number handling must not decide whether a 0/1
        // encoding of the hint is honored — boolValue treats them uniformly.
        let notAdmin: Bool
        if let adminNumber = payload["is_admin"] as? NSNumber {
            notAdmin = !adminNumber.boolValue
        } else {
            notAdmin = false
        }

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
            // Deltas produced: open the group's outstanding-delta window so
            // the next group-scoped GroupError may be read as our denial.
            if frames.count > 1 {
                outstandingMemberDeltas.insert(groupId)
            }
            // Committed only after the frames are actually written.
            let gen = generation
            commit = { [weak self] in
                self?.commitRegisteredMembers(
                    generationAtTranslate: gen,
                    groupId: groupId,
                    members: desired
                )
            }
        }
        return .replace(frames, commit)
    }

    private func commitRegisteredMembers(generationAtTranslate: Int64, groupId: String, members: Set<String>) {
        lock.lock()
        defer { lock.unlock() }
        guard generationAtTranslate == generation else { return }
        // A GroupError may have marked the group admin-denied while the
        // frames were in flight; the denial wins.
        if !memberDeltasDenied.contains(groupId) {
            registeredMembers[groupId] = members
        }
    }

    private func commitLeave(generationAtTranslate: Int64, groupId: String) {
        lock.lock()
        defer { lock.unlock() }
        guard generationAtTranslate == generation else { return }
        leaveSent.insert(groupId)
        registeredMembers.removeValue(forKey: groupId)
        memberDeltasDenied.remove(groupId)
        // A left group's answers are no longer ours to correlate; a stale
        // window must not let a post-leave GroupError mark a future rejoin
        // as admin-denied.
        outstandingMemberDeltas.remove(groupId)
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
