package com.offlineprotocol

import android.util.Base64
import org.json.JSONArray
import org.json.JSONObject
import org.junit.Assert.assertEquals
import org.junit.Assert.assertFalse
import org.junit.Assert.assertNull
import org.junit.Assert.assertTrue
import org.junit.Test
import org.junit.runner.RunWith
import org.robolectric.RobolectricTestRunner
import org.robolectric.annotation.Config

/**
 * The gateway contract's decisions and frame shapes.
 *
 * What is *not* here is the signed proof: those bytes are built and pinned in
 * the core, by conformance vectors, so this side has no copy of the layout to
 * drift from. What is here is everything the manager decides on its own.
 *
 * Robolectric because [GatewayAttachPolicy] uses `android.util.Base64`, which
 * is a framework class with no JVM stub. `java.util.Base64` is API 26 and this
 * module targets 24.
 */
@RunWith(RobolectricTestRunner::class)
@Config(sdk = [24])
class GatewayAttachPolicyTest {

    // ---- Challenge --------------------------------------------------------

    @Test
    fun `a well formed challenge is accepted`() {
        val challenge = ByteArray(32) { it.toByte() }
        val json = JSONObject().put("challenge", Base64.encodeToString(challenge, Base64.NO_WRAP))

        val outcome = GatewayAttachPolicy.decodeChallenge(json)

        assertTrue(outcome is GatewayAttachPolicy.ChallengeOutcome.Declare)
        assertTrue(
            (outcome as GatewayAttachPolicy.ChallengeOutcome.Declare)
                .challenge.contentEquals(challenge)
        )
    }

    @Test
    fun `a missing challenge is skipped not guessed`() {
        assertEquals(
            GatewayAttachPolicy.ChallengeOutcome.Skip(GatewayAttachPolicy.Reason.CHALLENGE_ABSENT),
            GatewayAttachPolicy.decodeChallenge(JSONObject())
        )
        assertEquals(
            GatewayAttachPolicy.ChallengeOutcome.Skip(GatewayAttachPolicy.Reason.CHALLENGE_ABSENT),
            GatewayAttachPolicy.decodeChallenge(JSONObject().put("challenge", ""))
        )
    }

    /**
     * A short challenge is refused here as well as in the core. The replay
     * bound is the challenge, so a gateway that mints eight bytes has weakened
     * the thing the handshake exists for.
     */
    @Test
    fun `a wrong sized challenge is refused`() {
        for (size in listOf(8, 16, 31, 33, 64)) {
            val challenge = ByteArray(size) { 7 }
            val json =
                JSONObject().put("challenge", Base64.encodeToString(challenge, Base64.NO_WRAP))
            assertEquals(
                "a $size-byte challenge must not be signed",
                GatewayAttachPolicy.ChallengeOutcome.Skip(
                    GatewayAttachPolicy.Reason.CHALLENGE_WRONG_SIZE),
                GatewayAttachPolicy.decodeChallenge(json)
            )
        }
    }

    // ---- Binding ----------------------------------------------------------

    @Test
    fun `the echoed address must be ours`() {
        assertEquals(
            GatewayAttachPolicy.BindingOutcome.BOUND,
            GatewayAttachPolicy.bindingOutcome("off1alice", "off1alice")
        )
        assertEquals(
            GatewayAttachPolicy.BindingOutcome.MISMATCH,
            GatewayAttachPolicy.bindingOutcome("off1bob", "off1alice")
        )
    }

    /**
     * With no local address there is nothing this device declared, so an echo
     * is not evidence of anything and must not count as bound.
     */
    @Test
    fun `an echo with no local address is not a binding`() {
        assertEquals(
            GatewayAttachPolicy.BindingOutcome.UNKNOWN_LOCAL,
            GatewayAttachPolicy.bindingOutcome("off1alice", null)
        )
        assertEquals(
            GatewayAttachPolicy.BindingOutcome.UNKNOWN_LOCAL,
            GatewayAttachPolicy.bindingOutcome("off1alice", "")
        )
    }

    // ---- Capabilities -----------------------------------------------------

    @Test
    fun `capabilities are bounded`() {
        val tokens = JSONArray()
        repeat(200) { tokens.put("cap_$it") }

        val kept = GatewayAttachPolicy.capabilityTokens(JSONObject().put("tokens", tokens))

        assertEquals(GatewayAttachPolicy.MAX_CAPABILITY_TOKENS, kept.size)
    }

    /**
     * Oversized tokens are dropped before the count is applied, so padding
     * cannot push out the tokens that matter.
     */
    @Test
    fun `padding cannot evict real capability tokens`() {
        val tokens = JSONArray()
        repeat(64) { tokens.put("x".repeat(4096)) }
        tokens.put("gateway_v1")

        val kept = GatewayAttachPolicy.capabilityTokens(JSONObject().put("tokens", tokens))

        assertEquals(listOf("gateway_v1"), kept)
    }

    @Test
    fun `capabilities ignore non strings and empties`() {
        val tokens = JSONArray().put("gateway_v1").put(7).put("").put("backbone_reticulum_v1")

        val kept = GatewayAttachPolicy.capabilityTokens(JSONObject().put("tokens", tokens))

        assertEquals(listOf("gateway_v1", "backbone_reticulum_v1"), kept)
    }

    @Test
    fun `absent capabilities are empty not an error`() {
        assertEquals(emptyList<String>(), GatewayAttachPolicy.capabilityTokens(JSONObject()))
        assertEquals(
            emptyList<String>(),
            GatewayAttachPolicy.capabilityTokens(JSONObject().put("tokens", "nope"))
        )
    }

    // ---- Message ids ------------------------------------------------------

    /**
     * A UUID is what the core hands us, and it has to survive the gateway's own
     * rule unchanged: an id it would refuse comes back under a name nothing is
     * waiting on.
     */
    @Test
    fun `a uuid passes the gateways own rule`() {
        val uuid = java.util.UUID.randomUUID().toString()
        assertEquals(uuid, GatewayAttachPolicy.sanitizeMessageId(uuid))
    }

    @Test
    fun `an id the gateway would refuse is rejected here`() {
        assertNull(GatewayAttachPolicy.sanitizeMessageId(null))
        assertNull(GatewayAttachPolicy.sanitizeMessageId(""))
        assertNull(GatewayAttachPolicy.sanitizeMessageId("has space"))
        assertNull(GatewayAttachPolicy.sanitizeMessageId("has/slash"))
        assertNull(GatewayAttachPolicy.sanitizeMessageId("a".repeat(65)))
        assertEquals("a".repeat(64), GatewayAttachPolicy.sanitizeMessageId("a".repeat(64)))
    }

    // ---- Frames we send ---------------------------------------------------

    @Test
    fun `identify carries the version`() {
        val json = JSONObject(GatewayAttachPolicy.identifyJson("off1alice")!!)
        assertEquals("Identify", json.getString("type"))
        assertEquals("off1alice", json.getString("device_id"))
        assertEquals(1, json.getInt("protocol_version"))
    }

    @Test
    fun `declaration carries base64 fields`() {
        val key = ByteArray(32) { 1 }
        val sig = ByteArray(64) { 2 }

        val json = JSONObject(GatewayAttachPolicy.declarationJson("off1alice", key, sig)!!)

        assertEquals("DeclareAddress", json.getString("type"))
        assertEquals("off1alice", json.getString("address"))
        assertTrue(
            Base64.decode(json.getString("public_key"), Base64.DEFAULT).contentEquals(key))
        assertTrue(
            Base64.decode(json.getString("signature"), Base64.DEFAULT).contentEquals(sig))
    }

    /**
     * The id goes on the wire. Without it the gateway mints its own and every
     * verdict comes back uncorrelatable, which is the whole reason the shipped
     * bridge could not settle on an answer.
     */
    @Test
    fun `send message carries the message id`() {
        val json = JSONObject(
            GatewayAttachPolicy.sendMessageJson("abc-123", "off1bob", "Zm9v", null)!!)

        assertEquals("SendMessage", json.getString("type"))
        assertEquals("abc-123", json.getString("message_id"))
        assertEquals("off1bob", json.getString("recipient"))
        assertEquals("base64", json.getString("encoding"))
        assertFalse(json.has("reply_to_msg"))
    }

    @Test
    fun `send message omits an empty reply to`() {
        val json = JSONObject(GatewayAttachPolicy.sendMessageJson("abc", "off1bob", "Zm9v", "")!!)
        assertFalse(json.has("reply_to_msg"))
    }

    /**
     * One frame for the batch, which is the contract's shape and not the
     * relay's one-peer-per-frame query.
     */
    @Test
    fun `check presence asks about every peer in one frame`() {
        val json = JSONObject(
            GatewayAttachPolicy.checkPresenceJson(listOf("off1a", "off1b", "off1c"))!!)

        assertEquals("CheckPresence", json.getString("type"))
        assertEquals(3, json.getJSONArray("peers").length())
    }

    @Test
    fun `check presence is capped at what a gateway answers`() {
        val peers = (0 until 200).map { "off1peer$it" }
        val json = JSONObject(GatewayAttachPolicy.checkPresenceJson(peers)!!)
        assertEquals(
            GatewayAttachPolicy.MAX_PRESENCE_PEERS, json.getJSONArray("peers").length())
    }

    @Test
    fun `check presence with no peers is not sent`() {
        assertNull(GatewayAttachPolicy.checkPresenceJson(emptyList()))
    }

    // ---- Frames we read ---------------------------------------------------

    @Test
    fun `a message sent settles its id`() {
        val verdict = GatewayAttachPolicy.parseVerdict(
            JSONObject().put("message_id", "abc").put("recipient", "off1bob"), "MessageSent")

        assertEquals("abc", verdict?.messageId)
        assertTrue(verdict!!.sent)
        assertNull(verdict.reason)
    }

    /**
     * The reason travels verbatim: the core classifies on the
     * `recipient_unreachable` prefix and discards the rest.
     */
    @Test
    fun `a delivery error carries its reason untouched`() {
        val verdict = GatewayAttachPolicy.parseVerdict(
            JSONObject()
                .put("message_id", "abc")
                .put("reason", "recipient_unreachable: not attached here"),
            "DeliveryError"
        )

        assertFalse(verdict!!.sent)
        assertEquals("recipient_unreachable: not attached here", verdict.reason)
    }

    @Test
    fun `a verdict with no id settles nothing`() {
        assertNull(GatewayAttachPolicy.parseVerdict(JSONObject(), "MessageSent"))
        assertNull(
            GatewayAttachPolicy.parseVerdict(
                JSONObject().put("message_id", ""), "DeliveryError"))
    }

    @Test
    fun `presence is read`() {
        val answer = GatewayAttachPolicy.parsePresence(
            JSONObject()
                .put("peer", "off1bob")
                .put("online", true)
                .put("last_seen_ms", 1_786_924_800_000L)
        )

        assertEquals("off1bob", answer?.peer)
        assertEquals(true, answer?.online)
        assertEquals(1_786_924_800_000L, answer?.lastSeenMs)
    }

    /**
     * A missing `online` is not readable as "offline": that manufactures a
     * claim the gateway never made, and a claim is what drives parking.
     */
    @Test
    fun `presence without an online flag is not a claim`() {
        assertNull(GatewayAttachPolicy.parsePresence(JSONObject().put("peer", "off1bob")))
        assertNull(GatewayAttachPolicy.parsePresence(JSONObject().put("online", true)))
        assertNull(
            GatewayAttachPolicy.parsePresence(
                JSONObject().put("peer", "").put("online", false)))
    }

    @Test
    fun `presence without last seen is still an answer`() {
        val answer = GatewayAttachPolicy.parsePresence(
            JSONObject().put("peer", "off1bob").put("online", false))

        assertEquals(false, answer?.online)
        assertNull(answer?.lastSeenMs)
    }

    // ---- Constants --------------------------------------------------------

    /**
     * The verdict timeout must stay below the core's pending-confirmation
     * expiry, or the core settles the frame first and the verdict then arrives
     * for an id it has already moved past. A Rust guard pins the same
     * relationship across both bridges; this is the local half.
     */
    @Test
    fun `the verdict timeout is shorter than the cores own expiry`() {
        assertTrue(GatewayAttachPolicy.VERDICT_TIMEOUT_MS < 120_000L)
        assertTrue(GatewayAttachPolicy.ATTACH_TIMEOUT_MS < 60_000L)
    }

    /**
     * A refusal that carries no wording is still a refusal, and the recipient
     * it names still reaches the manager.
     */
    @Test
    fun `a DeliveryError without a reason still settles and carries the recipient`() {
        val json = JSONObject("""{"type":"DeliveryError","message_id":"m1","recipient":"off1bob"}""")
        val verdict = GatewayAttachPolicy.parseVerdict(json, "DeliveryError")!!
        assertFalse(verdict.sent)
        assertEquals("DeliveryError", verdict.reason)
        assertEquals("off1bob", verdict.recipient)
    }

    /**
     * The token bound is in bytes, as the core's is: a hundred two-byte
     * characters is over it, and exactly 128 bytes is on it.
     */
    @Test
    fun `capability tokens are bounded in bytes not characters`() {
        val exact = "a".repeat(128)
        val over = "a".repeat(129)
        val multibyte = "é".repeat(100)
        val json = JSONObject().put("tokens", JSONArray(listOf(exact, over, multibyte, "gateway_v1")))
        assertEquals(listOf(exact, "gateway_v1"), GatewayAttachPolicy.capabilityTokens(json))
    }
}
