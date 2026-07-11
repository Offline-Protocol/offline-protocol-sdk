package com.offlineprotocol

/**
 * Tracks wire-level in-flight message ids per recipient so the relay's
 * recipient-keyed failure signals (DeliveryError / ConnectionRequestError —
 * neither carries a message_id) can be correlated back to the SDK message
 * ids still awaiting an outcome.
 *
 * Entries are plane-tagged because each plane has its own relay answer
 * channel: only SendMessage frames ([Plane.DATA]) earn a `MessageSent` and
 * fail via `DeliveryError`, while relay-native connection-request ops
 * ([Plane.CONN_REQ]) fail via `ConnectionRequestError`. Correlating across
 * planes would let a conn_req entry absorb a data frame's `MessageSent`,
 * leaving the delivered data message tracked for a later `DeliveryError`
 * to false-fail.
 *
 * Recipient-keyed correlation within one plane is the best available and
 * safe by construction: everything in flight to an offline peer failed.
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

    /**
     * Which relay answer channel a frame belongs to. Group-plane primaries
     * (CreateGroup / SendGroupMessage / LeaveGroup) are deliberately not
     * representable: their error channel is the group-scoped GroupError,
     * not a recipient-keyed signal, so they must never be tracked here.
     */
    enum class Plane { DATA, CONN_REQ }

    private data class InFlight(val messageId: String, val plane: Plane, val sentAtMs: Long)

    private val lock = Any()
    private val byRecipient = HashMap<String, ArrayDeque<InFlight>>()

    /** Records a wire send; entries beyond the per-recipient cap drop oldest-first. */
    fun recordSent(recipient: String, messageId: String, plane: Plane, nowMs: Long) {
        if (recipient.isEmpty() || messageId.isEmpty()) return
        synchronized(lock) {
            val queue = byRecipient.getOrPut(recipient) { ArrayDeque() }
            queue.addLast(InFlight(messageId, plane, nowMs))
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
     * Resolves one DATA entry on the relay's `MessageSent` answer: the
     * relay accepted and forwarded that frame, so it must not be swept into
     * a later recipient-keyed `DeliveryError` (which would false-fail a
     * delivered message — for a welcome, parking a lifecycle the peer
     * actually received). Removes the exact `messageId` when the relay
     * echoed ours; otherwise removes the oldest DATA entry for the
     * recipient — data sends per recipient are FIFO on one socket and the
     * relay answers them in order, so oldest-first is sound within the
     * plane. CONN_REQ entries are never candidates: only SendMessage frames
     * earn a `MessageSent`, and letting a conn_req entry absorb one would
     * leave the accepted data frame tracked for a false-fail.
     */
    fun resolveOnRelayAccepted(recipient: String, messageId: String?, nowMs: Long) {
        if (recipient.isEmpty()) return
        synchronized(lock) {
            val queue = byRecipient[recipient] ?: return
            while (queue.isNotEmpty() && nowMs - queue.first().sentAtMs > ttlMs) {
                queue.removeFirst()
            }
            val exactMatch = messageId != null &&
                queue.removeAll { it.plane == Plane.DATA && it.messageId == messageId }
            if (!exactMatch) {
                queue.firstOrNull { it.plane == Plane.DATA }?.let { queue.remove(it) }
            }
            if (queue.isEmpty()) byRecipient.remove(recipient)
        }
    }

    /**
     * Removes and returns every live (non-expired) in-flight id of [plane]
     * for a recipient. `DeliveryError` answers the DATA plane and
     * `ConnectionRequestError` the CONN_REQ plane; each must fail only its
     * own frames.
     */
    fun drainRecipient(recipient: String, plane: Plane, nowMs: Long): List<String> {
        synchronized(lock) {
            val queue = byRecipient[recipient] ?: return emptyList()
            val drained = queue.filter { it.plane == plane }
            queue.removeAll { it.plane == plane }
            if (queue.isEmpty()) byRecipient.remove(recipient)
            return drained.filter { nowMs - it.sentAtMs <= ttlMs }.map { it.messageId }
        }
    }

    /** Drops entries older than the TTL, regardless of plane; called from the poll tick. */
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
