package com.offlineprotocol

import android.util.Base64
import org.json.JSONObject
import org.junit.Assert.assertEquals
import org.junit.Assert.assertFalse
import org.junit.Assert.assertNotNull
import org.junit.Assert.assertTrue
import org.junit.Test
import org.junit.runner.RunWith
import org.robolectric.RobolectricTestRunner
import org.robolectric.annotation.Config

/**
 * Pins the relay address declaration: the exact bytes signed, and the four
 * conditions under which a connection stays in account-name space.
 *
 * Mirrors iOS's AddressDeclarationPolicyTests — keep in sync.
 *
 * The payload test is a cross-repo pin. Its expected value is copied from the
 * relay's own `address_proof_payload_matches_the_pinned_vector` (relay-server
 * src/address_binding.rs), so the two implementations cannot drift apart
 * silently — which matters more than usual here, because a wrong payload is not
 * a compile error or a parse error on either side. It is a signature that
 * simply does not verify, reported by the relay as `AddressError`, and
 * indistinguishable in its logs from an attack.
 */
/// Robolectric, not plain JUnit: `android.util.Base64` is a framework class and
/// the policy owns the encoding on purpose (it is half the wire contract), so a
/// stubbed one would leave that half untested. Pinned to this module's
/// `minSdkVersion` like the other Robolectric suites here.
@RunWith(RobolectricTestRunner::class)
@Config(sdk = [24])
class AddressDeclarationPolicyTest {

    /** A real derivation from the core's address golden vectors. */
    private val address = "off1qysluvwl5922yctzd0u9gpr06gn3k7ldfvgtwgvn"

    /** The challenge from the relay's pinned vector: bytes 0x00…0x1f. */
    private val vectorChallenge = ByteArray(32) { it.toByte() }

    private fun hex(bytes: ByteArray): String =
        bytes.joinToString("") { String.format("%02x", it) }

    private fun b64(bytes: ByteArray): String = Base64.encodeToString(bytes, Base64.NO_WRAP)

    private fun capabilities(vararg extra: String): List<String> =
        listOf("group_delivery_v3") + extra

    // ---- The signed bytes ----

    /**
     * Byte-for-byte against the relay's pinned vector. Any drift in the domain
     * string, the length prefix's width or endianness, the UTF-8 encoding, or
     * the concatenation order fails here rather than in the field.
     */
    @Test
    fun proofPayloadMatchesThePinnedRelayVector() {
        assertEquals(
            "6f66666c696e652d72656c61792d616464722d7631" +
                "00000005" +
                "616c696365" +
                "000102030405060708090a0b0c0d0e0f101112131415161718191a1b1c1d1e1f",
            hex(AddressDeclarationPolicy.proofPayload("alice", vectorChallenge))
        )
    }

    /**
     * The length prefix is big-endian. A little-endian bridge writes `05000000`
     * here and produces a signature the relay refuses — the one error the
     * pinned vector exists to catch, isolated so a failure names it.
     */
    @Test
    fun accountLengthPrefixIsBigEndian() {
        val payload = AddressDeclarationPolicy.proofPayload("alice", vectorChallenge)
        val domainLength = AddressDeclarationPolicy.PROOF_DOMAIN.length
        assertEquals("00000005", hex(payload.copyOfRange(domainLength, domainLength + 4)))
    }

    /**
     * The prefix counts UTF-8 bytes, not characters. The relay reads the same
     * bytes back out, so a character count silently mis-frames every account
     * name outside ASCII.
     */
    @Test
    fun accountLengthCountsUtf8BytesNotCharacters() {
        val account = "zoë" // 3 characters, 4 UTF-8 bytes
        val payload = AddressDeclarationPolicy.proofPayload(account, vectorChallenge)
        val domainLength = AddressDeclarationPolicy.PROOF_DOMAIN.length
        assertEquals("00000004", hex(payload.copyOfRange(domainLength, domainLength + 4)))
        assertEquals(domainLength + 4 + 4 + 32, payload.size)
    }

    /**
     * The domain must not prefix, nor be prefixed by, **any** of the core's
     * control-frame domains. If one did, a relay-chosen challenge could steer
     * this signature into the control-message domain and replay as a frame from
     * this peer. The relay pins the same relation from its side.
     *
     * Enumerated rather than checked against one, because the core gained a
     * second control domain when the signing payload started binding the
     * frame's timestamp, and a test that checked only the first would have gone
     * quiet about the new one while still reading as though it covered the
     * question.
     */
    @Test
    fun proofDomainCannotCollideWithControlFrameSigning() {
        val controlDomains = listOf("offline-ctrl-v1", "offline-ctrl-v2")
        val proofDomain = AddressDeclarationPolicy.PROOF_DOMAIN
        for (controlDomain in controlDomains) {
            assertFalse(
                "proof domain must not be prefixed by $controlDomain",
                proofDomain.startsWith(controlDomain),
            )
            assertFalse(
                "$controlDomain must not be prefixed by the proof domain",
                controlDomain.startsWith(proofDomain),
            )
        }
    }

    /**
     * The payload starts with the domain, so a signature over the bare
     * challenge — the naive implementation — can never equal it. The relay
     * refuses that shape explicitly (`a_bare_challenge_signature_is_refused`).
     */
    @Test
    fun payloadIsNeverTheBareChallenge() {
        val payload = AddressDeclarationPolicy.proofPayload("alice", vectorChallenge)
        assertFalse(payload.contentEquals(vectorChallenge))
        assertTrue(
            hex(payload).startsWith(
                hex(AddressDeclarationPolicy.PROOF_DOMAIN.toByteArray(Charsets.US_ASCII))
            )
        )
    }

    // ---- The decision ----

    /**
     * The happy path: capability, a well-formed challenge, and an account name
     * the relay itself supplied.
     */
    @Test
    fun declaresWhenCapabilityChallengeAndAccountArePresent() {
        assertEquals(
            AddressDeclarationPolicy.Outcome.Declare("alice", vectorChallenge),
            AddressDeclarationPolicy.decide(
                capabilities(AddressDeclarationPolicy.CAPABILITY),
                b64(vectorChallenge),
                "alice"
            )
        )
    }

    /**
     * An older relay. It omits the capability, would parse a `DeclareAddress`
     * into nothing and answer nothing at all — so the token is what gates the
     * send, and its absence is expected rather than exceptional.
     */
    @Test
    fun skipsWhenRelayLacksTheCapability() {
        assertEquals(
            AddressDeclarationPolicy.Outcome.Skip(
                AddressDeclarationPolicy.Reason.CAPABILITY_ABSENT
            ),
            AddressDeclarationPolicy.decide(capabilities(), b64(vectorChallenge), "alice")
        )
    }

    /** Capability without a challenge: nothing to sign. */
    @Test
    fun skipsWhenChallengeIsAbsent() {
        assertEquals(
            AddressDeclarationPolicy.Outcome.Skip(
                AddressDeclarationPolicy.Reason.CHALLENGE_ABSENT
            ),
            AddressDeclarationPolicy.decide(
                capabilities(AddressDeclarationPolicy.CAPABILITY),
                null,
                "alice"
            )
        )
    }

    /** Not base64 at all. */
    @Test
    fun skipsWhenChallengeIsNotBase64() {
        assertEquals(
            AddressDeclarationPolicy.Outcome.Skip(
                AddressDeclarationPolicy.Reason.CHALLENGE_MALFORMED
            ),
            AddressDeclarationPolicy.decide(
                capabilities(AddressDeclarationPolicy.CAPABILITY),
                "not base64!!",
                "alice"
            )
        )
    }

    /**
     * Decodes, but not to 32 bytes. Signing it would produce a proof that
     * cannot verify, so it is refused before the FFI is touched.
     */
    @Test
    fun skipsWhenChallengeIsWrongLength() {
        assertEquals(
            AddressDeclarationPolicy.Outcome.Skip(
                AddressDeclarationPolicy.Reason.CHALLENGE_MALFORMED
            ),
            AddressDeclarationPolicy.decide(
                capabilities(AddressDeclarationPolicy.CAPABILITY),
                b64(ByteArray(16) { 7 }),
                "alice"
            )
        )
    }

    /**
     * The relay decodes with a strict standard-alphabet engine, so a base64url
     * spelling of the same 32 bytes is not interchangeable.
     */
    @Test
    fun skipsWhenChallengeIsBase64Url() {
        // A 32-byte value whose standard encoding contains both '+' and '/'.
        val raw = ByteArray(32)
        raw[0] = 0xFB.toByte()
        raw[1] = 0xF0.toByte()
        val standard = b64(raw)
        val urlSafe = standard.replace('+', '-').replace('/', '_').trimEnd('=')
        assertFalse("the fixture must actually differ between alphabets", standard == urlSafe)
        assertEquals(
            AddressDeclarationPolicy.Outcome.Skip(
                AddressDeclarationPolicy.Reason.CHALLENGE_MALFORMED
            ),
            AddressDeclarationPolicy.decide(
                capabilities(AddressDeclarationPolicy.CAPABILITY),
                urlSafe,
                "alice"
            )
        )
    }

    /**
     * No account name on the frame. The proof binds the name the relay
     * resolved, so there is nothing local that could stand in: signing a
     * substitute (the profile, a device id) yields a signature that cannot
     * verify and reads as an attack in the relay's logs.
     */
    @Test
    fun skipsWhenAccountIsAbsent() {
        assertEquals(
            AddressDeclarationPolicy.Outcome.Skip(
                AddressDeclarationPolicy.Reason.ACCOUNT_ABSENT
            ),
            AddressDeclarationPolicy.decide(
                capabilities(AddressDeclarationPolicy.CAPABILITY),
                b64(vectorChallenge),
                null
            )
        )
    }

    @Test
    fun skipsWhenAccountIsEmpty() {
        assertEquals(
            AddressDeclarationPolicy.Outcome.Skip(
                AddressDeclarationPolicy.Reason.ACCOUNT_ABSENT
            ),
            AddressDeclarationPolicy.decide(
                capabilities(AddressDeclarationPolicy.CAPABILITY),
                b64(vectorChallenge),
                ""
            )
        )
    }

    // ---- The frame ----

    /**
     * Field names and the frame tag are the relay's `ClientMessage` variant;
     * all three values are base64 standard *with* padding, which is what the
     * relay's decoder requires.
     */
    @Test
    fun declarationFrameShape() {
        val publicKey = ByteArray(32) { 0xAB.toByte() }
        val signature = ByteArray(64) { 0xCD.toByte() }
        val json = AddressDeclarationPolicy.declarationJson(address, publicKey, signature)
        assertNotNull(json)
        val parsed = JSONObject(json!!)
        assertEquals("DeclareAddress", parsed.getString("type"))
        assertEquals(address, parsed.getString("address"))
        assertEquals(b64(publicKey), parsed.getString("public_key"))
        assertEquals(b64(signature), parsed.getString("signature"))
        assertEquals(4, parsed.length())
    }

    /**
     * Length and padding of the encoded material, as the relay expects to find
     * it: 32 bytes → 44 characters, 64 bytes → 88. NO_WRAP is what keeps the
     * newline `Base64.DEFAULT` would append off the wire.
     */
    @Test
    fun encodedMaterialIsPaddedStandardBase64() {
        val json = AddressDeclarationPolicy.declarationJson(
            address,
            ByteArray(32) { 0xAB.toByte() },
            ByteArray(64) { 0xCD.toByte() }
        )
        val parsed = JSONObject(json!!)
        val publicKey = parsed.getString("public_key")
        val signature = parsed.getString("signature")
        assertEquals(44, publicKey.length)
        assertTrue(publicKey.endsWith("="))
        assertEquals(88, signature.length)
        assertTrue(signature.endsWith("="))
        assertFalse(publicKey.contains("-") || publicKey.contains("_"))
        assertFalse(signature.contains("-") || signature.contains("_"))
        assertFalse(publicKey.contains("\n") || signature.contains("\n"))
    }
}
