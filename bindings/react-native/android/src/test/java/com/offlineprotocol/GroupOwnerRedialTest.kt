package com.offlineprotocol

import org.junit.Assert.assertEquals
import org.junit.Assert.assertFalse
import org.junit.Assert.assertNull
import org.junit.Assert.assertTrue
import org.junit.Test

/**
 * [GroupOwnerRedial]: when a Wi-Fi Direct group client dials its owner again.
 * The manager's own group handling needs a device (docs/bridges C9), so the
 * decision it makes after a stream ends is pinned here.
 */
class GroupOwnerRedialTest {

    private fun next(
        ended: String = "192.168.49.1",
        delivered: Boolean = false,
        held: Boolean = false,
        running: Boolean = true,
        isGroupOwner: Boolean = false,
        owner: String? = "192.168.49.1",
        currentDelayMs: Long = 8_000,
    ) = GroupOwnerRedial.next(
        ended = ended,
        delivered = delivered,
        held = held,
        running = running,
        isGroupOwner = isGroupOwner,
        owner = owner,
        currentDelayMs = currentDelayMs,
        initialDelayMs = 1_000,
        maxDelayMs = 60_000,
    )

    @Test
    fun `a refusing owner is redialled on a doubling delay`() {
        assertEquals(GroupOwnerRedial.Plan("192.168.49.1", 8_000, 16_000), next())
    }

    @Test
    fun `the delay stops doubling at the ceiling`() {
        assertEquals(60_000L, next(currentDelayMs = 60_000)?.nextDelayMs)
    }

    @Test
    fun `a stream that carried its peer starts the ladder over`() {
        assertEquals(GroupOwnerRedial.Plan("192.168.49.1", 1_000, 2_000), next(delivered = true))
    }

    @Test
    fun `a stream the owner proved but refused climbs the ladder`() {
        // The owner's stale stream wins there: a reset here redialed every second.
        assertEquals(GroupOwnerRedial.Plan("192.168.49.1", 8_000, 16_000), next(delivered = false))
    }

    @Test
    fun `no redial while another stream holds the owner's address`() {
        assertNull(next(held = true))
        assertNull(next(held = true, delivered = true))
    }

    @Test
    fun `a group switch dials the new owner at once`() {
        // Regression pin: the new owner's own dial lost the one-outbound-socket
        // flag to the old stream winding down, and the old stream's redial then
        // refused to dial an owner other than its own. Nothing dialled again.
        assertEquals(
            GroupOwnerRedial.Plan("192.168.49.7", 1_000, 2_000),
            next(ended = "192.168.49.1", owner = "192.168.49.7"),
        )
    }

    @Test
    fun `nothing is dialled without a group, as its owner, or once stopped`() {
        assertNull(next(owner = null))
        assertNull(next(isGroupOwner = true))
        assertNull(next(running = false))
    }

    // --- Leaving a group whose owner is gone -------------------------------------

    private fun after(
        count: Int,
        proved: Boolean = false,
        sameOwner: Boolean = true,
        currentDelayMs: Long = 60_000,
    ) = GroupOwnerRedial.unprovedAtCeilingAfter(count, proved, sameOwner, currentDelayMs, maxDelayMs = 60_000)

    @Test
    fun `only dials at the ceiling count toward leaving`() {
        // Below the ceiling an owner may be restarting its application.
        assertEquals(0, after(0, currentDelayMs = 32_000))
        assertEquals(1, after(0))
        assertEquals(2, after(1))
    }

    @Test
    fun `a proved stream or another owner starts the count over`() {
        assertEquals(0, after(2, proved = true))
        assertEquals(0, after(2, sameOwner = false))
    }

    @Test
    fun `a client leaves an application's group whose owner keeps proving nothing`() {
        // The group outlives the owner's process: a dead owner held its
        // clients in a group that carried nothing, and a client in a group
        // never formed again.
        val n = GroupOwnerRedial.LEAVE_AFTER_UNPROVED_AT_CEILING
        assertFalse(GroupOwnerRedial.shouldLeave(n - 1, applicationGroup = true))
        assertTrue(GroupOwnerRedial.shouldLeave(n, applicationGroup = true))
    }

    @Test
    fun `a group paired in the system settings is never left`() {
        assertFalse(GroupOwnerRedial.shouldLeave(1_000, applicationGroup = false))
    }
}
