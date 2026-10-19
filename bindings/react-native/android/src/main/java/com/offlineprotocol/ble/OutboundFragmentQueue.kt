package com.offlineprotocol.ble

import java.util.concurrent.ConcurrentHashMap
import java.util.concurrent.atomic.AtomicInteger

/**
 * Outcome of one [OutboundFragmentQueue.flush] pass.
 *
 * [sent] exists because progress is **not** derivable from a queue-size delta:
 * [OutboundFragmentQueue.flush] also evicts entries past their TTL, so a queue
 * that shrank may have delivered nothing at all. The drain's backpressure
 * ladder resets on progress, and a permanently stalled peer shedding expired
 * fragments every TTL window would otherwise re-earn the fast retry ladder
 * forever — which is precisely the peer the ladder's ceiling exists to stop
 * (OFF-2123). Counting accepted sends keeps the two apart.
 */
internal data class FlushResult(
    /** Fragments the sender accepted this pass. Expiry evictions do not count. */
    val sent: Int,
    /**
     * True if at least one recipient still had unsent fragments when the flush
     * finished. Callers use this as the "stalled writer" signal.
     */
    val hasUnsent: Boolean,
)

/**
 * FIFO buffer of outbound BLE fragments that cannot be sent immediately —
 * either because the recipient has no open connection, or because the
 * previous write hit flow control.
 *
 * Thread-safety contract: every mutating operation must run on the BLE
 * thread. The per-recipient [ArrayDeque] values are not thread-safe, and
 * even a `size` read computes `tail - head` over a backing array that the
 * BLE-thread writer may be resizing. The outer [ConcurrentHashMap] exists
 * purely so off-thread callers can cheaply read [totalCount] and
 * [recipientIds] without blocking.
 *
 * The BLE-thread contract is enforced at runtime via [bleThreadCheck] —
 * the invariant used to live in a comment block; it now fails fast on any
 * drift. Tests inject a no-op guard so the class can be exercised in plain
 * JVM unit tests.
 *
 * ### Overflow policy
 *
 * When [enqueue] would push the per-recipient queue past [maxPerPeer], the
 * entire queue for that recipient is discarded before the new fragment is
 * appended. Dropping just the oldest fragment (the previous policy) was
 * unsafe: fragments are slices of a single application message and
 * evicting slice 0 of a five-slice message leaves four orphan slices that
 * would reassemble into garbage at the receiver. Dropping the whole
 * queue avoids orphaning the slices behind a dropped one, but it is still
 * a loss, and it tears the message whose first slices already went out.
 *
 * So the cap is a memory backstop and must stay unreachable in normal
 * operation. The drain stops pulling from the Rust core while any recipient
 * is past the [isBackedUp] mark ([anyBackedUp]), checked **before** each pop,
 * because the pop is destructive and nothing can hand a fragment back. Every
 * fragment in this queue arrived through one such pop, so a recipient never
 * holds more than the mark, and the mark sits below the cap. The cap also
 * sits above the largest message the core will fragment (512 fragments).
 * With the old cap of 100, and a poll path that popped without the check,
 * the 2026-10-19 volume test never delivered a 16 KB or 30 KB message from
 * Android.
 *
 * ### The mark is small on purpose
 *
 * [DEFAULT_BACKPRESSURE_MARK] is a few seconds of link time, not a fraction of
 * the cap, because the Rust core can see only what it still holds. A message
 * counts as queued, and its acknowledgement timer is extended instead of
 * retried, until its first fragment is pulled. With the mark at 3/4 of the cap
 * the drain pulled 576 fragments ahead of an indication-paced link, the core
 * saw nothing queued, and every message behind the first few seconds was
 * resent: 19,404 resends from Android in the third volume run, each one more
 * load on the link that was already the bottleneck. A fragment left in Rust
 * costs nothing; one pulled early only waits here, expires after [timeoutMs],
 * and tears its message.
 *
 * Rust guards pin the cap and the mark equal to the iOS queue's.
 */
internal class OutboundFragmentQueue(
    private val bleThreadCheck: () -> Unit = DEFAULT_BLE_THREAD_CHECK,
    private val maxPerPeer: Int = DEFAULT_MAX_PER_PEER,
    private val timeoutMs: Long = DEFAULT_TIMEOUT_MS,
    /** Below [maxPerPeer]; see the class note. Smaller caps keep the old 3/4. */
    private val backpressureMark: Int = minOf(DEFAULT_BACKPRESSURE_MARK, maxPerPeer * 3 / 4),
    private val clock: () -> Long = System::currentTimeMillis,
    private val onDropped: (recipientId: String, reason: DropReason, count: Int) -> Unit =
        { _, _, _ -> },
) {

    enum class DropReason { CAPPED, EXPIRED }

    private data class Entry(val data: ByteArray, val timestamp: Long)

    private val queues = ConcurrentHashMap<String, ArrayDeque<Entry>>()
    private val total = AtomicInteger(0)

    /** Aggregate fragment count across all recipients. Safe from any thread. */
    fun totalCount(): Int = total.get()

    /** Snapshot of recipients with outstanding fragments. Safe from any thread. */
    fun recipientIds(): List<String> = queues.keys.toList()

    /** Number of recipients with outstanding fragments. Safe from any thread. */
    fun recipientCount(): Int = queues.size

    /** True if [recipientId] has at least one fragment already waiting. */
    fun hasPending(recipientId: String): Boolean {
        bleThreadCheck()
        return queues[recipientId]?.isNotEmpty() == true
    }

    /**
     * True once [recipientId]'s queue has filled to [backpressureMark]. The
     * drain loop uses this as a backpressure
     * signal to STOP pulling more fragments out of the Rust core — which is
     * a destructive pop — into this bounded queue. Without it, when the BLE
     * write gate paces sends slower than the loop can pull, the loop spins
     * the whole Rust backlog into the queue, overflows [maxPerPeer], and
     * [enqueue] discards the entire per-peer queue mid-message. Holding the
     * backlog in Rust (the proper unbounded, ordered buffer) instead keeps
     * delivery lossless; the scheduled re-drain resumes pulling once
     * [flush] has drained the queue back below the mark.
     */
    fun isBackedUp(recipientId: String): Boolean {
        bleThreadCheck()
        val size = queues[recipientId]?.size ?: 0
        return size >= backpressureMark
    }

    /**
     * True if any recipient is past the [isBackedUp] mark. The drain checks
     * it before every pop from the Rust core: the pop is one FIFO across all
     * peers and cannot be undone, so the only lossless moment to stop is
     * before it. Safe only on the BLE thread, like every other read here.
     */
    fun anyBackedUp(): Boolean {
        bleThreadCheck()
        return queues.values.any { it.size >= backpressureMark }
    }

    /**
     * Append a fragment to [recipientId]'s queue. If doing so would exceed
     * [maxPerPeer] the entire per-recipient queue is discarded first (see
     * the class-level overflow policy note). [onDropped] is invoked once
     * with the total number of fragments evicted, then the new fragment is
     * appended to a fresh empty queue.
     */
    fun enqueue(recipientId: String, data: ByteArray) {
        bleThreadCheck()
        val queue = queues.getOrPut(recipientId) { ArrayDeque() }
        if (queue.size >= maxPerPeer) {
            val dropped = queue.size
            queue.clear()
            total.addAndGet(-dropped)
            onDropped(recipientId, DropReason.CAPPED, dropped)
        }
        queue.addLast(Entry(data, clock()))
        total.incrementAndGet()
    }

    /**
     * Maintain FIFO ordering for per-recipient streams: if [recipientId]
     * already has queued fragments waiting for an earlier send to clear,
     * append [data] and return true so the caller skips a direct send.
     * Otherwise return false so the caller can try the direct path.
     */
    fun enqueueIfBlocked(recipientId: String, data: ByteArray): Boolean {
        bleThreadCheck()
        if (!hasPending(recipientId)) return false
        enqueue(recipientId, data)
        return true
    }

    /**
     * Drop every fragment queued for [recipientId] — used when the peer has
     * been evicted and the bytes can never be delivered. Returns the count
     * dropped so callers can surface a diagnostic.
     */
    fun removeAll(recipientId: String): Int {
        bleThreadCheck()
        val removed = queues.remove(recipientId) ?: return 0
        val count = removed.size
        if (count > 0) total.addAndGet(-count)
        return count
    }

    /** Drop every queue. Used by transport stop. */
    fun clear() {
        bleThreadCheck()
        queues.clear()
        total.set(0)
    }

    /**
     * Walk every recipient's queue, evicting entries older than [timeoutMs]
     * and then attempting to send the remainder in FIFO order via [send].
     * If [send] returns false the fragment is left in place (so the next
     * flush retries it) and iteration for that recipient stops — but other
     * recipients continue to drain.
     */
    fun flush(send: (recipientId: String, data: ByteArray) -> Boolean): FlushResult {
        bleThreadCheck()
        var hasUnsent = false
        var sent = 0
        val now = clock()

        for (recipientId in queues.keys.toList()) {
            val queue = queues[recipientId] ?: continue

            // Expiry is per-fragment, not per-message. These entries are opaque
            // fragment bytes from the Rust fragmenter with no message grouping at
            // this layer, so on a stall longer than timeoutMs the early fragments
            // of a multi-fragment message can be dropped while later ones survive,
            // tearing it. Bounded, not silent: the receiver's idle reassembly times
            // out the partial and the sender's higher layer (Welcome retransmit /
            // ack-driven retry) re-sends the whole message. True whole-message
            // expiry would need message ids threaded down from Rust — tracked
            // separately.
            var expired = 0
            val expiryIter = queue.iterator()
            while (expiryIter.hasNext()) {
                val fragment = expiryIter.next()
                if (now - fragment.timestamp >= timeoutMs) {
                    expiryIter.remove()
                    expired++
                }
            }
            if (expired > 0) {
                total.addAndGet(-expired)
                onDropped(recipientId, DropReason.EXPIRED, expired)
            }

            if (queue.isEmpty()) {
                queues.remove(recipientId)
                continue
            }

            val sendIter = queue.iterator()
            while (sendIter.hasNext()) {
                val fragment = sendIter.next()
                if (send(recipientId, fragment.data)) {
                    sendIter.remove()
                    total.decrementAndGet()
                    sent++
                } else {
                    hasUnsent = true
                    break
                }
            }

            if (queue.isEmpty()) {
                queues.remove(recipientId)
            }
        }
        return FlushResult(sent = sent, hasUnsent = hasUnsent)
    }

    companion object {
        /** Above 4/3 of the core's 512-fragment message ceiling; see the class note. */
        const val DEFAULT_MAX_PER_PEER: Int = 768
        /** About 2.5 s of an indication-paced link; see the class note. */
        const val DEFAULT_BACKPRESSURE_MARK: Int = 32
        const val DEFAULT_TIMEOUT_MS: Long = 30_000L

        val DEFAULT_BLE_THREAD_CHECK: () -> Unit = {
            BleTransportFacade.assertOnBleLooper("OutboundFragmentQueue")
        }
    }
}
