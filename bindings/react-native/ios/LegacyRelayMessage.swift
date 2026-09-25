//
// LegacyRelayMessage.swift
// OfflineProtocol
//
// Builds the full serialized `Message` dictionary for relay frames that
// don't already carry one: legacy JS-relay senders whose `content` is bare
// text, and bridge-synthesized internal (relay) messages.
//
// Every field required by the Rust `Message` deserializer must be present
// and correctly shaped: a missing `id`/`timestamp` or an unknown `priority`
// makes the transport silently drop the frame. The required-field set is
// pinned by LegacyRelayMessageTests; the Rust twin of this shape is
// `serialized_control_frame` in `crates/offline-protocol-uniffi/src/lib.rs`.
// Mirrors android/.../LegacyRelayMessage.kt, keep in sync.
//
// INVARIANT: a rebuilt frame is addressed to this device, which is its
// `localAddress()` once MLS has minted one and the profile until then. The
// core forwards rather than processes a frame addressed to anyone else, and
// after `initialize_mls` the profile is not this device's id: a frame named
// for it is handed to the mesh forwarder and never reaches the app. That is
// why the builder takes both identities and chooses, rather than taking a
// recipient a caller could fill with the profile.
//

import Foundation

enum LegacyRelayMessage {
    /// The recipient a rebuilt inbound frame must carry. See the INVARIANT
    /// above. `localAddress` must be resolved per frame by the caller, never
    /// captured: the manager is built before MLS may have run.
    static func recipient(localAddress: String?, profile: String) -> String {
        if let address = localAddress, !address.isEmpty {
            return address
        }
        return profile
    }

    /// `requiresAck` defaults to true for frames that a real peer actually
    /// transmitted (legacy JS-relay senders), whose sender is waiting on a
    /// delivery confirmation. Bridge-synthesized frames pass `false`: nothing
    /// crossed a wire, so nobody awaits an ACK, and the core would otherwise
    /// address that ACK to the frame's `sender`, which for a relay answer is
    /// a placeholder, not a reachable peer.
    ///
    /// `appId` is this instance's configured application id, the same value
    /// the core stamps on every frame it originates. It has no default: a
    /// literal here once stamped every synthesized frame with one fixed id
    /// whatever the app had configured.
    static func buildDict(
        senderId: String,
        localAddress: String?,
        profile: String,
        appId: String,
        content: String,
        timestampMs: Int64,
        messageId: String? = nil,
        replyToMsg: String? = nil,
        requiresAck: Bool = true
    ) -> [String: Any] {
        var dict: [String: Any] = [
            "id": (messageId?.isEmpty == false) ? messageId! : UUID().uuidString,
            "sender": senderId,
            "recipient": recipient(localAddress: localAddress, profile: profile),
            "content": content,
            "app_id": appId,
            // The SDK's canonical lowercase variant. The core also accepts
            // the capitalized alias; any other spelling fails
            // deserialization and the frame is silently dropped.
            "priority": "medium",
            "ttl": 8,
            "hop_count": 0,
            "requires_ack": requiresAck,
            "timestamp": timestampMs
        ]
        if let replyToMsg = replyToMsg, !replyToMsg.isEmpty {
            dict["reply_to_msg"] = replyToMsg
        }
        return dict
    }
}
