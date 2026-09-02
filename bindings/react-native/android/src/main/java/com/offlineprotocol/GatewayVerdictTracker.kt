package com.offlineprotocol

/**
 * Tracks submitted frames until the gateway's verdict settles them.
 *
 * The gateway answers every `SendMessage` with exactly one `MessageSent` or
 * `DeliveryError`, correlated by `message_id`. This holds the ids between the
 * two, so the manager can bound how many are outstanding, notice the ones a
 * gateway never answered, and settle every one of them when a connection dies.
 *
 * Three rules it exists to enforce, each of which was a real defect on some
 * implementation of this contract before it was written down:
 *
 * 1. **An id already in flight is not sent again.** The core re-queues an
 *    unconfirmed frame under the same id after its own acknowledgement timeout,
 *    and a verdict can honestly take longer than that over a radio backbone.
 *    Sending it twice forwards the frame twice and, when the second copy times
 *    out, fails an id the gateway already confirmed.
 * 2. **Every id is settled exactly once.** [settle] returns whether this call
 *    was the one that removed it, so a duplicate verdict cannot report a second
 *    outcome for a frame the core has already moved on from.
 * 3. **Nothing is left waiting on a dead connection.** [drainAll] hands back
 *    every outstanding id so the manager can fail them; a frame nobody answers
 *    for costs the core its full 120-second expiry.
 *
 * Thread-safe: the manager touches it from its IO looper and from the receive
 * thread. Mirrors `GatewayVerdictTracker.swift`.
 */
class GatewayVerdictTracker {

    private val lock = Any()
    private val inFlight = LinkedHashMap<String, Long>()

    /** Frames outstanding right now. */
    val count: Int
        get() = synchronized(lock) { inFlight.size }

    /**
     * Records [messageId] as submitted at [nowMs].
     *
     * Returns `false` when it was already in flight, in which case the caller
     * must **not** send it again. Popping it from the core's outbox was enough:
     * the attempt already outstanding is what settles that id, and the core's
     * pending entry is refreshed by the pop.
     */
    fun begin(messageId: String, nowMs: Long): Boolean =
        synchronized(lock) {
            if (inFlight.containsKey(messageId)) return false
            inFlight[messageId] = nowMs
            true
        }

    /**
     * Settles [messageId]. Returns `false` if it was not outstanding, which is
     * how a duplicate or unsolicited verdict is ignored.
     */
    fun settle(messageId: String): Boolean =
        synchronized(lock) { inFlight.remove(messageId) != null }

    /**
     * Removes and returns every id submitted more than [timeoutMs] ago.
     *
     * A gateway that answers nothing is a contract violation, but it is also
     * indistinguishable from one whose socket is wedged, and the core cannot
     * retry a frame nobody has failed. Removing them here is what turns silence
     * back into a retry.
     */
    fun expired(nowMs: Long, timeoutMs: Long): List<String> =
        synchronized(lock) {
            val stale = inFlight.filterValues { nowMs - it > timeoutMs }.keys.toList()
            stale.forEach { inFlight.remove(it) }
            stale
        }

    /**
     * Removes and returns everything outstanding, for a connection that is
     * going away.
     */
    fun drainAll(): List<String> =
        synchronized(lock) {
            val ids = inFlight.keys.toList()
            inFlight.clear()
            ids
        }
}
