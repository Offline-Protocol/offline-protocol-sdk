package com.offlineprotocol

/**
 * Picks the one service instance a central handshakes with when a remote phone
 * presents several.
 *
 * Mirrors iOS's BleServiceInstanceSelection.swift. Keep in sync.
 *
 * A phone running more than one SDK app presents one instance of the service
 * per app, all behind a single BLE link (see [BleAppTag]). The link still
 * carries exactly one peer: the peripheral role attributes an inbound write to
 * a sender by the link it arrived on, so a second identity on the same link
 * would have its hop-0 control frames refused by the core's
 * `validate_transport_sender`. The central therefore chooses one instance, and
 * everything after the choice (the DEVICE_ID and IDENTITY reads, the
 * subscription, every write) goes to that instance only.
 *
 * ## Why it never refuses the link
 *
 * A phone running only apps other than ours is still a mesh neighbour: relay
 * forwarding is keyed on the recipient and never on the app, so that phone
 * carries our traffic today. Refusing it would regress relaying to fix
 * discovery. When no instance is ours the choice falls back to the first
 * instance, which is what the central did before this object existed.
 *
 * ## Why a single instance never reaches this object
 *
 * The callers only consult it with two or more instances. A link with one
 * instance runs the handshake exactly as it did before the tag existed and does
 * not read the tag at all, so no working deployment changes behaviour.
 */
object BleServiceInstanceSelection {

    /**
     * Stable diagnostic reasons, shared with the Swift mirror so a trace reads
     * the same on both platforms.
     */
    object Reason {
        /** An instance served this app's tag. */
        const val SAME_APP = "same_app"

        /**
         * No instance served this app's tag, and exactly one served none: a
         * build that predates the tag, most plausibly this app's own.
         */
        const val SOLE_UNTAGGED = "sole_untagged"

        /** No basis to prefer any instance; the first one, as before the tag. */
        const val FIRST_INSTANCE = "first_instance"

        /**
         * No instance can complete the handshake. The first is returned so the
         * handshake refuses it for the characteristic it lacks, as it did
         * before the tag.
         */
        const val NONE_CAN_HANDSHAKE = "none_can_handshake"
    }

    /**
     * What one service instance offered, in the order the platform reported the
     * instances.
     *
     * @property tag the APP_TAG value, or null when the instance serves none (a
     *   build that predates the tag) or the read failed or timed out.
     * @property canHandshake whether the instance exposes both DEVICE_ID and
     *   IDENTITY. One that lacks either can never complete the handshake.
     */
    class Candidate(val tag: ByteArray?, val canHandshake: Boolean)

    /**
     * @property index index into the candidates passed to [select].
     * @property reason one of [Reason].
     */
    data class Selection(val index: Int, val reason: String)

    /**
     * Chooses an instance. [candidates] is never empty at the call sites; an
     * empty list returns index 0 with [Reason.NONE_CAN_HANDSHAKE].
     *
     * In order: the first handshake-capable instance serving [ownTag]; else,
     * when exactly one handshake-capable instance serves no tag, that one; else
     * the first handshake-capable instance; else index 0.
     */
    @JvmStatic
    fun select(candidates: List<Candidate>, ownTag: ByteArray): Selection {
        val usable = candidates.indices.filter { candidates[it].canHandshake }

        usable.firstOrNull { candidates[it].tag?.contentEquals(ownTag) == true }?.let {
            return Selection(it, Reason.SAME_APP)
        }

        val untagged = usable.filter { candidates[it].tag == null }
        if (untagged.size == 1) {
            return Selection(untagged[0], Reason.SOLE_UNTAGGED)
        }

        usable.firstOrNull()?.let { return Selection(it, Reason.FIRST_INSTANCE) }
        return Selection(0, Reason.NONE_CAN_HANDSHAKE)
    }
}
