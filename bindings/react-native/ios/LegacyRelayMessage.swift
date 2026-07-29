//
// LegacyRelayMessage.swift
// OfflineProtocol
//
// Builds the full serialized `Message` dictionary for relay frames that
// don't already carry one: legacy JS-relay senders whose `content` is bare
// text, and bridge-synthesized internal (relay) messages.
//
// Every field required by the Rust `Message` deserializer must be present
// and correctly shaped — a missing `id`/`timestamp` or a non-lowercase
// `priority` makes the transport silently drop the frame. The required-field
// set is pinned by LegacyRelayMessageTests; the Rust twin of this shape is
// `serialized_control_frame` in `crates/offline-protocol-uniffi/src/lib.rs`.
// Mirrors android/.../LegacyRelayMessage.kt — keep in sync.
//

import Foundation

enum LegacyRelayMessage {
    /// `requiresAck` defaults to true for frames that a real peer actually
    /// transmitted (legacy JS-relay senders), whose sender is waiting on a
    /// delivery confirmation. Bridge-synthesized frames pass `false`: nothing
    /// crossed a wire, so nobody awaits an ACK — and the core would otherwise
    /// address that ACK to the frame's `sender`, which for a relay answer is
    /// a placeholder, not a reachable peer.
    static func buildDict(
        senderId: String,
        recipientId: String,
        content: String,
        timestampMs: Int64,
        messageId: String? = nil,
        replyToMsg: String? = nil,
        requiresAck: Bool = true
    ) -> [String: Any] {
        var dict: [String: Any] = [
            "id": (messageId?.isEmpty == false) ? messageId! : UUID().uuidString,
            "sender": senderId,
            "recipient": recipientId, // Will be corrected by protocol
            "content": content,
            "app_id": "offline-messenger", // Default app ID
            // Serde expects the SDK's canonical lowercase variant ("medium");
            // any other casing fails deserialization and the frame is
            // silently dropped by the transport.
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
