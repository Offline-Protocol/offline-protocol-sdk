package com.offlineprotocol

import com.offlineprotocol.BleServiceInstanceSelection.Candidate
import com.offlineprotocol.BleServiceInstanceSelection.Reason
import com.offlineprotocol.BleServiceInstanceSelection.Selection
import org.junit.Assert.assertEquals
import org.junit.Test

/**
 * Pins which service instance a central binds to when a phone runs several SDK
 * apps. Mirrors iOS's BleServiceInstanceSelectionTests. Keep in sync.
 *
 * Regression pin: before this rule, iOS read DEVICE_ID and IDENTITY from every
 * instance into one per-peripheral slot and cross-checked whichever pair
 * landed, so a phone running two SDK apps was refused or announced under the
 * wrong app's identity at random. Android bound to the first instance always.
 */
class BleServiceInstanceSelectionTest {

    private val ours = byteArrayOf(0x01, 0x02, 0x03, 0x04, 0x05, 0x06, 0x07, 0x08)
    private val chat = byteArrayOf(0x11, 0x12, 0x13, 0x14, 0x15, 0x16, 0x17, 0x18)
    private val game = byteArrayOf(0x21, 0x22, 0x23, 0x24, 0x25, 0x26, 0x27, 0x28)

    private fun candidate(tag: ByteArray?, canHandshake: Boolean = true) =
        Candidate(tag, canHandshake)

    private fun select(vararg candidates: Candidate): Selection =
        BleServiceInstanceSelection.select(candidates.toList(), ours)

    /** The case this object exists for: our app's instance is not the first. */
    @Test
    fun `picks our app's instance wherever it sits`() {
        assertEquals(
            Selection(2, Reason.SAME_APP),
            select(candidate(chat), candidate(game), candidate(ours)),
        )
    }

    /** Two builds of one app on one phone: the first wins. */
    @Test
    fun `two instances of our app pick the first`() {
        assertEquals(
            Selection(1, Reason.SAME_APP),
            select(candidate(chat), candidate(ours), candidate(ours)),
        )
    }

    /**
     * A tag is compared by content, not identity: the central reads a fresh
     * array off the wire, never the one it computed.
     */
    @Test
    fun `tags compare by content`() {
        assertEquals(
            Selection(1, Reason.SAME_APP),
            select(candidate(chat), candidate(ours.copyOf())),
        )
    }

    /** Our tag beats an untagged instance: the untagged rule is a fallback. */
    @Test
    fun `our tag beats an untagged instance`() {
        assertEquals(
            Selection(1, Reason.SAME_APP),
            select(candidate(null), candidate(ours)),
        )
    }

    /**
     * One build predating the tag among tagged foreign apps: most plausibly
     * ours, and certainly not one of the apps that named themselves.
     */
    @Test
    fun `sole untagged instance among foreign apps is chosen`() {
        assertEquals(
            Selection(1, Reason.SOLE_UNTAGGED),
            select(candidate(chat), candidate(null), candidate(game)),
        )
    }

    /**
     * Only foreign apps: the phone is still a relay neighbour, bound as before
     * the tag existed.
     */
    @Test
    fun `all foreign falls back to the first instance`() {
        assertEquals(
            Selection(0, Reason.FIRST_INSTANCE),
            select(candidate(chat), candidate(game)),
        )
    }

    /** Several builds predating the tag: nothing to go on, so the first. */
    @Test
    fun `several untagged fall back to the first instance`() {
        assertEquals(
            Selection(0, Reason.FIRST_INSTANCE),
            select(candidate(null), candidate(null)),
        )
    }

    /**
     * An instance that cannot finish the handshake is never chosen over one
     * that can, even when it serves our tag.
     */
    @Test
    fun `instance that cannot handshake is skipped`() {
        assertEquals(
            Selection(1, Reason.FIRST_INSTANCE),
            select(candidate(ours, canHandshake = false), candidate(chat)),
        )
        assertEquals(
            Selection(1, Reason.SOLE_UNTAGGED),
            select(candidate(null, canHandshake = false), candidate(null), candidate(chat)),
        )
    }

    /**
     * Nothing usable: index 0, so the handshake refuses it for the
     * characteristic it lacks, exactly as before the tag.
     */
    @Test
    fun `no instance can handshake returns the first`() {
        assertEquals(
            Selection(0, Reason.NONE_CAN_HANDSHAKE),
            select(candidate(ours, canHandshake = false), candidate(chat, canHandshake = false)),
        )
        assertEquals(Selection(0, Reason.NONE_CAN_HANDSHAKE), select())
    }

    /**
     * The reason strings are shared with Swift; a rename on one side only would
     * split the diagnostics.
     */
    @Test
    fun `reason strings are stable`() {
        assertEquals("same_app", Reason.SAME_APP)
        assertEquals("sole_untagged", Reason.SOLE_UNTAGGED)
        assertEquals("first_instance", Reason.FIRST_INSTANCE)
        assertEquals("none_can_handshake", Reason.NONE_CAN_HANDSHAKE)
    }
}
