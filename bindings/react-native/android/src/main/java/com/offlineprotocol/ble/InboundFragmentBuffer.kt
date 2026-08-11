package com.offlineprotocol.ble

import java.util.concurrent.atomic.AtomicInteger

/**
 * FIFO buffer for inbound BLE fragments that arrive before the sender's
 * stable device ID is known. Fragments are keyed by the connection's BLE
 * address (stable for the lifetime of a single LL connection, and therefore
 * RPA-safe within that window). Once the reverse GATT read resolves the
 * device ID, the transport layer calls [drain] to hand the buffered bytes to
 * the protocol in order.
 *
 * Thread-safety contract: every mutating operation must run on the main
 * thread. The per-address [ArrayDeque] values are not thread-safe, and the
 * invariant is enforced at runtime via [bleThreadCheck] — tests inject a
 * no-op guard so the class can be exercised in plain JVM unit tests. The
 * `totalCount` snapshot is an [AtomicInteger] so off-thread diagnostic paths
 * (e.g., `refreshSelfMetrics` running on the BLE thread but reading during
 * a binder-thread callback's post-hop) remain safe.
 *
 * ### Overflow policy
 *
 * When [enqueue] would push a per-peer buffer past [maxPerPeer], the entire
 * buffer for that peer is discarded before the new fragment is appended.
 * Dropping the oldest fragment (the previous inbound-path policy) is unsafe:
 * fragments are slices of a single application message, and evicting slice
 * 0 of a five-slice message leaves four orphan slices that the reassembler
 * would stitch into garbage. Whole-queue drop gives clean backpressure at
 * message-boundary granularity — we lose messages that hadn't finished
 * arriving, but we never hand the protocol a torn message.
 *
 * ### Peer cap
 *
 * A separate global cap ([maxPeers]) bounds the number of peers with
 * outstanding buffers. When a newly queued fragment would exceed this cap,
 * the peer whose oldest buffered fragment is the stalest is evicted — but
 * never the peer currently being queued for, so a burst under the peer cap
 * cannot evict its own buffer mid-enqueue.
 */
internal class InboundFragmentBuffer(
    private val bleThreadCheck: () -> Unit = DEFAULT_BLE_THREAD_CHECK,
    private val maxPerPeer: Int = DEFAULT_MAX_PER_PEER,
    private val maxPeers: Int = DEFAULT_MAX_PEERS,
    private val timeoutMs: Long = DEFAULT_TIMEOUT_MS,
    private val clock: () -> Long = System::currentTimeMillis,
    private val onDropped: (address: String, reason: DropReason, count: Int) -> Unit =
        { _, _, _ -> },
) {

    enum class DropReason { CAPPED_PER_PEER, CAPPED_PEERS, EXPIRED }

    private data class Entry(val data: ByteArray, val timestamp: Long)

    private val buffers = HashMap<String, ArrayDeque<Entry>>()
    private val total = AtomicInteger(0)

    /** Aggregate fragment count across all peers. Safe from any thread. */
    fun totalCount(): Int = total.get()

    /** Number of peers with outstanding buffers. BLE-thread only. */
    fun peerCount(): Int {
        bleThreadCheck()
        return buffers.size
    }

    /**
     * Snapshot of peer addresses with outstanding buffers. BLE-thread only.
     * Used by the connection monitor to re-kick device-id resolution for
     * peers whose reverse read stalled.
     */
    fun pendingAddresses(): List<String> {
        bleThreadCheck()
        return buffers.keys.toList()
    }

    /** True if [address] has at least one buffered fragment. BLE-thread only. */
    fun hasPending(address: String): Boolean {
        bleThreadCheck()
        return buffers[address]?.isNotEmpty() == true
    }

    /**
     * Append a fragment for [address]. If the per-peer cap is hit, the
     * entire per-peer buffer is cleared first (see class-level rationale),
     * [onDropped] is invoked with the evicted count, and the new fragment
     * is appended to a fresh empty buffer. After appending, the peer cap
     * is enforced: if this new entry pushes the peer count over [maxPeers],
     * the oldest-head peer (excluding [address]) is evicted.
     */
    fun enqueue(address: String, data: ByteArray) {
        bleThreadCheck()
        val queue = buffers.getOrPut(address) { ArrayDeque() }
        if (queue.size >= maxPerPeer) {
            val dropped = queue.size
            queue.clear()
            total.addAndGet(-dropped)
            onDropped(address, DropReason.CAPPED_PER_PEER, dropped)
        }
        queue.addLast(Entry(data, clock()))
        total.incrementAndGet()
        enforcePeerCap(keepAddress = address)
    }

    /**
     * Remove and return all fragments queued for [address], in FIFO order.
     * Empty bucket ⇒ empty list. Used by the transport layer once a device
     * ID has been resolved and the buffered bytes can be forwarded to the
     * protocol.
     */
    fun drain(address: String): List<ByteArray> {
        bleThreadCheck()
        val removed = buffers.remove(address) ?: return emptyList()
        val count = removed.size
        if (count > 0) total.addAndGet(-count)
        return removed.map { it.data }
    }

    /**
     * Drop every fragment buffered for [address] without forwarding — used
     * when the peer has been evicted and the bytes can never be delivered.
     * Returns the count dropped so callers can surface a diagnostic.
     */
    fun removeAll(address: String): Int {
        bleThreadCheck()
        val removed = buffers.remove(address) ?: return 0
        val count = removed.size
        if (count > 0) total.addAndGet(-count)
        return count
    }

    /** Drop every buffer. Used by transport stop. */
    fun clear() {
        bleThreadCheck()
        buffers.clear()
        total.set(0)
    }

    /**
     * Walk every peer's buffer and evict whole buffers that have gone idle
     * for [timeoutMs] (no fragment received within the window). Returns the
     * total number of fragments evicted for diagnostics. Called by the
     * transport's routing cleanup ticker so stale inbound bytes do not pin
     * memory forever when a peer that was mid-handshake goes silent.
     *
     * Eviction is at WHOLE-BUFFER granularity keyed on the NEWEST entry —
     * not per-fragment. The previous per-fragment sweep tore a still-arriving
     * multi-fragment message: while device-id resolution was pending (a
     * reverse connect throttled to once / [timeoutMs]), the sender keeps
     * streaming fragments, but the sweep dropped fragment 0 the moment it
     * aged past the window while fragments 1..n were still in flight, handing
     * the reassembler a permanent hole and dropping the message. Keying on the
     * newest fragment means a buffer survives as long as bytes are still
     * arriving, and is dropped wholesale only once the peer truly goes silent —
     * matching the whole-message invariant [enqueue]'s overflow policy already
     * documents.
     */
    fun evictExpired(): Int {
        bleThreadCheck()
        val now = clock()
        var totalEvicted = 0
        val iter = buffers.entries.iterator()
        while (iter.hasNext()) {
            val entry = iter.next()
            val queue = entry.value
            // Newest entry = last appended (timestamps are append-monotonic).
            val newestTs = queue.lastOrNull()?.timestamp
            if (newestTs == null) {
                iter.remove()
                continue
            }
            if (now - newestTs >= timeoutMs) {
                val dropped = queue.size
                total.addAndGet(-dropped)
                onDropped(entry.key, DropReason.EXPIRED, dropped)
                totalEvicted += dropped
                iter.remove()
            }
        }
        return totalEvicted
    }

    private fun enforcePeerCap(keepAddress: String) {
        if (buffers.size <= maxPeers) return
        // Evict the peer whose head entry is the oldest (i.e., the peer
        // most likely to have been abandoned mid-handshake), excluding the
        // peer we just queued for so a burst under the cap cannot evict
        // its own freshly-queued bytes.
        var victimAddress: String? = null
        var victimTimestamp = Long.MAX_VALUE
        for ((addr, queue) in buffers) {
            if (addr == keepAddress) continue
            val headTs = queue.firstOrNull()?.timestamp ?: 0L
            if (headTs < victimTimestamp) {
                victimTimestamp = headTs
                victimAddress = addr
            }
        }
        if (victimAddress != null) {
            val removed = buffers.remove(victimAddress)
            if (removed != null) {
                val count = removed.size
                if (count > 0) total.addAndGet(-count)
                onDropped(victimAddress, DropReason.CAPPED_PEERS, count)
            }
        }
    }

    companion object {
        const val DEFAULT_MAX_PER_PEER: Int = 100
        const val DEFAULT_MAX_PEERS: Int = 32
        // Idle window (reset by each fragment) before a partial buffer is evicted.
        // This holds inbound fragments only until the sender's device-id resolves —
        // a GATT connect+read throttled to one attempt per MIN_RECONNECT_INTERVAL
        // (5s) and prone to GATT-133 retries. At 5s a first-contact multi-fragment
        // MLS Welcome that arrives in a burst could be evicted before resolution
        // completes, losing the Welcome before it ever reaches the Rust reassembler.
        // 15s comfortably exceeds the reverse-resolution worst case while staying
        // well under the 30s Rust reassembly window; memory stays bounded by the
        // per-peer / peer caps above.
        const val DEFAULT_TIMEOUT_MS: Long = 15_000L

        val DEFAULT_BLE_THREAD_CHECK: () -> Unit = {
            BleTransportFacade.assertOnBleLooper("InboundFragmentBuffer")
        }
    }
}
