package com.offlineprotocol

import org.junit.Assert.assertEquals
import org.junit.Assert.assertNull
import org.junit.Test

/**
 * [GroupOwnerRedial]: when a Wi-Fi Direct group client dials its owner again.
 * The manager's own group handling needs a device (docs/bridges C9), so the
 * decision it makes after a stream ends is pinned here.
 */
class GroupOwnerRedialTest {

    private fun next(
        ended: String = "192.168.49.1",
        proved: Boolean = false,
        running: Boolean = true,
        isGroupOwner: Boolean = false,
        owner: String? = "192.168.49.1",
        currentDelayMs: Long = 8_000,
    ) = GroupOwnerRedial.next(
        ended = ended,
        proved = proved,
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
    fun `a stream that proved its peer starts the ladder over`() {
        assertEquals(GroupOwnerRedial.Plan("192.168.49.1", 1_000, 2_000), next(proved = true))
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
}
