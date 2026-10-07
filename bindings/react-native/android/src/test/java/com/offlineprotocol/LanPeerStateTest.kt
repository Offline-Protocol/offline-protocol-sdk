package com.offlineprotocol

import org.junit.Assert.assertEquals
import org.junit.Assert.assertFalse
import org.junit.Assert.assertNull
import org.junit.Assert.assertTrue
import org.junit.Test

/**
 * [ResolveQueue] and [NetworkChoice]: the LAN carrier's state, each rule one
 * a device found broken first. NsdManager and the timers need a device; this
 * is the bookkeeping they drive.
 */
class LanPeerStateTest {

    private fun queue(maxAlreadyActive: Int = 5) = ResolveQueue<String>({ it }, maxAlreadyActive)

    // --- one resolve at a time -------------------------------------------------

    @Test
    fun `one resolve is in flight at a time, in the order found`() {
        val q = queue()
        q.found("a")
        q.found("b")
        val first = q.next()!!
        assertEquals("a", first.item)
        assertNull("one at a time", q.next())
        assertTrue(q.answered(first))
        assertEquals("b", q.next()!!.item)
    }

    @Test
    fun `a service waits once however often it is queued`() {
        val q = queue()
        q.found("a")
        q.queue("a")
        q.found("a")
        q.next()!!.let { assertTrue(q.answered(it)) }
        assertNull(q.next())
    }

    // --- a service lost meanwhile ------------------------------------------------

    @Test
    fun `an answer for a service lost while resolving is not applied`() {
        // NSD sends one loss: a record written after it is never removed.
        val q = queue()
        q.found("a")
        val ticket = q.next()!!
        q.lost("a")
        assertFalse(q.answered(ticket))
        assertNull("nothing left to resolve", q.next())
    }

    @Test
    fun `a lost service is no longer queued, and cannot be queued again`() {
        val q = queue()
        q.found("a")
        q.found("b")
        q.lost("a")
        q.queue("a")
        assertEquals("b", q.next()!!.item)
    }

    // --- a resolve that gives up -------------------------------------------------

    @Test
    fun `a resolve that gave up can be queued again while reported`() {
        val q = queue()
        q.found("a")
        val ticket = q.next()!!
        assertTrue(q.gaveUp(ticket))
        q.queue("a")
        assertEquals("a", q.next()!!.item)
    }

    @Test
    fun `a late answer after the watchdog gave up is ignored, and the queue moved on`() {
        val q = queue()
        q.found("a")
        q.found("b")
        val stuck = q.next()!!
        assertTrue("the watchdog retires it", q.gaveUp(stuck))
        val next = q.next()!!
        assertEquals("b", next.item)
        assertFalse("the stuck one answering now changes nothing", q.answered(stuck))
        assertTrue(q.answered(next))
    }

    @Test
    fun `clearing retires the resolve in flight`() {
        val q = queue()
        q.found("a")
        val ticket = q.next()!!
        q.clear()
        assertFalse(q.answered(ticket))
        assertFalse(q.gaveUp(ticket))
        assertFalse(q.isReported("a"))
    }

    // --- ALREADY_ACTIVE ------------------------------------------------------------

    @Test
    fun `ALREADY_ACTIVE waits at the head of the queue a few times, then gives way`() {
        val q = queue(maxAlreadyActive = 2)
        q.found("a")
        q.found("b")
        repeat(2) {
            val ticket = q.next()!!
            assertEquals("a waits at the head", "a", ticket.item)
            assertTrue(q.waitForActive(ticket))
            assertNull("still in flight while it waits", q.next())
            assertTrue(q.release(ticket))
        }
        val third = q.next()!!
        assertEquals("a", third.item)
        assertFalse("waited enough: given up", q.waitForActive(third))
        assertTrue(q.gaveUp(third))
        assertEquals("the queue moves on", "b", q.next()!!.item)
    }

    @Test
    fun `an answer resets the ALREADY_ACTIVE count`() {
        val q = queue(maxAlreadyActive = 1)
        q.found("a")
        q.next()!!.let { assertTrue(q.waitForActive(it)); assertTrue(q.release(it)) }
        q.next()!!.let { assertTrue(q.answered(it)) }
        q.queue("a")
        q.next()!!.let { assertTrue("counted afresh", q.waitForActive(it)) }
    }

    @Test
    fun `a service lost while waiting out ALREADY_ACTIVE is not put back`() {
        val q = queue()
        q.found("a")
        val ticket = q.next()!!
        q.lost("a")
        assertTrue(q.waitForActive(ticket))
        assertTrue(q.release(ticket))
        assertNull(q.next())
    }

    // --- the network ---------------------------------------------------------------

    @Test
    fun `the first matching network is adopted and kept while another comes and goes`() {
        val n = NetworkChoice<String, Int>()
        assertEquals(NetworkChoice.Up.ADOPT, n.up("A", 1))
        assertEquals(NetworkChoice.Up.WAIT, n.up("B", 1))
        assertEquals("a change on another network moves nothing", NetworkChoice.Up.WAIT, n.up("B", 2))
        assertEquals(NetworkChoice.Up.UPDATE, n.up("A", 2))
        assertFalse(n.lost("B"))
        assertEquals("A", n.current)
    }

    @Test
    fun `losing the current network falls back to another still up`() {
        val n = NetworkChoice<String, Int>()
        n.up("A", 1)
        n.up("B", 7)
        assertTrue(n.lost("A"))
        assertNull(n.current)
        assertEquals("B" to 7, n.fallback())
        assertEquals("B", n.current)
    }

    @Test
    fun `losing the last network leaves none`() {
        val n = NetworkChoice<String, Int>()
        n.up("A", 1)
        assertTrue(n.lost("A"))
        assertNull(n.fallback())
        assertNull(n.current)
        assertEquals("the next one up is adopted", NetworkChoice.Up.ADOPT, n.up("C", 1))
    }

    @Test
    fun `clearing forgets every network`() {
        val n = NetworkChoice<String, Int>()
        n.up("A", 1)
        n.up("B", 1)
        n.clear()
        assertNull(n.current)
        assertNull(n.fallback())
    }
}
