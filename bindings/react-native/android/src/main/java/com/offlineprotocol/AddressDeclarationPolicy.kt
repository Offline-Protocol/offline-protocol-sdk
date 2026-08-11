package com.offlineprotocol

import android.util.Base64
import org.json.JSONObject
import java.io.ByteArrayOutputStream

/**
 * Builds the `DeclareAddress` proof a relay connection needs to be routed by
 * address instead of by account name.
 *
 * Mirrors iOS's AddressDeclarationPolicy.swift — keep in sync.
 *
 * ## Why the declaration exists
 *
 * The relay authenticates a JWT and knows the connection by its *account name*,
 * but since the addressing cutover the core stamps `Message.sender` with
 * `local_address()`. A relay that attributes an inbound frame by account name
 * therefore hands the receiver a `transport_peer_id` that cannot match the
 * sender it is strict-matched against in `validate_transport_sender`, and every
 * security-gated control frame (`__MLS_KEY_PKG__`, `__MLS_WELCOME__`) is
 * rejected — no MLS session can be established over the relay at all. Declaring
 * closes that, and is also what makes an `off1…` recipient resolvable in the
 * relay's registry.
 *
 * ## Why the account name is inside the signed bytes
 *
 * The signing key here is the identity key that also signs mesh control frames
 * under `offline-ctrl-v1`. If the proof were a signature over a bare
 * relay-chosen challenge, a hostile relay could hand out a challenge shaped
 * like a control-frame payload and harvest a signature that replays as a
 * control frame from this peer. Binding the domain **and** the account makes
 * the signature meaningful only as "this account, on this relay, holds this
 * key" — it cannot be replayed under another account, nor onto another
 * connection, since the challenge is minted per connection.
 *
 * The layout is fixed by the relay's `address_binding::address_proof_payload`
 * and pinned there by a hex vector, which
 * `AddressDeclarationPolicyTest.proofPayloadMatchesThePinnedRelayVector`
 * mirrors byte for byte. Neither side may change it alone.
 *
 * ## Why every failure is a skip rather than an error
 *
 * A connection that does not declare keeps working exactly as it did before
 * addresses existed — the relay attributes it by account name, which is the
 * legacy path and still the only path an older relay has. So the absence of a
 * capability, of a challenge, or of a local identity is a reason to stay quiet,
 * not to fail a connection that is otherwise fine.
 */
object AddressDeclarationPolicy {

    /**
     * The relay capability token gating the whole exchange. A relay that does
     * not advertise it also omits `address_challenge`, and would parse a
     * `DeclareAddress` into nothing and answer nothing — so the token is the
     * tell, not a timeout.
     */
    const val CAPABILITY = "address_routing_v1"

    /**
     * Domain separator prefixing the signed payload. Must not prefix, nor be
     * prefixed by, the core's `offline-ctrl-v1` control-frame domain; the relay
     * pins that relation in
     * `the_proof_domain_cannot_collide_with_control_message_signing`.
     */
    const val PROOF_DOMAIN = "offline-relay-addr-v1"

    /**
     * The relay mints exactly this many challenge bytes. A frame carrying any
     * other length is malformed, and signing it would produce a proof that
     * cannot verify — better to skip and say so.
     */
    const val CHALLENGE_LENGTH = 32

    /**
     * Stable diagnostic reasons, shared with the Swift mirror so a field report
     * reproduces under the same string on both platforms.
     */
    object Reason {
        /**
         * The relay does not advertise `address_routing_v1` — an older
         * deployment. Expected, and the reason this is not an error.
         */
        const val CAPABILITY_ABSENT = "capability_absent"

        /** Capability advertised but no `address_challenge` came with it. */
        const val CHALLENGE_ABSENT = "challenge_absent"

        /**
         * The challenge was not standard base64, or did not decode to exactly
         * [CHALLENGE_LENGTH] bytes.
         */
        const val CHALLENGE_MALFORMED = "challenge_malformed"

        /**
         * The `Authenticated` frame carried no account name to bind the proof
         * to. Never sign a locally-chosen substitute here: the relay verifies
         * against the name *it* resolved, so a guess produces a signature that
         * cannot verify and is indistinguishable, in the relay's logs, from an
         * attack.
         */
        const val ACCOUNT_ABSENT = "account_absent"

        /**
         * `localAddress()` was null — MLS is not initialized, so there is no
         * identity to prove. An app running with encryption disabled stays in
         * account-name space by construction.
         */
        const val ADDRESS_UNAVAILABLE = "address_unavailable"

        /** The identity key or the signature could not be produced. */
        const val SIGNING_FAILED = "signing_failed"

        /** The proof was built but the frame could not be serialized. */
        const val FRAME_UNSERIALIZABLE = "frame_unserializable"
    }

    sealed class Outcome {
        /** Sign [proofPayload] and send the declaration. */
        data class Declare(val account: String, val challenge: ByteArray) : Outcome() {
            // ByteArray needs structural equality by hand, and the tests
            // compare whole outcomes.
            override fun equals(other: Any?): Boolean {
                if (this === other) return true
                if (other !is Declare) return false
                return account == other.account && challenge.contentEquals(other.challenge)
            }

            override fun hashCode(): Int = 31 * account.hashCode() + challenge.contentHashCode()
        }

        /** Send nothing; [reason] is a [Reason]. */
        data class Skip(val reason: String) : Outcome()
    }

    /**
     * Decides whether this connection can declare, from the `Authenticated`
     * frame alone.
     *
     * Deliberately does **not** take the local address: this runs before any
     * FFI call so that the common skip (an older relay) costs no acquisition of
     * the protocol mutex. The caller fetches the address only after a
     * [Outcome.Declare], and reports [Reason.ADDRESS_UNAVAILABLE] if it is null.
     *
     * @param capabilities the `capabilities` array, verbatim.
     * @param addressChallenge the `address_challenge` field — base64 of the raw
     *   challenge bytes, absent on relays without the capability.
     * @param username the `username` field **as the relay sent it**. Callers
     *   must not substitute a local fallback (see [Reason.ACCOUNT_ABSENT]).
     */
    @JvmStatic
    fun decide(
        capabilities: List<String>,
        addressChallenge: String?,
        username: String?
    ): Outcome {
        if (!capabilities.contains(CAPABILITY)) {
            return Outcome.Skip(Reason.CAPABILITY_ABSENT)
        }
        if (addressChallenge.isNullOrEmpty()) {
            return Outcome.Skip(Reason.CHALLENGE_ABSENT)
        }
        // Standard alphabet — the relay encodes with the same, and the
        // url-safe alphabet's '-'/'_' are rejected outright here. (This decoder
        // tolerates missing padding where the relay's does not; that can only
        // make us accept a challenge the relay would never mint, and the length
        // check below still has to pass.)
        val challenge = try {
            Base64.decode(addressChallenge, Base64.NO_WRAP)
        } catch (e: IllegalArgumentException) {
            return Outcome.Skip(Reason.CHALLENGE_MALFORMED)
        }
        if (challenge.size != CHALLENGE_LENGTH) {
            return Outcome.Skip(Reason.CHALLENGE_MALFORMED)
        }
        if (username.isNullOrEmpty()) {
            return Outcome.Skip(Reason.ACCOUNT_ABSENT)
        }
        return Outcome.Declare(username, challenge)
    }

    /**
     * The exact bytes the relay verifies the signature over.
     *
     *     "offline-relay-addr-v1" ‖ u32be(account.utf8.size) ‖ account.utf8 ‖ challenge
     *
     * No separators, no terminators. The length prefix is what makes the
     * concatenation unambiguous — without it, an account name ending in bytes
     * that look like the start of a challenge could be re-split, so two
     * different (account, challenge) pairs would share one payload.
     */
    @JvmStatic
    fun proofPayload(account: String, challenge: ByteArray): ByteArray {
        val accountBytes = account.toByteArray(Charsets.UTF_8)
        val out = ByteArrayOutputStream(
            PROOF_DOMAIN.length + 4 + accountBytes.size + challenge.size
        )
        out.write(PROOF_DOMAIN.toByteArray(Charsets.US_ASCII))
        // Big-endian, and of the UTF-8 *byte* count — not the character count,
        // which differs for any non-ASCII account name.
        val length = accountBytes.size
        out.write((length ushr 24) and 0xFF)
        out.write((length ushr 16) and 0xFF)
        out.write((length ushr 8) and 0xFF)
        out.write(length and 0xFF)
        out.write(accountBytes)
        out.write(challenge)
        return out.toByteArray()
    }

    /**
     * The `DeclareAddress` frame, serialized.
     *
     * Owns the base64 encoding so the wire contract lives in one place per
     * platform: standard alphabet, padded, no line breaks — a 32-byte key
     * encodes to 44 characters and a 64-byte signature to 88. The relay decodes
     * with a strict engine and refuses anything else (NO_WRAP matters: the
     * default flags append a newline, which would not survive its decoder).
     *
     * Returns null only if serialization fails, which three `String` values
     * cannot cause; the caller reports [Reason.FRAME_UNSERIALIZABLE] rather
     * than putting a malformed frame on the wire.
     */
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
}
