package com.offlineprotocol

/**
 * Builds the full serialized `Message` JSON for relay frames that don't
 * already carry one: legacy JS-relay senders whose `content` is bare text,
 * and bridge-synthesized internal (relay) messages.
 *
 * Every field required by the Rust `Message` deserializer must be present
 * and correctly shaped: a missing `id`/`timestamp` or an unknown `priority`
 * makes the transport silently drop the frame. The required-field set is
 * pinned by LegacyRelayMessageTest; the Rust twin of this shape is
 * `serialized_control_frame` in `crates/offline-protocol-uniffi/src/lib.rs`.
 * Mirrors ios/LegacyRelayMessage.swift, keep in sync.
 *
 * INVARIANT: a rebuilt frame is addressed to this device, which is its
 * `localAddress()` once MLS has minted one and the profile until then. The
 * core forwards rather than processes a frame addressed to anyone else, and
 * after `initialize_mls` the profile is not this device's id: a frame named
 * for it is handed to the mesh forwarder and never reaches the app. That is
 * why the builder takes both identities and chooses, rather than taking a
 * recipient a caller could fill with the profile.
 */
object LegacyRelayMessage {
    /**
     * The recipient a rebuilt inbound frame must carry. See the INVARIANT
     * above. [localAddress] must be resolved per frame by the caller, never
     * captured: the manager is built before MLS may have run.
     */
    fun recipient(localAddress: String?, profile: String): String =
        localAddress?.takeIf { it.isNotEmpty() } ?: profile

    /**
     * [requiresAck] defaults to true for frames that a real peer actually
     * transmitted (legacy JS-relay senders), whose sender is waiting on a
     * delivery confirmation. Bridge-synthesized frames pass `false`: nothing
     * crossed a wire, so nobody awaits an ACK, and the core would otherwise
     * address that ACK to the frame's `sender`, which for a relay answer is a
     * placeholder, not a reachable peer.
     */
    fun buildJson(
        senderId: String,
        localAddress: String?,
        profile: String,
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
        put("recipient", recipient(localAddress, profile))
        put("content", content)
        put("app_id", "offline-messenger") // Default app ID
        // The SDK's canonical lowercase variant. The core also accepts the
        // capitalized alias; any other spelling fails deserialization and the
        // frame is silently dropped.
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
