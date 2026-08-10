package com.offlineprotocol

import org.junit.Assert.assertEquals
import org.junit.Test

/**
 * Pins the BLE discovery gate: a peer is announced only under an address it
 * proved. Mirrors iOS's PeerIdentityBindingTests — keep in sync.
 *
 * Regression pin: before this rule, `DEVICE_ID` was announced as-is. It carried
 * the app-chosen `profile`, which is not the peer's identity and is commonly a
 * shared constant like "default" — so peers collided on one id, and every
 * control frame they sent was dropped by the core's `validate_transport_sender`
 * because the `Message.sender` they stamp is their derived address, not their
 * profile.
 */
class PeerIdentityBindingTest {

    /**
     * Real derivations, from `crates/offline-protocol-core/src/address.rs`
     * golden vectors — 44 characters, canonical bech32m.
     */
    private val addressA = "off1qysluvwl5922yctzd0u9gpr06gn3k7ldfvgtwgvn"
    private val addressB = "off1qyqqqqqqqqqqqqqqqqqqqqqqqqqqqqqqqqn8antf"

    /** The happy path: what the peer advertises is what its key derives to. */
    @Test
    fun `matching advertisement is verified`() {
        assertEquals(
            PeerIdentityBinding.Outcome.Verified(addressA),
            PeerIdentityBinding.resolve(addressA, addressA),
        )
    }

    /**
     * The announced id comes from the proof, not the claim. They are equal here
     * by construction, so this pins *which* value is returned — the one a
     * future relaxation of the comparison must not be able to bypass.
     */
    @Test
    fun `verified peer id is the derived address`() {
        val outcome = PeerIdentityBinding.resolve(addressA, addressA)
        assertEquals(
            addressA,
            (outcome as PeerIdentityBinding.Outcome.Verified).peerId,
        )
    }

    /**
     * Two different addresses: the peer is claiming an id its key does not
     * derive to. This is the impersonation attempt the gate exists for.
     */
    @Test
    fun `mismatched address is rejected`() {
        assertEquals(
            PeerIdentityBinding.Outcome.Rejected(PeerIdentityBinding.Reason.ADDRESS_MISMATCH),
            PeerIdentityBinding.resolve(addressA, addressB),
        )
    }

    /**
     * The cutover case: a build that still advertises its profile. It must stay
     * invisible rather than being surfaced under an unproven name.
     */
    @Test
    fun `profile shaped advertisement is rejected`() {
        assertEquals(
            PeerIdentityBinding.Outcome.Rejected(PeerIdentityBinding.Reason.ADDRESS_MISMATCH),
            PeerIdentityBinding.resolve("default", addressA),
        )
    }

    /**
     * No identity read, or one that failed to verify. The caller passes null
     * for anything short of a decoded blob with a good signature, so this one
     * case covers absent, undecodable, and forged alike.
     */
    @Test
    fun `unverified identity is rejected`() {
        assertEquals(
            PeerIdentityBinding.Outcome.Rejected(PeerIdentityBinding.Reason.UNVERIFIED_IDENTITY),
            PeerIdentityBinding.resolve(addressA, null),
        )
    }

    /**
     * An identity alone is not enough. The peer must also advertise the
     * address, because `DEVICE_ID` is what the MTU map and the connection
     * registry key on — accepting identity-only would leave those unkeyed.
     */
    @Test
    fun `missing device id is rejected`() {
        assertEquals(
            PeerIdentityBinding.Outcome.Rejected(PeerIdentityBinding.Reason.MISSING_DEVICE_ID),
            PeerIdentityBinding.resolve(null, addressA),
        )
    }

    /**
     * An empty characteristic value is the same as an absent one. The central
     * already closes the link on this (`empty_device_id`); the rule must not
     * disagree.
     */
    @Test
    fun `empty device id is rejected`() {
        assertEquals(
            PeerIdentityBinding.Outcome.Rejected(PeerIdentityBinding.Reason.MISSING_DEVICE_ID),
            PeerIdentityBinding.resolve("", addressA),
        )
    }

    /**
     * Neither side available — the missing device id is reported, so the
     * diagnostic names the first thing the handshake failed to obtain.
     */
    @Test
    fun `both absent reports the device id`() {
        assertEquals(
            PeerIdentityBinding.Outcome.Rejected(PeerIdentityBinding.Reason.MISSING_DEVICE_ID),
            PeerIdentityBinding.resolve(null, null),
        )
    }

    /**
     * Bech32m permits an uppercase encoding, but the core emits canonical
     * lowercase from one shared `derive_address`. A peer advertising the other
     * casing did not derive its id the way we do, so it is refused rather than
     * normalised into agreement.
     */
    @Test
    fun `case differing advertisement is rejected`() {
        assertEquals(
            PeerIdentityBinding.Outcome.Rejected(PeerIdentityBinding.Reason.ADDRESS_MISMATCH),
            PeerIdentityBinding.resolve(addressA.uppercase(), addressA),
        )
    }
}
