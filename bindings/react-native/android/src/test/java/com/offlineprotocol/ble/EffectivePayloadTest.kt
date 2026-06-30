package com.offlineprotocol.ble

import org.junit.Assert.assertEquals
import org.junit.Assert.assertNull
import org.junit.Test

/**
 * Unit tests for [computeEffectivePayload] — the per-peer ATT payload the BLE
 * transport flushes to the Rust fragmenter.
 *
 * The regression these guard against: a notify-subscribed peer whose peripheral
 * (notify-link) MTU was never observed must NOT be sized for the larger central
 * link, or a multi-fragment MLS Welcome egressed over the notify link overflows
 * it and is silently truncated on air, stalling 1:1 MLS convergence (the owner
 * loops "Welcome send confirmation timed out"). 514 is the typical central-link
 * payload; 185 is [BleTransportFacade]'s conservative fragment cap that every
 * notify link can carry.
 */
class EffectivePayloadTest {
    private val floor = 185

    @Test
    fun `floors to 185 when subscribed and peripheral MTU unobserved`() {
        // Central negotiated 514, notify-link MTU never reported, peer IS
        // notify-subscribed -> floor to 185 rather than collapse to 514.
        assertEquals(
            185,
            computeEffectivePayload(
                central = 514,
                peripheralStaged = null,
                notifySubscribed = true,
                floor = floor,
            ),
        )
    }

    @Test
    fun `a real peripheral MTU wins over the floor`() {
        // When the notify link's MTU IS observed, use min(central, peripheral);
        // the floor must never demote a link whose payload we actually know.
        assertEquals(
            200,
            computeEffectivePayload(
                central = 514,
                peripheralStaged = 200,
                notifySubscribed = true,
                floor = floor,
            ),
        )
    }

    @Test
    fun `min still applies when peripheral is the larger of the two`() {
        assertEquals(
            300,
            computeEffectivePayload(
                central = 300,
                peripheralStaged = 480,
                notifySubscribed = true,
                floor = floor,
            ),
        )
    }

    @Test
    fun `no floor when the peer is not notify-subscribed`() {
        // Not subscribed: an unknown peripheral term stays unknown, so the
        // effective payload is just the central link's.
        assertEquals(
            514,
            computeEffectivePayload(
                central = 514,
                peripheralStaged = null,
                notifySubscribed = false,
                floor = floor,
            ),
        )
    }

    @Test
    fun `null when nothing is known and not subscribed`() {
        // Neither link known and not subscribed -> clear the Rust entry (caller
        // lets the fragmenter revert to its own floor).
        assertNull(
            computeEffectivePayload(
                central = null,
                peripheralStaged = null,
                notifySubscribed = false,
                floor = floor,
            ),
        )
    }

    @Test
    fun `floors to 185 when subscribed even with no central link`() {
        // A peer subscribed to our notify with no central link and no observed
        // peripheral MTU still gets an explicit 185 floor (not a clear), so the
        // notify egress is bounded and Rust's fragment_fallback_count is not
        // falsely ticked for a recipient that is a registered direct peer.
        assertEquals(
            185,
            computeEffectivePayload(
                central = null,
                peripheralStaged = null,
                notifySubscribed = true,
                floor = floor,
            ),
        )
    }
}
