package com.offlineprotocol

/**
 * When the LAN carrier dials an advertised address, when it dials again, and
 * who may open a stream to it. Framework-free; the manager owns the timers
 * and sockets. Mirrors iOS's `PeerStreamDialPolicy` (PeerStreamSession.swift)
 * case for case, keep in sync (ADR 0027, ADR 0028).
 *
 * Not thread-safe: [LanPeerDiscovery] calls it on the transport thread only.
 */
internal class PeerStreamDialPolicy {
    /** Addresses with a dial scheduled or an outbound stream open. */
    private val dialing = HashSet<String>()
    private val redialDelayMs = HashMap<String, Long>()

    /**
     * A peer advertised [address]. The delay to dial after, or null when no
     * dial is due: one is already under way, or a stream holds the address.
     */
    fun discovered(address: String, weAreLower: Boolean, held: Boolean): Long? =
        schedule(address, held, if (weAreLower) 0L else HIGHER_ADDRESS_DELAY_MS)

    /**
     * An outbound stream toward [address] ended. The delay to redial after,
     * or null when the advert is gone or another stream holds the address.
     */
    fun ended(address: String, advertised: Boolean, held: Boolean): Long? {
        dialing.remove(address)
        if (!advertised) return null
        val delay = redialDelayMs[address] ?: REDIAL_INITIAL_DELAY_MS
        schedule(address, held, delay) ?: return null
        redialDelayMs[address] = minOf(delay * 2, REDIAL_MAX_DELAY_MS)
        return delay
    }

    /**
     * A scheduled dial opened nothing because it is not due: the advert went,
     * the address is now held, or the transport paused.
     */
    fun abandoned(address: String) {
        dialing.remove(address)
    }

    /**
     * A scheduled dial found every stream slot taken. The delay to try again
     * after, on the redial ladder: abandoning it would lose the peer for
     * good, since nothing else dials an address whose record did not change.
     */
    fun noSlot(address: String): Long? = ended(address, advertised = true, held = false)

    /** An outbound stream toward [address] proved it: the ladder starts over. */
    fun proved(address: String) {
        redialDelayMs.remove(address)
    }

    fun reset() {
        dialing.clear()
        redialDelayMs.clear()
    }

    private fun schedule(address: String, held: Boolean, delay: Long): Long? {
        if (held || address in dialing) return null
        dialing.add(address)
        return delay
    }

    companion object {
        /**
         * How long the higher address of a pair waits before dialing, so that
         * the lower one's stream, which both ends keep, usually arrives first.
         */
        const val HIGHER_ADDRESS_DELAY_MS = 5_000L
        const val REDIAL_INITIAL_DELAY_MS = 1_000L
        const val REDIAL_MAX_DELAY_MS = 60_000L

        /**
         * Whether the listener takes a connection from [host], given the hosts
         * of the inbound streams open now and the count of all open streams.
         *
         * The listener binds every interface, so on a shared network anyone
         * on it can connect, not only group members. Two bounds keep that
         * from costing this device its own dials: inbound streams take at
         * most [PeerStreamSockets.Limits.maxInbound] of the budget, and one
         * remote address at most [PeerStreamSockets.Limits.maxInboundPerHost]
         * of those. Unknown hosts (null) share one bound.
         */
        fun admitsInbound(
            host: String?,
            inboundFrom: Collection<String?>,
            open: Int,
            limits: PeerStreamSockets.Limits,
        ): Boolean =
            open < limits.maxStreams &&
                inboundFrom.size < limits.maxInbound &&
                inboundFrom.count { it == host } < limits.maxInboundPerHost

        /**
         * The record each advertised address is dialed at, from every peer
         * record held now ([records] as address to record, our own left out).
         *
         * Rebuilt whole on each change, never patched per record, because one
         * address can be advertised by two records at once: a peer whose
         * record outlived it beside the one it published on return. Removing
         * a record by its address took the live record's address with it. Of
         * two records for one address, one in [fresh] (just resolved) wins,
         * then the one in [current], which a dial may be using. A record in
         * [unprovable] is left out: its dial was answered by a peer that did
         * not prove the address it advertises.
         */
        fun <E> adverts(
            records: List<Pair<String, E>>,
            fresh: Set<E>,
            current: Map<String, E>,
            unprovable: Set<E> = emptySet(),
        ): Map<String, E> {
            fun rank(address: String, endpoint: E): Int =
                if (endpoint in fresh) 2 else if (current[address] == endpoint) 1 else 0
            val out = HashMap<String, E>()
            for ((address, endpoint) in records) {
                if (endpoint in unprovable) continue
                val kept = out[address]
                if (kept != null && rank(address, kept) >= rank(address, endpoint)) continue
                out[address] = endpoint
            }
            return out
        }
    }
}
