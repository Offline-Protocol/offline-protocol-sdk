package com.offlineprotocol

import org.junit.Assert.assertEquals
import org.junit.Assert.assertFalse
import org.junit.Assert.assertTrue
import org.junit.Test

/**
 * Pins the reconstructed-Message shape against the Rust `Message`
 * deserializer's requirements (see `serialized_control_frame` in
 * `crates/offline-protocol-uniffi/src/lib.rs` — the Rust twin of this
 * shape). Regression pin for the silent-drop bug where synthesized relay
 * frames were missing `id`/`timestamp` and used `"Medium"` instead of the
 * canonical lowercase priority.
 */
class LegacyRelayMessageTest {

    @Test
    fun everyFieldRequiredByTheRustDeserializerIsPresent() {
        val json = LegacyRelayMessage.buildJson(
            senderId = "alice",
            recipientId = "bob",
            content = "hello",
            timestampMs = 1_700_000_000_000L
        )

        // id, sender, recipient, app_id, priority, ttl, hop_count, content,
        // and timestamp have no serde default in the Rust Message struct —
        // a frame missing any of them fails deserialization and is silently
        // dropped by the transport.
        for (key in listOf(
            "id", "sender", "recipient", "content", "app_id",
            "priority", "ttl", "hop_count", "requires_ack", "timestamp"
        )) {
            assertTrue("required field '$key' missing", json.has(key))
        }
    }

    @Test
    fun priorityIsTheCanonicalLowercaseVariant() {
        val json = LegacyRelayMessage.buildJson(
            senderId = "alice",
            recipientId = "bob",
            content = "hello",
            timestampMs = 1_700_000_000_000L
        )
        // rename_all = "lowercase" on MessagePriority: "Medium" fails.
        assertEquals("medium", json.getString("priority"))
    }

    @Test
    fun idFallsBackToAUuidWhenAbsentAndPassesThroughWhenGiven() {
        val generated = LegacyRelayMessage.buildJson(
            senderId = "alice",
            recipientId = "bob",
            content = "hello",
            timestampMs = 1_700_000_000_000L,
            messageId = ""
        )
        // MessageId deserializes via Uuid::parse_str — a non-UUID id fails.
        java.util.UUID.fromString(generated.getString("id"))

        val given = LegacyRelayMessage.buildJson(
            senderId = "alice",
            recipientId = "bob",
            content = "hello",
            timestampMs = 1_700_000_000_000L,
            messageId = "6dd7f6f0-9d2c-4b6a-8f3e-2a1b0c9d8e7f"
        )
        assertEquals("6dd7f6f0-9d2c-4b6a-8f3e-2a1b0c9d8e7f", given.getString("id"))
    }

    @Test
    fun replyToMsgIncludedOnlyWhenNonEmpty() {
        val without = LegacyRelayMessage.buildJson(
            senderId = "alice",
            recipientId = "bob",
            content = "hello",
            timestampMs = 1_700_000_000_000L,
            replyToMsg = ""
        )
        assertFalse(without.has("reply_to_msg"))

        val with = LegacyRelayMessage.buildJson(
            senderId = "alice",
            recipientId = "bob",
            content = "hello",
            timestampMs = 1_700_000_000_000L,
            replyToMsg = "7ee8f6f0-9d2c-4b6a-8f3e-2a1b0c9d8e7f"
        )
        assertEquals("7ee8f6f0-9d2c-4b6a-8f3e-2a1b0c9d8e7f", with.getString("reply_to_msg"))
    }

    @Test
    fun scalarFieldsCarryTheWireValues() {
        val json = LegacyRelayMessage.buildJson(
            senderId = "alice",
            recipientId = "bob",
            content = "hello",
            timestampMs = 1_700_000_000_000L
        )
        assertEquals("alice", json.getString("sender"))
        assertEquals("bob", json.getString("recipient"))
        assertEquals("hello", json.getString("content"))
        assertEquals(1_700_000_000_000L, json.getLong("timestamp"))
        assertEquals(0, json.getInt("hop_count"))
        assertEquals(8, json.getInt("ttl"))
        assertTrue(json.getBoolean("requires_ack"))
    }

    /**
     * Bridge-synthesized frames (injectGroupInternalMessage) opt out of the
     * ACK: nothing transmitted them, so no sender awaits a delivery
     * confirmation — and the core addresses that ACK to the frame's `sender`,
     * which for a relay answer is a placeholder, not a reachable peer.
     * Regression pin for the phantom-peer bug, where every injected frame
     * produced an undeliverable outbound DM to "relay".
     */
    @Test
    fun requiresAckIsOptOutForSynthesizedFrames() {
        val json = LegacyRelayMessage.buildJson(
            senderId = "relay",
            recipientId = "bob",
            content = "__GROUP_CREATED__{}",
            timestampMs = 1_700_000_000_000L,
            requiresAck = false
        )
        assertFalse(json.getBoolean("requires_ack"))
        // The opt-out must not disturb the required-field set.
        for (key in listOf(
            "id", "sender", "recipient", "content", "app_id",
            "priority", "ttl", "hop_count", "requires_ack", "timestamp"
        )) {
            assertTrue("required field '$key' missing", json.has(key))
        }
    }
}
