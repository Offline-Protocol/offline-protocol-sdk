package com.offlineprotocol

import android.util.Base64
import org.json.JSONArray
import org.json.JSONObject

/**
 * Pure decisions and frame shapes for gateway daemon contract v1.
 *
 * The manager owns the socket, the loopers and the lifecycle; everything here
 * is a function of its arguments, which is what makes it testable at all.
 * Mirrors `GatewayAttachPolicy.swift` — keep the two in sync.
 *
 * **What is deliberately not here: the signed proof.** The bytes a device
 * signs to attach are built and signed in the core, behind
 * `gatewayAddressDeclaration(challenge)`. The relay's equivalent had to live
 * on this side because it commits the relay's account name, which only a
 * bridge knows, and it is now hand-mirrored in three languages. This one
 * commits our own address, so it exists once where conformance vectors can pin
 * it. What is left here is the framing around it.
 */
object GatewayAttachPolicy {

    // ---- Wire constants ---------------------------------------------------

    /** The contract version this client speaks, sent on `Identify`. */
    const val PROTOCOL_VERSION = 1

    /**
     * Bytes of challenge the gateway mints per connection. A declaration is not
     * attempted for anything else: the core refuses to sign it, and finding
     * that out from a thrown exception is worse than not asking.
     */
    const val CHALLENGE_LENGTH = 32

    /**
     * How long the whole handshake may take, from `Identify` to
     * `StatusUpdate(connected)`.
     *
     * Far shorter than the 60s connection timeout, deliberately: TCP to the
     * daemon is a LAN hop, so a gateway that has accepted the socket and not
     * finished the handshake is not slow, it is broken. Waiting a minute to
     * learn that is a minute in which the selector has been told nothing.
     */
    const val ATTACH_TIMEOUT_MS = 10_000L

    /**
     * How long a submitted frame may go without a verdict before this client
     * treats the gateway's silence as a failure.
     *
     * **Must stay below the core's 120s pending-confirmation timeout**, the
     * other clock on the same frame. If this were the longer of the two, the
     * core would expire the frame first and count it a failure, and the verdict
     * arriving afterwards would settle an id the core had already moved past. A
     * Rust guard pins this number and that relationship across both bridges.
     */
    const val VERDICT_TIMEOUT_MS = 60_000L

    /**
     * Frames submitted but not yet answered. Roughly what the core's own
     * session bootstrap bursts, so a smaller number would throttle the one case
     * that matters most.
     */
    const val MAX_IN_FLIGHT = 8

    /** Capability bounds, matching the relay's, which is what the contract points at. */
    const val MAX_CAPABILITY_TOKENS = 64
    const val MAX_CAPABILITY_TOKEN_BYTES = 128

    /**
     * Peers a gateway answers per `CheckPresence`. Asking about more only
     * guarantees silence for the ones past the cap.
     */
    const val MAX_PRESENCE_PEERS = 64

    /**
     * Longest line this client will accept before abandoning the stream. A
     * frame at the gateway's cap arrives base64-encoded, so 4/3 of its size
     * plus the JSON around it.
     */
    const val MAX_LINE_BYTES = 1 shl 20

    /**
     * Longest `AddressDeclared` echo a manager hands to the core. An address
     * is 44 characters; the bound is what keeps a hostile echo, which may be
     * as long as a line, out of the core's log and security event.
     */
    const val MAX_ADDRESS_BYTES = 128

    // ---- Attach -----------------------------------------------------------

    /**
     * Stable diagnostic reasons, shared with the Swift mirror so a field report
     * reproduces under the same string on both platforms.
     */
    object Reason {
        const val ADDRESS_UNAVAILABLE = "address_unavailable"
        const val CHALLENGE_ABSENT = "challenge_absent"
        const val CHALLENGE_MALFORMED = "challenge_malformed"
        const val CHALLENGE_WRONG_SIZE = "challenge_wrong_size"
        const val SIGNING_FAILED = "signing_failed"
        const val FRAME_UNSERIALIZABLE = "frame_unserializable"
    }

    /** What a `Challenge` frame yielded. */
    sealed class ChallengeOutcome {
        data class Declare(val challenge: ByteArray) : ChallengeOutcome() {
            override fun equals(other: Any?): Boolean =
                this === other || (other is Declare && challenge.contentEquals(other.challenge))

            override fun hashCode(): Int = challenge.contentHashCode()
        }

        data class Skip(val reason: String) : ChallengeOutcome()
    }

    /**
     * What a gateway's `AddressDeclared` echo says about this device.
     *
     * The same three answers the core reports on, decided again here because
     * the two act on different things: the core emits the security warning, and
     * the bridge decides whether this carrier can be offered at all.
     */
    enum class BindingOutcome {
        /** The gateway bound the address we declared. The session is proven. */
        BOUND,

        /** It bound something else. A security event, not a retry. */
        MISMATCH,

        /** We hold no address, so nothing here was ever declared by us. */
        UNKNOWN_LOCAL,
    }

    /**
     * Reads the challenge out of a `Challenge` frame.
     *
     * The size is checked here as well as in the core because the two refusals
     * mean different things to a reader: this one says the gateway is not
     * speaking the contract, and the core's says something asked it to sign a
     * payload it should not.
     */
    @JvmStatic
    fun decodeChallenge(json: JSONObject): ChallengeOutcome {
        val encoded = json.optString("challenge", "")
        if (encoded.isEmpty()) return ChallengeOutcome.Skip(Reason.CHALLENGE_ABSENT)
        val challenge =
            try {
                Base64.decode(encoded, Base64.DEFAULT)
            } catch (e: IllegalArgumentException) {
                return ChallengeOutcome.Skip(Reason.CHALLENGE_MALFORMED)
            }
        if (challenge.size != CHALLENGE_LENGTH) {
            return ChallengeOutcome.Skip(Reason.CHALLENGE_WRONG_SIZE)
        }
        return ChallengeOutcome.Declare(challenge)
    }

    /** Compares the gateway's echo with this device's own address. */
    @JvmStatic
    fun bindingOutcome(declared: String, local: String?): BindingOutcome {
        if (local.isNullOrEmpty()) return BindingOutcome.UNKNOWN_LOCAL
        return if (declared == local) BindingOutcome.BOUND else BindingOutcome.MISMATCH
    }

    /**
     * The capability tokens worth storing, bounded on the way in.
     *
     * Oversized tokens are dropped **before** the count is applied, so a
     * gateway cannot pad its list to evict the tokens that matter.
     */
    @JvmStatic
    fun capabilityTokens(json: JSONObject): List<String> {
        val raw = json.optJSONArray("tokens") ?: return emptyList()
        val kept = mutableListOf<String>()
        for (i in 0 until raw.length()) {
            val token = raw.opt(i) as? String ?: continue
            if (token.isEmpty()) continue
            if (token.toByteArray(Charsets.UTF_8).size > MAX_CAPABILITY_TOKEN_BYTES) continue
            kept.add(token)
            if (kept.size == MAX_CAPABILITY_TOKENS) break
        }
        return kept
    }

    // ---- Message ids ------------------------------------------------------

    private val MESSAGE_ID_RE = Regex("[A-Za-z0-9._-]{1,64}")

    /**
     * The gateway's own rule for a client-supplied id.
     *
     * Applied before sending, not after: an id the gateway would refuse is
     * replaced *there* by one it mints, and the verdict then comes back under a
     * name nothing here is waiting on. Message ids are UUIDs, which pass; this
     * is what keeps that from being an assumption.
     */
    @JvmStatic
    fun sanitizeMessageId(candidate: String?): String? =
        if (candidate != null && MESSAGE_ID_RE.matches(candidate)) candidate else null

    // ---- Frames this client sends -----------------------------------------

    @JvmStatic
    fun identifyJson(deviceId: String): String? =
        try {
            JSONObject().apply {
                put("type", "Identify")
                put("device_id", deviceId)
                put("protocol_version", PROTOCOL_VERSION)
            }.toString()
        } catch (e: Exception) {
            null
        }

    @JvmStatic
    fun declarationJson(address: String, publicKey: ByteArray, signature: ByteArray): String? =
        try {
            JSONObject().apply {
                put("type", "DeclareAddress")
                put("address", address)
                put("public_key", Base64.encodeToString(publicKey, Base64.NO_WRAP))
                put("signature", Base64.encodeToString(signature, Base64.NO_WRAP))
            }.toString()
        } catch (e: Exception) {
            null
        }

    @JvmStatic
    fun sendMessageJson(
        messageId: String,
        recipient: String,
        content: String,
        replyToMsg: String?
    ): String? =
        try {
            JSONObject().apply {
                put("type", "SendMessage")
                put("recipient", recipient)
                put("content", content)
                put("encoding", "base64")
                put("message_id", messageId)
                if (!replyToMsg.isNullOrEmpty()) put("reply_to_msg", replyToMsg)
            }.toString()
        } catch (e: Exception) {
            null
        }

    /**
     * One frame for the whole batch, which is the shape the contract takes and
     * the opposite of the relay's one-peer-per-frame `CheckPresence`.
     */
    @JvmStatic
    fun checkPresenceJson(peers: List<String>): String? {
        val asked = peers.take(MAX_PRESENCE_PEERS)
        if (asked.isEmpty()) return null
        return try {
            JSONObject().apply {
                put("type", "CheckPresence")
                put("peers", JSONArray(asked))
            }.toString()
        } catch (e: Exception) {
            null
        }
    }

    // ---- Frames this client reads -----------------------------------------

    /**
     * A verdict: the id it settles, and the reason if it is a refusal.
     *
     * `reason` is `null` for `MessageSent` and the gateway's own text for
     * `DeliveryError`. It is passed to the core verbatim: the classifier
     * matches the `recipient_unreachable` prefix and discards the rest, so
     * nothing here needs to understand it.
     */
    data class Verdict(val messageId: String, val reason: String?, val recipient: String?) {
        val sent: Boolean
            get() = reason == null
    }

    @JvmStatic
    fun parseVerdict(json: JSONObject, type: String): Verdict? {
        val messageId = json.optString("message_id", "")
        // Nothing to settle. The gateway mints an id for a submission that
        // carried none, but this client always sends one, so a verdict with no
        // id is not ours to act on.
        if (messageId.isEmpty()) return null
        val recipient = json.optString("recipient", "").ifEmpty { null }
        if (type == "MessageSent") return Verdict(messageId, null, recipient)
        val reason = json.optString("reason", "").ifEmpty { "DeliveryError" }
        return Verdict(messageId, reason, recipient)
    }

    data class PresenceAnswer(val peer: String, val online: Boolean, val lastSeenMs: Long?)

    @JvmStatic
    fun parsePresence(json: JSONObject): PresenceAnswer? {
        val peer = json.optString("peer", "")
        if (peer.isEmpty()) return null
        // A missing or non-boolean `online` is not readable as "offline": that
        // would manufacture a claim the gateway never made, and a claim is what
        // drives parking.
        if (!json.has("online") || json.opt("online") !is Boolean) return null
        val online = json.getBoolean("online")
        val lastSeen = if (json.opt("last_seen_ms") is Number) json.optLong("last_seen_ms") else null
        return PresenceAnswer(peer, online, lastSeen)
    }
}
