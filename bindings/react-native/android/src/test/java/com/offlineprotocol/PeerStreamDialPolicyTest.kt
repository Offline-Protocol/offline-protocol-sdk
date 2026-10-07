package com.offlineprotocol

import org.junit.Assert.assertFalse
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
}
