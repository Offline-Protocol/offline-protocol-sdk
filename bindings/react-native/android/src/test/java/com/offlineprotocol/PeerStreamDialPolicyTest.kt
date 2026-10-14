package com.offlineprotocol

import org.junit.Assert.assertEquals
import org.junit.Assert.assertFalse
import org.junit.Assert.assertNotNull
import org.junit.Assert.assertNull
import org.junit.Assert.assertTrue
import org.junit.Test

/** Mirrors iOS's PeerStreamDialPolicyTests, case for case where both have one. */
class PeerStreamDialPolicyTest {
    private val limits = PeerStreamSockets.Limits()

    /** One machine on the LAN cannot fill the listener, and a full listener leaves room to dial. */
    @Test
    fun `inbound is bounded per host and leaves room to dial`() {
        val one = List(limits.maxInboundPerHost) { "10.0.0.9" }
        assertFalse(PeerStreamDialPolicy.admitsInbound("10.0.0.9", one, one.size, limits))
        assertTrue(PeerStreamDialPolicy.admitsInbound("10.0.0.7", one, one.size, limits))

        val many = List(limits.maxInbound) { "10.0.1.$it" }
        assertFalse(PeerStreamDialPolicy.admitsInbound("10.0.2.1", many, many.size, limits))
        assertTrue("dials keep a share of the budget", limits.maxInbound < limits.maxStreams)
        assertFalse(PeerStreamDialPolicy.admitsInbound("10.0.2.1", emptyList(), limits.maxStreams, limits))
    }

    @Test
    fun `unknown hosts share one bound`() {
        val unknown = List(limits.maxInboundPerHost) { null }
        assertFalse(PeerStreamDialPolicy.admitsInbound(null, unknown, unknown.size, limits))
    }

    private val peer = "off1qysluvwl5922yctzd0u9gpr06gn3k7ldfvgtwgvn"
    private val higher = PeerStreamDialPolicy.HIGHER_ADDRESS_DELAY_MS
    private val initial = PeerStreamDialPolicy.REDIAL_INITIAL_DELAY_MS

    @Test
    fun `the lower address dials at once and the higher waits`() {
        assertEquals(0L, PeerStreamDialPolicy().discovered(peer, weAreLower = true, held = false))
        assertEquals(higher, PeerStreamDialPolicy().discovered(peer, weAreLower = false, held = false))
    }

    @Test
    fun `no dial while one is under way or the address is held`() {
        val policy = PeerStreamDialPolicy()
        assertNotNull(policy.discovered(peer, weAreLower = true, held = false))
        assertNull("one dial at a time", policy.discovered(peer, weAreLower = true, held = false))
        assertNull(PeerStreamDialPolicy().discovered(peer, weAreLower = true, held = true))
    }

    @Test
    fun `an abandoned dial lets the next advert dial`() {
        val policy = PeerStreamDialPolicy()
        policy.discovered(peer, weAreLower = true, held = false)
        policy.abandoned(peer)
        assertEquals(0L, policy.discovered(peer, weAreLower = true, held = false))
    }

    @Test
    fun `the ladder doubles to its cap`() {
        val policy = PeerStreamDialPolicy()
        policy.discovered(peer, weAreLower = true, held = false)
        val delays = List(9) { policy.ended(peer, advertised = true, held = false) }
        assertEquals(
            listOf(1L, 2L, 4L, 8L, 16L, 32L, 60L, 60L, 60L).map { it * 1_000 },
            delays,
        )
    }

    @Test
    fun `a lost race does not climb the ladder`() {
        // The higher address's dial loses to the lower's stream: its stream
        // ends while the address is held. That is no redial, so no step.
        val policy = PeerStreamDialPolicy()
        repeat(5) {
            policy.discovered(peer, weAreLower = false, held = false)
            assertNull(policy.ended(peer, advertised = true, held = true))
        }
        // When the held stream later ends, the first redial is the first step.
        assertEquals(higher, policy.discovered(peer, weAreLower = false, held = false))
        assertEquals(initial, policy.ended(peer, advertised = true, held = false))
    }

    @Test
    fun `no redial once the advert is gone`() {
        val policy = PeerStreamDialPolicy()
        policy.discovered(peer, weAreLower = true, held = false)
        assertNull(policy.ended(peer, advertised = false, held = false))
        assertEquals(
            "the ended stream no longer counts as a dial under way",
            0L, policy.discovered(peer, weAreLower = true, held = false),
        )
    }

    @Test
    fun `a proved stream starts the ladder over`() {
        val policy = PeerStreamDialPolicy()
        policy.discovered(peer, weAreLower = true, held = false)
        policy.ended(peer, advertised = true, held = false)
        policy.ended(peer, advertised = true, held = false)
        policy.proved(peer)
        assertEquals(initial, policy.ended(peer, advertised = true, held = false))
    }

    @Test
    fun `reset forgets everything`() {
        val policy = PeerStreamDialPolicy()
        policy.discovered(peer, weAreLower = true, held = false)
        policy.ended(peer, advertised = true, held = false)
        policy.reset()
        assertEquals(0L, policy.discovered(peer, weAreLower = true, held = false))
        assertEquals(initial, policy.ended(peer, advertised = true, held = false))
    }

    /** A peer back before its old record expired is advertised by two records. */
    @Test
    fun `removing a stale record keeps the live one's address`() {
        var adverts = PeerStreamDialPolicy.adverts(listOf(peer to "old"), setOf("old"), emptyMap())
        adverts = PeerStreamDialPolicy.adverts(listOf(peer to "old", peer to "new"), setOf("new"), adverts)
        assertEquals("the record a change added is the newest", "new", adverts[peer])
        adverts = PeerStreamDialPolicy.adverts(listOf(peer to "new"), emptySet(), adverts)
        assertEquals("new", adverts[peer])
        assertNull(
            "the last record going takes the address",
            PeerStreamDialPolicy.adverts(emptyList<Pair<String, String>>(), emptySet(), adverts)[peer],
        )
    }

    @Test
    fun `the record in use stays over an older one`() {
        val records = listOf(peer to "a", peer to "b")
        for (current in listOf("a", "b")) {
            assertEquals(current, PeerStreamDialPolicy.adverts(records, emptySet(), mapOf(peer to current))[peer])
        }
    }

    /** A dial that found no free slot is tried again, later each time, and stays the one dial. */
    @Test
    fun `a dial with no slot is tried again on the ladder`() {
        val policy = PeerStreamDialPolicy()
        policy.discovered(peer, weAreLower = true, held = false)
        assertEquals(initial, policy.noSlot(peer))
        assertNull("one dial at a time", policy.discovered(peer, weAreLower = true, held = false))
        assertEquals(2 * initial, policy.noSlot(peer))
    }

    /** A record that answered without proving its address gives way, or leaves the address unadvertised. */
    @Test
    fun `an unprovable record is left out`() {
        val records = listOf(peer to "liar", peer to "real")
        assertEquals(
            "real",
            PeerStreamDialPolicy.adverts(records, setOf("liar"), mapOf(peer to "liar"), setOf("liar"))[peer],
        )
        assertNull(PeerStreamDialPolicy.adverts(listOf(peer to "liar"), emptySet(), emptyMap(), setOf("liar"))[peer])
    }
}
