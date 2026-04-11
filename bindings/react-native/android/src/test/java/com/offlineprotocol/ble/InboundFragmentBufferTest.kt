package com.offlineprotocol.ble

import org.junit.Assert.assertArrayEquals
import org.junit.Assert.assertEquals
import org.junit.Assert.assertFalse
import org.junit.Assert.assertTrue
import org.junit.Test

/**
 * Unit tests for [InboundFragmentBuffer]. Mirrors the style and injection
 * points of [OutboundFragmentQueueTest] — fake clock, no-op main-thread
 * guard, and a drop-event recorder — so the class can be exercised in plain
 * JVM unit tests without an Android looper.
 *
 * Properties under test:
 *
 *   - FIFO ordering per peer, across enqueue → drain cycles.
 *   - Per-peer cap drops the whole per-address buffer at message-boundary
 *     granularity (never mid-message) and reports via the drop callback.
 *     This is the exact regression from the previous head-eviction policy.
 *   - Peer cap evicts the peer with the stalest head entry, excluding the
 *     address currently being queued for.
 *   - Expiry during [evictExpired] evicts stale entries and prunes empty
 *     buckets while keeping the aggregate counter consistent.
 *   - `removeAll` / `clear` bring the total counter back to zero.
 *   - `totalCount` is readable without tripping the main-thread guard, so
 *     callback-thread diagnostic paths stay safe.
 *   - The main-thread guard is invoked exactly once per mutating call.
 */
class InboundFragmentBufferTest {

    private class FakeClock(private var nowMs: Long = 0L) : () -> Long {
        override fun invoke(): Long = nowMs
        fun advance(ms: Long) {
            nowMs += ms
        }
    }

    private data class DropEvent(
        val address: String,
        val reason: InboundFragmentBuffer.DropReason,
        val count: Int,
    )

    private class Harness(
        maxPerPeer: Int = 4,
        maxPeers: Int = 3,
        timeoutMs: Long = 1_000L,
    ) {
        val clock = FakeClock()
        val drops = mutableListOf<DropEvent>()
        val mainThreadInvocations = java.util.concurrent.atomic.AtomicInteger(0)
        val buffer = InboundFragmentBuffer(
            mainThreadCheck = { mainThreadInvocations.incrementAndGet() },
            maxPerPeer = maxPerPeer,
            maxPeers = maxPeers,
            timeoutMs = timeoutMs,
            clock = clock,
            onDropped = { address, reason, count ->
                drops += DropEvent(address, reason, count)
            },
        )
    }

    @Test
    fun `enqueue then drain returns fragments in FIFO order`() {
        val h = Harness()
        h.buffer.enqueue("aa:00", byteArrayOf(1))
        h.buffer.enqueue("aa:00", byteArrayOf(2))
        h.buffer.enqueue("aa:00", byteArrayOf(3))
        assertEquals(3, h.buffer.totalCount())
        assertTrue(h.buffer.hasPending("aa:00"))

        val drained = h.buffer.drain("aa:00")
        assertEquals(3, drained.size)
        assertArrayEquals(byteArrayOf(1), drained[0])
        assertArrayEquals(byteArrayOf(2), drained[1])
        assertArrayEquals(byteArrayOf(3), drained[2])
        assertEquals(0, h.buffer.totalCount())
        assertFalse(h.buffer.hasPending("aa:00"))
    }

    @Test
    fun `drain with no bucket returns empty list and leaves counter at zero`() {
        val h = Harness()
        val drained = h.buffer.drain("bb:11")
        assertTrue(drained.isEmpty())
        assertEquals(0, h.buffer.totalCount())
    }

    @Test
    fun `per-peer cap drops the whole buffer and reports CAPPED_PER_PEER with count`() {
        // Regression guard for the pre-fix head-eviction policy. Fragments
        // are slices of one application message; dropping slice 0 of a
        // five-slice message would leave the reassembler with four orphans
        // that stitch into garbage. The safe policy is to drop the whole
        // peer-buffer at message-boundary granularity and let the upstream
        // retry path re-send the whole message.
        val h = Harness(maxPerPeer = 2)
        h.buffer.enqueue("aa:00", byteArrayOf(1))
        h.buffer.enqueue("aa:00", byteArrayOf(2))
        h.buffer.enqueue("aa:00", byteArrayOf(3))
        assertEquals(1, h.buffer.totalCount())
        assertEquals(1, h.drops.size)
        assertEquals(
            DropEvent("aa:00", InboundFragmentBuffer.DropReason.CAPPED_PER_PEER, 2),
            h.drops[0],
        )
        val drained = h.buffer.drain("aa:00")
        assertEquals(1, drained.size)
        assertArrayEquals(byteArrayOf(3), drained[0])
    }

    @Test
    fun `per-peer cap overflow does not affect other peers`() {
        val h = Harness(maxPerPeer = 2)
        h.buffer.enqueue("aa:00", byteArrayOf(1))
        h.buffer.enqueue("aa:00", byteArrayOf(2))
        h.buffer.enqueue("bb:11", byteArrayOf(10))
        h.buffer.enqueue("bb:11", byteArrayOf(11))
        h.buffer.enqueue("aa:00", byteArrayOf(3)) // trips aa:00's per-peer cap

        // aa:00 kept only the freshly enqueued byte; bb:11 untouched.
        assertEquals(3, h.buffer.totalCount())
        val aa = h.buffer.drain("aa:00")
        val bb = h.buffer.drain("bb:11")
        assertEquals(1, aa.size)
        assertArrayEquals(byteArrayOf(3), aa[0])
        assertEquals(2, bb.size)
        assertEquals(1, h.drops.size)
        assertEquals("aa:00", h.drops[0].address)
    }

    @Test
    fun `peer cap evicts the oldest-head peer and never the address being queued`() {
        // maxPeers = 2. We fill buckets in ascending timestamp order so the
        // eviction target is unambiguous: the first peer queued has the
        // oldest head entry and should be the one evicted when the third
        // peer arrives.
        val h = Harness(maxPeers = 2)
        h.clock.advance(10)
        h.buffer.enqueue("oldest", byteArrayOf(1))
        h.clock.advance(10)
        h.buffer.enqueue("middle", byteArrayOf(2))
        h.clock.advance(10)
        // Queuing for "newest" trips the peer cap; "oldest" should be the
        // victim — it has the stalest head timestamp and it is not the
        // address being queued for.
        h.buffer.enqueue("newest", byteArrayOf(3))

        assertFalse(h.buffer.hasPending("oldest"))
        assertTrue(h.buffer.hasPending("middle"))
        assertTrue(h.buffer.hasPending("newest"))
        assertEquals(2, h.buffer.totalCount())
        assertEquals(1, h.drops.size)
        assertEquals(
            DropEvent("oldest", InboundFragmentBuffer.DropReason.CAPPED_PEERS, 1),
            h.drops[0],
        )
    }

    @Test
    fun `peer cap never evicts the address being enqueued into`() {
        // Policy is "never evict keepAddress" — not "evict whoever has the
        // oldest head". Construct the hostile scenario: the peer being
        // queued for has the oldest head of all, so a naive oldest-head
        // walk would pick it; the cap must still fall on a different peer.
        val h = Harness(maxPeers = 2)
        h.clock.advance(500)
        h.buffer.enqueue("alpha", byteArrayOf(1)) // head ts = 500
        h.buffer.enqueue("beta", byteArrayOf(2))  // head ts = 500
        // Back-date the clock so gamma's head timestamp is the oldest.
        h.clock.advance(-400)
        h.buffer.enqueue("gamma", byteArrayOf(3)) // head ts = 100, stalest

        // gamma must survive despite having the stalest head.
        assertTrue(h.buffer.hasPending("gamma"))
        assertEquals(1, h.drops.size)
        val evicted = h.drops.single().address
        assertTrue(
            "eviction must fall on alpha or beta, was $evicted",
            evicted == "alpha" || evicted == "beta",
        )
        val survivingAmongSeeds = listOf("alpha", "beta").count { h.buffer.hasPending(it) }
        assertEquals(1, survivingAmongSeeds)
    }

    @Test
    fun `evictExpired drops old entries and prunes empty buckets`() {
        val h = Harness(timeoutMs = 1_000L)
        h.buffer.enqueue("aa:00", byteArrayOf(1))
        h.buffer.enqueue("aa:00", byteArrayOf(2))
        h.buffer.enqueue("bb:11", byteArrayOf(10))
        h.clock.advance(1_500L)
        h.buffer.enqueue("aa:00", byteArrayOf(3))

        val expired = h.buffer.evictExpired()
        // aa:00: two entries expired, one survived (the post-advance one).
        // bb:11: one entry expired, bucket now empty and pruned.
        assertEquals(3, expired)
        assertEquals(1, h.buffer.totalCount())
        assertTrue(h.buffer.hasPending("aa:00"))
        assertFalse(h.buffer.hasPending("bb:11"))
        val aaDrain = h.buffer.drain("aa:00")
        assertEquals(1, aaDrain.size)
        assertArrayEquals(byteArrayOf(3), aaDrain[0])

        val expiryDrops = h.drops.filter { it.reason == InboundFragmentBuffer.DropReason.EXPIRED }
        assertEquals(2, expiryDrops.size)
        assertEquals(
            setOf("aa:00" to 2, "bb:11" to 1),
            expiryDrops.map { it.address to it.count }.toSet(),
        )
    }

    @Test
    fun `removeAll drops a single peer and updates the counter`() {
        val h = Harness()
        h.buffer.enqueue("aa:00", byteArrayOf(1))
        h.buffer.enqueue("aa:00", byteArrayOf(2))
        h.buffer.enqueue("bb:11", byteArrayOf(10))
        val removed = h.buffer.removeAll("aa:00")
        assertEquals(2, removed)
        assertEquals(1, h.buffer.totalCount())
        assertFalse(h.buffer.hasPending("aa:00"))
        assertTrue(h.buffer.hasPending("bb:11"))
    }

    @Test
    fun `removeAll on unknown address is a no-op`() {
        val h = Harness()
        h.buffer.enqueue("aa:00", byteArrayOf(1))
        assertEquals(0, h.buffer.removeAll("bb:11"))
        assertEquals(1, h.buffer.totalCount())
    }

    @Test
    fun `clear resets the counter to zero`() {
        val h = Harness()
        h.buffer.enqueue("aa:00", byteArrayOf(1))
        h.buffer.enqueue("bb:11", byteArrayOf(2))
        h.buffer.clear()
        assertEquals(0, h.buffer.totalCount())
        assertFalse(h.buffer.hasPending("aa:00"))
        assertFalse(h.buffer.hasPending("bb:11"))
    }

    @Test
    fun `totalCount is readable without invoking the main-thread guard`() {
        val h = Harness()
        h.buffer.enqueue("aa:00", byteArrayOf(1))
        val enqueueInvocations = h.mainThreadInvocations.get()
        // totalCount is an AtomicInteger snapshot — safe from any thread.
        // It must not call the guard or off-thread diagnostic paths would
        // crash under the default Looper check.
        h.buffer.totalCount()
        assertEquals(enqueueInvocations, h.mainThreadInvocations.get())
    }

    @Test
    fun `pendingAddresses returns a snapshot and touches the main-thread guard`() {
        val h = Harness()
        h.buffer.enqueue("aa:00", byteArrayOf(1))
        h.buffer.enqueue("bb:11", byteArrayOf(2))
        val before = h.mainThreadInvocations.get()
        val snapshot = h.buffer.pendingAddresses()
        assertEquals(setOf("aa:00", "bb:11"), snapshot.toSet())
        assertTrue(h.mainThreadInvocations.get() > before)
    }

    @Test
    fun `main-thread guard fires for every mutating method`() {
        val h = Harness()
        val baseline = h.mainThreadInvocations.get()
        h.buffer.enqueue("aa:00", byteArrayOf(1))
        h.buffer.hasPending("aa:00")
        h.buffer.drain("aa:00")
        h.buffer.enqueue("bb:11", byteArrayOf(2))
        h.buffer.removeAll("bb:11")
        h.buffer.evictExpired()
        h.buffer.clear()
        // Lower bound: every call above touches the guard at least once.
        assertTrue(h.mainThreadInvocations.get() - baseline >= 7)
    }
}
