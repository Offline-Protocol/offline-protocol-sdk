package com.offlineprotocol

/**
 * Decides whether a discovered BLE peer may be announced to the protocol
 * layer, and under which identifier.
 *
 * Mirrors iOS's PeerIdentityBinding.swift — keep in sync.
 *
 * A peer serves two GATT characteristics: `DEVICE_ID` (`6E400003…`), a bare
 * string, and `IDENTITY` (`6E400004…`), an Ed25519 public key with a signature
 * over its mesh advertisement. Only the second proves anything. This object is
 * the rule that joins them: the advertised string is accepted only when it
 * equals the address derived from the key that signed the identity blob, and
 * the value handed onward is always the **derived** one.
 *
 * ## Why the derived address is what gets announced
 *
 * The two are equal whenever [resolve] returns [Outcome.Verified], so returning
 * the derived value costs nothing — but it means no code path can announce a
 * string that came from the unauthenticated characteristic. The announced id
 * becomes `Message.recipient` for every outbound frame, the key of the Rust
 * `peers` and `peer_mtus` maps, and the `transport_peer_id` the core matches
 * against `Message.sender` in `validate_transport_sender`. Sourcing it from the
 * proof rather than from the claim keeps a future relaxation of the comparison
 * from silently reopening the hole.
 *
 * ## Why a mismatch is fatal rather than degrading
 *
 * Announcing an unproven id is exactly the unauthenticated-advertisement hole
 * this closes: it seeds the routing table, the app's peer list, and the mesh
 * controller under a name the peer cannot prove. There is no useful "accept but
 * flag" state — the id is either load-bearing or it is not. A peer that serves
 * no identity, or one whose identity names a different address, is therefore
 * not surfaced at all.
 *
 * A build that still advertises its app-chosen profile in `DEVICE_ID` lands on
 * [Reason.ADDRESS_MISMATCH] and stays invisible. That is the intended cutover
 * behaviour: its frames would be rejected by the core's control gate anyway,
 * since the `Message.sender` it stamps is its derived address.
 */
object PeerIdentityBinding {

    /**
     * Stable diagnostic reasons, shared with the Swift mirror so a bug
     * reproduces under the same string on both platforms.
     */
    object Reason {
        /** The peer served no `DEVICE_ID`, or served an empty one. */
        const val MISSING_DEVICE_ID = "device_id_missing"

        /**
         * No verified identity: absent, undecodable, bad signature, or a key
         * that would not derive.
         */
        const val UNVERIFIED_IDENTITY = "identity_unverified"

        /** Both present, and they name different addresses. */
        const val ADDRESS_MISMATCH = "identity_address_mismatch"
    }

    sealed class Outcome {
        /** Announce the peer under [peerId]. */
        data class Verified(val peerId: String) : Outcome()

        /** Surface nothing and drop the link; [reason] is a [Reason]. */
        data class Rejected(val reason: String) : Outcome()
    }

    /**
     * Joins the advertised `DEVICE_ID` to the address derived from the peer's
     * verified `IDENTITY` key.
     *
     * @param advertisedDeviceId the raw `DEVICE_ID` characteristic value, or
     *   null if it was never read.
     * @param derivedAddress `deriveAddress(IDENTITY.publicKey)`, passed **only**
     *   when the identity blob decoded and its signature verified. Callers must
     *   not pass a derived address for an unverified blob — the signature is
     *   what makes the derivation mean anything.
     *
     * Comparison is exact. Addresses are canonical bech32m as produced by the
     * core's single `derive_address` implementation, so an equal-but-differently-
     * cased advertisement is a peer that did not derive its own id the way this
     * one did, and is refused rather than normalised into agreement.
     */
    @JvmStatic
    fun resolve(advertisedDeviceId: String?, derivedAddress: String?): Outcome {
        if (advertisedDeviceId.isNullOrEmpty()) {
            return Outcome.Rejected(Reason.MISSING_DEVICE_ID)
        }
        if (derivedAddress.isNullOrEmpty()) {
            return Outcome.Rejected(Reason.UNVERIFIED_IDENTITY)
        }
        if (advertisedDeviceId != derivedAddress) {
            return Outcome.Rejected(Reason.ADDRESS_MISMATCH)
        }
        return Outcome.Verified(derivedAddress)
    }
}
