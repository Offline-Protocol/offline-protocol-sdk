package com.offlineprotocol

/**
 * Builds the full serialized `Message` JSON for relay frames that don't
 * already carry one: legacy JS-relay senders whose `content` is bare text,
 * and bridge-synthesized internal (relay) messages.
 *
 * Every field required by the Rust `Message` deserializer must be present
 * and correctly shaped — a missing `id`/`timestamp` or a non-lowercase
 * `priority` makes the transport silently drop the frame. The required-field
 * set is pinned by LegacyRelayMessageTest; the Rust twin of this shape is
 * `serialized_control_frame` in `crates/offline-protocol-uniffi/src/lib.rs`.
 * Mirrors ios/LegacyRelayMessage.swift — keep in sync.
 */
object LegacyRelayMessage {
    /**
     * [requiresAck] defaults to true for frames that a real peer actually
     * transmitted (legacy JS-relay senders), whose sender is waiting on a
     * delivery confirmation. Bridge-synthesized frames pass `false`: nothing
     * crossed a wire, so nobody awaits an ACK — and the core would otherwise
     * address that ACK to the frame's `sender`, which for a relay answer is a
     * placeholder, not a reachable peer.
     */
    fun buildJson(
        senderId: String,
        recipientId: String,
        content: String,
        timestampMs: Long,
        messageId: String? = null,
        replyToMsg: String? = null,
        requiresAck: Boolean = true
    ): org.json.JSONObject = org.json.JSONObject().apply {
        put(
            "id",
            if (!messageId.isNullOrEmpty()) messageId
            else java.util.UUID.randomUUID().toString()
        )
        put("sender", senderId)
        put("recipient", recipientId) // Will be corrected by protocol
        put("content", content)
        put("app_id", "offline-messenger") // Default app ID
        // Serde expects the SDK's canonical lowercase variant ("medium");
        // any other casing fails deserialization and the frame is silently
        // dropped by the transport.
        put("priority", "medium")
        put("ttl", 8)
        put("hop_count", 0)
        put("requires_ack", requiresAck)
        put("timestamp", timestampMs)
        if (!replyToMsg.isNullOrEmpty()) {
            put("reply_to_msg", replyToMsg)
        }
    }
}
