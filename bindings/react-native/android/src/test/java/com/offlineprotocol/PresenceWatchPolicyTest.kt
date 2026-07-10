package com.offlineprotocol

import org.junit.Assert.assertEquals
import org.junit.Assert.assertTrue
import org.junit.Test

class PresenceWatchPolicyTest {

    @Test
    fun mergesCoreWatchlistWithLocalSignals() {
        val policy = PresenceWatchPolicy()
        policy.watch("delivery-error-peer", nowMs = 1_000)

        val queried = policy.peersToQuery(listOf("welcome-pending-peer"), nowMs = 1_000)

        assertEquals(
            setOf("delivery-error-peer", "welcome-pending-peer"),
            queried.toSet()
        )
    }

    @Test
    fun capsQueriesPerTickAndRotatesAcrossTicks() {
        val policy = PresenceWatchPolicy(maxQueriesPerTick = 2)
        for (i in 1..5) {
            policy.watch("peer$i", nowMs = 0)
        }

        val tick1 = policy.peersToQuery(emptyList(), nowMs = 1)
        val tick2 = policy.peersToQuery(emptyList(), nowMs = 2)
        val tick3 = policy.peersToQuery(emptyList(), nowMs = 3)

        assertEquals(2, tick1.size)
        assertEquals(2, tick2.size)
        // All five peers covered within ceil(5/2) = 3 ticks.
        assertEquals(
            setOf("peer1", "peer2", "peer3", "peer4", "peer5"),
            (tick1 + tick2 + tick3).toSet()
        )
    }

    @Test
    fun unwatchRemovesPeer() {
        val policy = PresenceWatchPolicy()
        policy.watch("bob", nowMs = 0)
        policy.unwatch("bob")

        assertTrue(policy.peersToQuery(emptyList(), nowMs = 1).isEmpty())
        assertTrue(policy.watchedPeers().isEmpty())
    }

    @Test
    fun idlePeersAreEvictedButCoreListedPeersStayFresh() {
        val policy = PresenceWatchPolicy(idleTtlMs = 1_000)
        policy.watch("stale", nowMs = 0)
        policy.watch("pending", nowMs = 0)

        // "pending" keeps being reported by the core watchlist, refreshing
        // its idle clock; "stale" was a one-off DeliveryError signal.
        val queried = policy.peersToQuery(listOf("pending"), nowMs = 2_000)

        assertEquals(listOf("pending"), queried)
        assertEquals(setOf("pending"), policy.watchedPeers())
    }

    @Test
    fun ignoresEmptyPeerIdsAndClearResets() {
        val policy = PresenceWatchPolicy()
        policy.watch("", nowMs = 0)
        assertTrue(policy.peersToQuery(listOf(""), nowMs = 1).isEmpty())

        policy.watch("bob", nowMs = 0)
        policy.clear()
        assertTrue(policy.watchedPeers().isEmpty())
        assertTrue(policy.peersToQuery(emptyList(), nowMs = 1).isEmpty())
    }
}
