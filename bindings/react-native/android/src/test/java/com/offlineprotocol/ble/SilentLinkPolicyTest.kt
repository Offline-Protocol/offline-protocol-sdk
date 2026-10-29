package com.offlineprotocol.ble

import org.junit.Assert.assertEquals
import org.junit.Assert.assertTrue
import org.junit.Test

/**
 * [SilentLinkPolicy] decides when a client link that keeps taking writes has
 * stopped delivering them. The failure it exists for: an Android central wrote
 * to an iPhone for 90 seconds and more, every write reported sent, nothing ever
 * came back, and only restarting the Android app ended it.
 */
class SilentLinkPolicyTest {
    private val policy = SilentLinkPolicy<String, String>(ackWaitMs = 10_000L)

    @Test
    fun `a link that keeps taking writes and returns nothing is silent`() {
        // The 2026-10-29 log: a burst of fragments, then the core resending.
        policy.noteWrite("link", 0L)
        policy.noteWrite("link", 30L)
        policy.noteWrite("link", 10_000L)
        policy.noteWrite("link", 15_000L)
        assertEquals(emptyList<String>(), policy.silentLinks(19_999L))
        assertEquals(listOf("link"), policy.silentLinks(20_000L))
    }

    @Test
    fun `one unanswered write never makes a link silent`() {
        // The last frame of an exchange is often an acknowledgement, which
        // nothing answers.
        policy.noteWrite("link", 0L)
        assertEquals(emptyList<String>(), policy.silentLinks(3_600_000L))
    }

    @Test
    fun `the fragments of one message are one write`() {
        for (t in 0L until 9_000L step 30L) policy.noteWrite("link", t)
        assertEquals(emptyList<String>(), policy.silentLinks(60_000L))
    }

    @Test
    fun `a later write gets the acknowledgement timeout before it counts`() {
        // An acknowledgement, a quiet minute, then a message: the peer has ten
        // seconds to answer the message, however long ago the first write was.
        policy.noteWrite("link", 0L)
        policy.noteWrite("link", 60_000L)
        assertEquals(emptyList<String>(), policy.silentLinks(69_999L))
        assertEquals(listOf("link"), policy.silentLinks(70_000L))
    }

    @Test
    fun `anything heard from the peer clears the link`() {
        policy.noteWrite("link", 0L)
        policy.noteWrite("link", 10_000L)
        policy.noteHeard("link")
        assertEquals(emptyList<String>(), policy.silentLinks(20_000L))
    }

    @Test
    fun `a link the peer has answered is never judged again`() {
        // On two phones: after a redial the peer answered once, then held its
        // acknowledgements back for a session it lacked. Judging the link
        // again tore it down every 30 to 40 seconds while the core healed.
        policy.noteWrite("link", 0L)
        policy.noteHeard("link")
        for (t in 1_000L..600_000L step 5_000L) policy.noteWrite("link", t)
        assertEquals(emptyList<String>(), policy.silentLinks(600_000L))
    }

    @Test
    fun `links are judged apart, and a forgotten link starts unjudged`() {
        policy.noteWrite("dead", 0L)
        policy.noteWrite("dead", 10_000L)
        policy.noteWrite("live", 0L)
        policy.noteWrite("live", 10_000L)
        policy.noteHeard("live")
        assertEquals(listOf("dead"), policy.silentLinks(20_000L))

        policy.forget("dead")
        policy.noteWrite("dead", 20_000L)
        assertEquals(emptyList<String>(), policy.silentLinks(40_000L))

        // Forgetting also drops the answer: a redial must be answered anew.
        policy.forget("live")
        policy.noteWrite("live", 40_000L)
        policy.noteWrite("live", 50_000L)
        assertEquals(listOf("live"), policy.silentLinks(60_000L))
    }

    @Test
    fun `a peer is redialled for silence once until it is heard from`() {
        // A peer that only receives frames asking for no acknowledgement never
        // answers a link that works. Judged per link alone, every redial would
        // be torn down again 25 to 30 seconds later, for as long as it lasts.
        val peerOf = { _: String -> "peer" }
        policy.noteWrite("first", 0L)
        policy.noteWrite("first", 10_000L)
        assertEquals(listOf("first"), policy.linksToRedial(20_000L, peerOf))

        policy.noteWrite("redial", 25_000L)
        policy.noteWrite("redial", 35_000L)
        assertEquals(emptyList<String>(), policy.linksToRedial(45_000L, peerOf))
        // No longer judged, however long it stays silent.
        policy.noteWrite("redial", 100_000L)
        assertEquals(emptyList<String>(), policy.linksToRedial(600_000L, peerOf))

        // Heard from again, the peer may be redialled on its next silence.
        policy.notePeerHeard("peer")
        policy.noteWrite("later", 600_000L)
        policy.noteWrite("later", 610_000L)
        assertEquals(listOf("later"), policy.linksToRedial(620_000L, peerOf))
    }

    @Test
    fun `the redial bound is per peer`() {
        policy.noteWrite("a", 0L)
        policy.noteWrite("a", 10_000L)
        policy.noteWrite("b", 0L)
        policy.noteWrite("b", 10_000L)
        assertEquals(listOf("a", "b"), policy.linksToRedial(20_000L) { it }.sorted())
    }

    @Test
    fun `retainAll drops the links it rejects`() {
        policy.noteWrite("closed", 0L)
        policy.noteWrite("closed", 10_000L)
        policy.retainAll { it != "closed" }
        assertTrue(policy.silentLinks(20_000L).isEmpty())
    }
}
