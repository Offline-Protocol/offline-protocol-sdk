package com.offlineprotocol.ble

import android.os.Looper
import java.util.concurrent.ConcurrentHashMap
import java.util.concurrent.atomic.AtomicInteger

/**
 * FIFO buffer of outbound BLE fragments that cannot be sent immediately —
 * either because the recipient has no open connection, or because the
 * previous write hit flow control.
 *
 * Thread-safety contract: every mutating operation must run on the main
 * thread. The per-recipient [ArrayDeque] values are not thread-safe, and
 * even a `size` read computes `tail - head` over a backing array that the
 * main-thread writer may be resizing. The outer [ConcurrentHashMap] exists
 * purely so off-thread callers can cheaply read [totalCount] and
 * [recipientIds] without blocking.
 *
 * The main-thread contract is enforced at runtime via [mainThreadCheck] —
 * the invariant used to live in a comment block; it now fails fast on any
 * drift. Tests inject a no-op guard so the class can be exercised in plain
 * JVM unit tests.
 */
internal class OutboundFragmentQueue(
    private val mainThreadCheck: () -> Unit = DEFAULT_MAIN_THREAD_CHECK,
    private val maxPerPeer: Int = DEFAULT_MAX_PER_PEER,
    private val timeoutMs: Long = DEFAULT_TIMEOUT_MS,
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
        mainThreadCheck()
        return queues[recipientId]?.isNotEmpty() == true
    }

    /**
     * Append a fragment to [recipientId]'s queue. If doing so would exceed
     * [maxPerPeer], the oldest entry for that recipient is dropped first —
     * the guarantee is that under backpressure we keep the freshest bytes,
     * not the stalest.
     */
    fun enqueue(recipientId: String, data: ByteArray) {
        mainThreadCheck()
        val queue = queues.getOrPut(recipientId) { ArrayDeque() }
        queue.addLast(Entry(data, clock()))
        total.incrementAndGet()
        if (queue.size > maxPerPeer) {
            queue.removeFirst()
            total.decrementAndGet()
            onDropped(recipientId, DropReason.CAPPED, 1)
        }
    }

    /**
     * Maintain FIFO ordering for per-recipient streams: if [recipientId]
     * already has queued fragments waiting for an earlier send to clear,
     * append [data] and return true so the caller skips a direct send.
     * Otherwise return false so the caller can try the direct path.
     */
    fun enqueueIfBlocked(recipientId: String, data: ByteArray): Boolean {
        mainThreadCheck()
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
        mainThreadCheck()
        val removed = queues.remove(recipientId) ?: return 0
        val count = removed.size
        if (count > 0) total.addAndGet(-count)
        return count
    }

    /** Drop every queue. Used by transport stop. */
    fun clear() {
        mainThreadCheck()
        queues.clear()
        total.set(0)
    }

    /**
     * Walk every recipient's queue, evicting entries older than [timeoutMs]
     * and then attempting to send the remainder in FIFO order via [send].
     * If [send] returns false the fragment is left in place (so the next
     * flush retries it) and iteration for that recipient stops — but other
     * recipients continue to drain.
     *
     * @return true if at least one recipient still had unsent fragments
     *   when flush finished. Callers use this as a "stalled writer" signal
     *   for diagnostics.
     */
    fun flush(send: (recipientId: String, data: ByteArray) -> Boolean): Boolean {
        mainThreadCheck()
        var hasUnsent = false
        val now = clock()

        for (recipientId in queues.keys.toList()) {
            val queue = queues[recipientId] ?: continue

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
                } else {
                    hasUnsent = true
                    break
                }
            }

            if (queue.isEmpty()) {
                queues.remove(recipientId)
            }
        }
        return hasUnsent
    }

    companion object {
        const val DEFAULT_MAX_PER_PEER: Int = 100
        const val DEFAULT_TIMEOUT_MS: Long = 30_000L

        val DEFAULT_MAIN_THREAD_CHECK: () -> Unit = {
            check(Looper.myLooper() == Looper.getMainLooper()) {
                "OutboundFragmentQueue must only be touched on the main thread"
            }
        }
    }
}
