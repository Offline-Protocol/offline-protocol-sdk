package com.offlineprotocol

/**
 * Decides which peers to query for relay presence (CheckPresence) each tick.
 *
 * Watch sources: recipients the relay reported unreachable (DeliveryError /
 * ConnectionRequestError) plus the core watchlist (peers with undelivered or
 * session-unproven MLS welcomes) merged at tick time. A peer leaves the set
 * on an online presence answer, on inbound traffic from the peer, or after
 * the idle TTL.
 *
 * Queries rotate round-robin so a large watch set is fully covered across
 * ticks while staying far under the relay's per-connection rate limit
 * (token bucket: burst 30, 10/s — one tick sends at most
 * [maxQueriesPerTick] frames every [DEFAULT_TICK_INTERVAL_MS] ms).
 *
 * Thread-safe: relay answers and inbound traffic mutate the watch set on
 * OkHttp's reader thread while the tick runs on the main handler, so all
 * state is guarded by an internal lock.
 */
class PresenceWatchPolicy(
    private val idleTtlMs: Long = DEFAULT_IDLE_TTL_MS,
    private val maxQueriesPerTick: Int = DEFAULT_MAX_QUERIES_PER_TICK
) {
    companion object {
        const val DEFAULT_IDLE_TTL_MS = 10 * 60_000L
        const val DEFAULT_MAX_QUERIES_PER_TICK = 10
        const val DEFAULT_TICK_INTERVAL_MS = 20_000L
    }

    private val lock = Any()
    private val lastRelevantAtMs = HashMap<String, Long>()
    private val rotation = ArrayDeque<String>()

    /** Adds a peer to the watch set (or refreshes its idle clock). */
    fun watch(peerId: String, nowMs: Long) {
        if (peerId.isEmpty()) return
        synchronized(lock) {
            if (lastRelevantAtMs.put(peerId, nowMs) == null) rotation.addLast(peerId)
        }
    }

    /** Removes a peer (online answer or inbound traffic proved reachability). */
    fun unwatch(peerId: String) {
        synchronized(lock) {
            if (lastRelevantAtMs.remove(peerId) != null) rotation.remove(peerId)
        }
    }

    fun watchedPeers(): Set<String> = synchronized(lock) { lastRelevantAtMs.keys.toSet() }

    /**
     * Merges the core watchlist (authoritatively still-pending peers refresh
     * their idle clock), evicts idle entries, and returns up to
     * [maxQueriesPerTick] peers to query this tick, round-robin.
     */
    fun peersToQuery(coreWatchlist: List<String>, nowMs: Long): List<String> = synchronized(lock) {
        for (peer in coreWatchlist) {
            watch(peer, nowMs)
        }
        val expired = lastRelevantAtMs.filterValues { nowMs - it > idleTtlMs }.keys.toList()
        for (peer in expired) {
            unwatch(peer)
        }

        if (rotation.isEmpty()) return emptyList()
        val count = minOf(maxQueriesPerTick, rotation.size)
        val result = ArrayList<String>(count)
        repeat(count) {
            val peer = rotation.removeFirst()
            rotation.addLast(peer)
            result.add(peer)
        }
        result
    }

    fun clear() {
        synchronized(lock) {
            lastRelevantAtMs.clear()
            rotation.clear()
        }
    }
}
