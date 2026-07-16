package com.offlineprotocol

/**
 * Tracks wire-level in-flight message ids per recipient so the relay's
 * recipient-keyed failure signal (DeliveryError — it carries no message_id)
 * can be correlated back to the SDK message ids still awaiting an outcome.
 *
 * Every tracked frame is a SendMessage: connection ops ship verbatim on the
 * data plane like any other traffic, and group-plane primaries (CreateGroup /
 * SendGroupMessage / LeaveGroup) are deliberately never recorded — their
 * error channel is the group-scoped GroupError, not a recipient-keyed
 * signal.
 *
 * Recipient-keyed correlation is the best available and safe by
 * construction: everything in flight to an offline peer failed.
 *
 * Thread-safe: sends record on the main handler's poll loop while the
 * relay's failure signals drain on OkHttp's reader thread, so all state is
 * guarded by an internal lock.
 */
class RecipientInFlightTracker(
    private val ttlMs: Long = DEFAULT_TTL_MS,
    private val maxPerRecipient: Int = DEFAULT_MAX_PER_RECIPIENT
) {
    companion object {
        const val DEFAULT_TTL_MS = 60_000L
        const val DEFAULT_MAX_PER_RECIPIENT = 32
    }

    private data class InFlight(val messageId: String, val sentAtMs: Long)

    private val lock = Any()
    private val byRecipient = HashMap<String, ArrayDeque<InFlight>>()

    /** Records a wire send; entries beyond the per-recipient cap drop oldest-first. */
    fun recordSent(recipient: String, messageId: String, nowMs: Long) {
        if (recipient.isEmpty() || messageId.isEmpty()) return
        synchronized(lock) {
            val queue = byRecipient.getOrPut(recipient) { ArrayDeque() }
            queue.addLast(InFlight(messageId, nowMs))
            while (queue.size > maxPerRecipient) queue.removeFirst()
        }
    }

    /**
     * Removes a specific entry — the send was never written (a false socket
     * write), so there is no relay outcome to correlate. Mirrors
     * ios/RecipientInFlightTracker.swift's unrecord.
     */
    fun unrecord(recipient: String, messageId: String) {
        synchronized(lock) {
            val queue = byRecipient[recipient] ?: return
            queue.removeAll { it.messageId == messageId }
            if (queue.isEmpty()) byRecipient.remove(recipient)
        }
    }

    /**
     * Resolves one entry on the relay's `MessageSent` answer: the relay
     * accepted and forwarded that frame, so it must not be swept into a
     * later recipient-keyed `DeliveryError` (which would false-fail a
     * delivered message — for a welcome, parking a lifecycle the peer
     * actually received). Removes the exact `messageId` when the relay
     * echoed ours; otherwise removes the oldest entry for the recipient —
     * sends per recipient are FIFO on one socket and the relay answers them
     * in order, so oldest-first is the sound correlation.
     */
    fun resolveOnRelayAccepted(recipient: String, messageId: String?, nowMs: Long) {
        if (recipient.isEmpty()) return
        synchronized(lock) {
            val queue = byRecipient[recipient] ?: return
            while (queue.isNotEmpty() && nowMs - queue.first().sentAtMs > ttlMs) {
                queue.removeFirst()
            }
            val exactMatch = messageId != null &&
                queue.removeAll { it.messageId == messageId }
            if (!exactMatch && queue.isNotEmpty()) {
                queue.removeFirst()
            }
            if (queue.isEmpty()) byRecipient.remove(recipient)
        }
    }

    /**
     * Removes and returns every live (non-expired) in-flight id for a
     * recipient.
     */
    fun drainRecipient(recipient: String, nowMs: Long): List<String> {
        synchronized(lock) {
            val queue = byRecipient.remove(recipient) ?: return emptyList()
            return queue.filter { nowMs - it.sentAtMs <= ttlMs }.map { it.messageId }
        }
    }

    /** Drops entries older than the TTL; called from the poll tick. */
    fun prune(nowMs: Long) {
        synchronized(lock) {
            val iterator = byRecipient.entries.iterator()
            while (iterator.hasNext()) {
                val entry = iterator.next()
                while (entry.value.isNotEmpty() && nowMs - entry.value.first().sentAtMs > ttlMs) {
                    entry.value.removeFirst()
                }
                if (entry.value.isEmpty()) iterator.remove()
            }
        }
    }

    /** Forgets everything — the socket died and the transport layer owns the outcome. */
    fun clear() {
        synchronized(lock) {
            byRecipient.clear()
        }
    }
}
