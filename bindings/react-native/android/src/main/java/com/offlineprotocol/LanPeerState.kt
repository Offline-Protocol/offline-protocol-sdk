package com.offlineprotocol

/**
 * The LAN carrier's resolves: what NSD reports, what waits to be resolved,
 * and the one resolve in flight. Framework-free, so the rules are pinned by
 * `LanPeerStateTest`; [LanPeerDiscovery] owns the NsdManager calls and the
 * timers. Not thread-safe: the transport thread only.
 *
 * The rules, each of which a device found broken first:
 * - One resolve at a time: before API 34 a second fails ALREADY_ACTIVE.
 * - An answer is applied only while its resolve is the current one and NSD
 *   still reports the service. NSD sends one loss, so a record written for a
 *   service lost meanwhile would never be removed.
 * - A resolve that gives up (failed, timed out, waited out ALREADY_ACTIVE)
 *   is queued again later while the service is reported: NSD reports it
 *   again only after losing it.
 */
internal class ResolveQueue<S : Any>(
    private val nameOf: (S) -> String,
    private val maxAlreadyActive: Int = 5,
) {
    /** A resolve handed out by [next]. Answers are matched to it by identity. */
    class Ticket<S>(val item: S)

    private val reported = HashMap<String, S>()
    private val pending = ArrayDeque<S>()
    private var inFlight: Ticket<S>? = null
    private var alreadyActive = 0

    /** NSD reported [item]: queued for a resolve. */
    fun found(item: S) {
        reported[nameOf(item)] = item
        queue(nameOf(item))
    }

    /** NSD lost [name]: no longer queued, and an answer in flight for it is not applied. */
    fun lost(name: String) {
        reported.remove(name)
        pending.removeAll { nameOf(it) == name }
    }

    /** Queues [name] again, if NSD still reports it and it is not already waiting. */
    fun queue(name: String) {
        val item = reported[name] ?: return
        if (pending.none { nameOf(it) == name }) pending.add(item)
    }

    /** The next resolve to start, or null while one is in flight or none waits. */
    fun next(): Ticket<S>? {
        if (inFlight != null) return null
        val item = pending.removeFirstOrNull() ?: return null
        return Ticket(item).also { inFlight = it }
    }

    /**
     * [ticket] answered. True when the answer is to be applied: it is still
     * the current resolve and its service is still reported.
     */
    fun answered(ticket: Ticket<S>): Boolean {
        if (inFlight !== ticket) return false
        inFlight = null
        alreadyActive = 0
        return reported.containsKey(nameOf(ticket.item))
    }

    /**
     * [ticket] failed or timed out. True when it was the current resolve;
     * the caller then queues it again later ([queue]) and starts the next.
     */
    fun gaveUp(ticket: Ticket<S>): Boolean {
        if (inFlight !== ticket) return false
        inFlight = null
        alreadyActive = 0
        return true
    }

    /**
     * [ticket] was answered ALREADY_ACTIVE: a resolve from before a restart
     * still runs. True when it waits (put back at the head, still in flight
     * until [release]); false once it has waited [maxAlreadyActive] times in
     * a row, when the caller gives it up.
     */
    fun waitForActive(ticket: Ticket<S>): Boolean {
        if (inFlight !== ticket) return false
        if (++alreadyActive > maxAlreadyActive) {
            alreadyActive = 0
            return false
        }
        if (reported.containsKey(nameOf(ticket.item))) pending.addFirst(ticket.item)
        return true
    }

    /** Ends [ticket]'s wait. True when it was still the current resolve. */
    fun release(ticket: Ticket<S>): Boolean {
        if (inFlight !== ticket) return false
        inFlight = null
        return true
    }

    /** Whether NSD reports [name] now. */
    fun isReported(name: String): Boolean = reported.containsKey(name)

    /** Forgets everything; a resolve in flight is retired. */
    fun clear() {
        reported.clear()
        pending.clear()
        inFlight = null
        alreadyActive = 0
    }
}

/**
 * Which matching Wi-Fi network the LAN carrier follows. More than one can be
 * up at once (Android's make-before-break switch, a local-only network
 * another app asked for). The current one is kept until it is lost, then
 * another is adopted: following whichever network spoke last tore down
 * every LAN stream on each change from another, and could settle on one
 * about to go, leaving none while another was still up. Framework-free and
 * pinned by `LanPeerStateTest`. Not thread-safe: the transport thread only.
 */
internal class NetworkChoice<N : Any, P : Any> {
    enum class Up {
        /** No network was current: adopt this one. */
        ADOPT,
        /** The current network changed: update its properties. */
        UPDATE,
        /** Another network: kept in case the current one is lost. */
        WAIT,
    }

    private val candidates = LinkedHashMap<N, P>()

    var current: N? = null
        private set

    fun up(network: N, properties: P): Up {
        candidates[network] = properties
        return when (current) {
            null -> { current = network; Up.ADOPT }
            network -> Up.UPDATE
            else -> Up.WAIT
        }
    }

    /** [network] was lost. True when it was the current one. */
    fun lost(network: N): Boolean {
        candidates.remove(network)
        if (network != current) return false
        current = null
        return true
    }

    /** Adopts another matching network, if one is up, and returns it. */
    fun fallback(): Pair<N, P>? {
        val (network, properties) = candidates.entries.firstOrNull() ?: return null
        current = network
        return network to properties
    }

    fun clear() {
        candidates.clear()
        current = null
    }
}
