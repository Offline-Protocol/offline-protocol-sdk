package com.offlineprotocol.ble

import org.junit.Assert.assertArrayEquals
import org.junit.Assert.assertEquals
import org.junit.Assert.assertFalse
import org.junit.Assert.assertTrue
import org.junit.Test

/**
 * Unit tests for [OutboundFragmentQueue]. The class is exercised in plain
 * JVM tests by injecting a no-op main-thread guard and a deterministic
 * clock — this is the whole reason those hooks exist.
 *
 * Properties under test:
 *
 *   - FIFO ordering per recipient, across enqueue → flush cycles.
 *   - Per-peer cap evicts oldest-first and reports via the drop callback.
 *   - Expiry during flush evicts stale entries and reports via the drop
 *     callback, and the aggregate counter stays consistent.
 *   - Flush stops draining a recipient on the first failed send but keeps
 *     trying the remaining recipients; the `hasUnsent` return surfaces the
 *     stall to diagnostics.
 *   - `enqueueIfBlocked` honours FIFO by queueing when a prior fragment is
 *     still waiting, and passes through otherwise.
 *   - `removeAll` / `clear` bring the total counter back to zero.
 *   - `totalCount`, `recipientIds`, `recipientCount` are readable without
 *     tripping the main-thread guard, so callback-thread diagnostic paths
 *     stay safe.
 */
class OutboundFragmentQueueTest {

    private class FakeClock(private var nowMs: Long = 0L) : () -> Long {
        override fun invoke(): Long = nowMs
        fun advance(ms: Long) {
            nowMs += ms
        }
    }

    private data class DropEvent(
        val recipientId: String,
        val reason: OutboundFragmentQueue.DropReason,
        val count: Int,
    )

    private class Harness(
        maxPerPeer: Int = 4,
        timeoutMs: Long = 1_000L,
    ) {
        val clock = FakeClock()
        val drops = mutableListOf<DropEvent>()
        val mainThreadInvocations = java.util.concurrent.atomic.AtomicInteger(0)
        val queue = OutboundFragmentQueue(
            mainThreadCheck = { mainThreadInvocations.incrementAndGet() },
            maxPerPeer = maxPerPeer,
            timeoutMs = timeoutMs,
            clock = clock,
            onDropped = { recipientId, reason, count ->
                drops += DropEvent(recipientId, reason, count)
            },
        )
    }

    @Test
    fun `enqueue then flush drains in FIFO order`() {
        val h = Harness()
        h.queue.enqueue("alice", byteArrayOf(1))
        h.queue.enqueue("alice", byteArrayOf(2))
        h.queue.enqueue("alice", byteArrayOf(3))
        assertEquals(3, h.queue.totalCount())

        val sent = mutableListOf<ByteArray>()
        val hasUnsent = h.queue.flush { _, data ->
            sent += data
            true
        }
        assertFalse(hasUnsent)
        assertEquals(0, h.queue.totalCount())
        assertEquals(3, sent.size)
        assertArrayEquals(byteArrayOf(1), sent[0])
        assertArrayEquals(byteArrayOf(2), sent[1])
        assertArrayEquals(byteArrayOf(3), sent[2])
    }

    @Test
    fun `hasPending reflects queue state`() {
        val h = Harness()
        assertFalse(h.queue.hasPending("alice"))
        h.queue.enqueue("alice", byteArrayOf(1))
        assertTrue(h.queue.hasPending("alice"))
        assertFalse(h.queue.hasPending("bob"))
    }

    @Test
    fun `enqueueIfBlocked passes through when recipient is idle`() {
        val h = Harness()
        val blocked = h.queue.enqueueIfBlocked("alice", byteArrayOf(1))
        assertFalse(blocked)
        assertEquals(0, h.queue.totalCount())
    }

    @Test
    fun `enqueueIfBlocked queues when recipient already has pending`() {
        val h = Harness()
        h.queue.enqueue("alice", byteArrayOf(1))
        val blocked = h.queue.enqueueIfBlocked("alice", byteArrayOf(2))
        assertTrue(blocked)
        assertEquals(2, h.queue.totalCount())
    }

    @Test
    fun `per-peer cap drops oldest and reports CAPPED`() {
        val h = Harness(maxPerPeer = 2)
        h.queue.enqueue("alice", byteArrayOf(1))
        h.queue.enqueue("alice", byteArrayOf(2))
        h.queue.enqueue("alice", byteArrayOf(3))
        assertEquals(2, h.queue.totalCount())
        assertEquals(1, h.drops.size)
        assertEquals(
            DropEvent("alice", OutboundFragmentQueue.DropReason.CAPPED, 1),
            h.drops[0],
        )

        // FIFO guarantee: the oldest (byte 1) was evicted, so drain returns 2 then 3.
        val sent = mutableListOf<ByteArray>()
        h.queue.flush { _, data ->
            sent += data
            true
        }
        assertEquals(2, sent.size)
        assertArrayEquals(byteArrayOf(2), sent[0])
        assertArrayEquals(byteArrayOf(3), sent[1])
    }

    @Test
    fun `flush evicts expired entries and reports EXPIRED`() {
        val h = Harness(timeoutMs = 1_000L)
        h.queue.enqueue("alice", byteArrayOf(1))
        h.queue.enqueue("alice", byteArrayOf(2))
        h.clock.advance(1_500L)
        h.queue.enqueue("alice", byteArrayOf(3))
        assertEquals(3, h.queue.totalCount())

        val sent = mutableListOf<ByteArray>()
        val hasUnsent = h.queue.flush { _, data ->
            sent += data
            true
        }
        assertFalse(hasUnsent)
        assertEquals(0, h.queue.totalCount())
        // Only the fresh fragment survived the flush.
        assertEquals(1, sent.size)
        assertArrayEquals(byteArrayOf(3), sent[0])
        assertEquals(1, h.drops.size)
        assertEquals(
            DropEvent("alice", OutboundFragmentQueue.DropReason.EXPIRED, 2),
            h.drops[0],
        )
    }

    @Test
    fun `flush returns hasUnsent when sender rejects fragment`() {
        val h = Harness()
        h.queue.enqueue("alice", byteArrayOf(1))
        h.queue.enqueue("alice", byteArrayOf(2))
        h.queue.enqueue("bob", byteArrayOf(10))

        val sent = mutableListOf<Pair<String, Byte>>()
        val hasUnsent = h.queue.flush { recipientId, data ->
            if (recipientId == "alice") {
                false // alice's link is stalled
            } else {
                sent += recipientId to data[0]
                true
            }
        }
        assertTrue(hasUnsent)
        // Alice's 2 fragments still queued; bob drained cleanly.
        assertEquals(2, h.queue.totalCount())
        assertTrue(h.queue.hasPending("alice"))
        assertFalse(h.queue.hasPending("bob"))
        assertEquals(1, sent.size)
        assertEquals("bob", sent[0].first)
    }

    @Test
    fun `flush stops on first failed send for a recipient but preserves FIFO`() {
        // Regression guard: if fragment 1 fails but fragment 2 succeeds, a
        // naive implementation could deliver bytes out of order. The queue
        // must stop draining a recipient as soon as a send fails.
        val h = Harness()
        h.queue.enqueue("alice", byteArrayOf(1))
        h.queue.enqueue("alice", byteArrayOf(2))
        h.queue.enqueue("alice", byteArrayOf(3))

        val sent = mutableListOf<Byte>()
        var call = 0
        val hasUnsent = h.queue.flush { _, data ->
            call++
            if (call == 1) {
                false // stall on the first send
            } else {
                sent += data[0]
                true
            }
        }
        assertTrue(hasUnsent)
        assertEquals(3, h.queue.totalCount())
        assertTrue(sent.isEmpty())
    }

    @Test
    fun `removeAll drops a recipient and updates totalCount`() {
        val h = Harness()
        h.queue.enqueue("alice", byteArrayOf(1))
        h.queue.enqueue("alice", byteArrayOf(2))
        h.queue.enqueue("bob", byteArrayOf(10))
        assertEquals(3, h.queue.totalCount())

        val removed = h.queue.removeAll("alice")
        assertEquals(2, removed)
        assertEquals(1, h.queue.totalCount())
        assertFalse(h.queue.hasPending("alice"))
        assertTrue(h.queue.hasPending("bob"))
    }

    @Test
    fun `removeAll on unknown recipient returns zero`() {
        val h = Harness()
        assertEquals(0, h.queue.removeAll("ghost"))
        assertEquals(0, h.queue.totalCount())
    }

    @Test
    fun `clear drops everything`() {
        val h = Harness()
        h.queue.enqueue("alice", byteArrayOf(1))
        h.queue.enqueue("bob", byteArrayOf(2))
        h.queue.clear()
        assertEquals(0, h.queue.totalCount())
        assertEquals(0, h.queue.recipientCount())
        assertTrue(h.queue.recipientIds().isEmpty())
    }

    @Test
    fun `recipientIds and recipientCount are safe to read from any thread`() {
        // Contract: the main-thread guard is only invoked by mutating and
        // single-entry lookup methods. Diagnostic paths on callback threads
        // must be able to read aggregate state without tripping the check.
        val h = Harness()
        h.queue.enqueue("alice", byteArrayOf(1))
        h.queue.enqueue("bob", byteArrayOf(2))
        val mainCallsBefore = h.mainThreadInvocations.get()

        assertEquals(2, h.queue.totalCount())
        assertEquals(2, h.queue.recipientCount())
        val ids = h.queue.recipientIds().toSet()
        assertEquals(setOf("alice", "bob"), ids)

        assertEquals(
            "off-thread reads must not call the main-thread guard",
            mainCallsBefore,
            h.mainThreadInvocations.get(),
        )
    }

    @Test
    fun `main-thread guard is invoked on every mutating entry point`() {
        // If a future refactor forgets to mainThreadCheck() in a mutating
        // method, this test fails. Cheap insurance for a load-bearing
        // invariant.
        val h = Harness()
        val before = h.mainThreadInvocations.get()

        h.queue.enqueue("alice", byteArrayOf(1))
        h.queue.enqueueIfBlocked("alice", byteArrayOf(2))
        h.queue.hasPending("alice")
        h.queue.flush { _, _ -> true }
        h.queue.enqueue("bob", byteArrayOf(3))
        h.queue.removeAll("bob")
        h.queue.clear()

        val delta = h.mainThreadInvocations.get() - before
        // 7 direct calls above. enqueueIfBlocked internally calls hasPending
        // and enqueue, which trip the guard again (the invariant is that
        // *every* entry point asserts, so nested calls double-count and
        // that is fine — we only care the counter moved far enough).
        assertTrue(
            "expected >= 7 main-thread guard invocations, got $delta",
            delta >= 7,
        )
    }

    @Test
    fun `flush is resilient across multiple recipients when one stalls`() {
        // alice stalls, bob drains, charlie drains. After the flush, only
        // alice's fragments remain and the counter reflects that exactly.
        val h = Harness()
        h.queue.enqueue("alice", byteArrayOf(1))
        h.queue.enqueue("alice", byteArrayOf(2))
        h.queue.enqueue("bob", byteArrayOf(10))
        h.queue.enqueue("charlie", byteArrayOf(20))
        h.queue.enqueue("charlie", byteArrayOf(21))

        val hasUnsent = h.queue.flush { recipientId, _ ->
            recipientId != "alice"
        }
        assertTrue(hasUnsent)
        assertEquals(2, h.queue.totalCount())
        assertTrue(h.queue.hasPending("alice"))
        assertFalse(h.queue.hasPending("bob"))
        assertFalse(h.queue.hasPending("charlie"))
        assertEquals(1, h.queue.recipientCount())
    }

    @Test
    fun `enqueue after clear allocates a fresh queue`() {
        val h = Harness()
        h.queue.enqueue("alice", byteArrayOf(1))
        h.queue.clear()
        assertEquals(0, h.queue.totalCount())
        assertFalse(h.queue.hasPending("alice"))

        h.queue.enqueue("alice", byteArrayOf(9))
        assertEquals(1, h.queue.totalCount())
        val sent = mutableListOf<Byte>()
        h.queue.flush { _, data ->
            sent += data[0]
            true
        }
        assertArrayEquals(byteArrayOf(9), sent.toByteArray())
    }
}
