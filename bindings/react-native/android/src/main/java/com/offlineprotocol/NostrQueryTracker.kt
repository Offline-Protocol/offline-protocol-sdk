package com.offlineprotocol

/**
 * Which relays still owe an end-of-stored-events for each in-flight resolution
 * query, and therefore when a query is finished.
 *
 * A query is broadcast: every connected relay answers it under the same
 * subscription id and each sends its own EOSE. Ending the query on the *first*
 * one makes the answer whatever the fastest relay happened to hold, and for a
 * username resolution that answer is the entire result. A relay holding nothing
 * wins that race by having nothing to send, so an empty or hostile relay would
 * decide what the user sees while every other relay served the honest
 * claimants. A claim is supposed to need only one honest relay to survive, so
 * completion has to wait for all of them.
 *
 * Tracking who is still owed is also what lets each relay's subscription close
 * as soon as *that* relay is done, which is what keeps a finished query from
 * leaving a standing filter on a public routing tag.
 *
 * Extracted from [NostrManager] rather than left inline because that class
 * needs a live OkHttp WebSocket and a UniFFI protocol instance, so nothing in
 * it can be unit tested. This holds the whole state machine and no I/O: the
 * manager keeps the CLOSE sends and the transport hand-back. Mirrors
 * ios/NostrQueryTracker.swift; the two are kept in sync by hand and their
 * completion deadline is pinned across both by a Rust guard.
 *
 * Time arrives as a parameter (monotonic, sleep-inclusive milliseconds, from
 * `SystemClock.elapsedRealtime`) instead of being read here, so a test can
 * drive a deadline without waiting on one and so the clock choice stays the
 * caller's. A wall clock would be wrong: an NTP correction or a manual change
 * would expire live queries early and complete them from a subset of relays,
 * which is the very outcome all-relay completion exists to prevent.
 *
 * Thread-safe. Relay frames arrive on OkHttp's reader thread while the poll
 * loop expires stale queries on the transport handler, so all state is guarded
 * by an internal lock.
 */
class NostrQueryTracker {

    companion object {
        /**
         * How long a query waits for stragglers before it is finished anyway.
         *
         * End-of-stored-events is the only completion signal a Nostr query has
         * and a relay is free never to send one. Bounded well below the
         * engine's own 30s resolution sweep, so the ordinary answer still comes
         * from here and the sweep stays the backstop it was written to be.
         *
         * Hand-mirrored in NostrQueryTracker.swift and pinned across both by
         * `nostr_query_completion_timeout_matches_across_both_bridges`.
         */
        const val COMPLETION_TIMEOUT_MS = 10_000L
    }

    private class QueryProgress(
        /** Relays that have not yet sent end-of-stored-events. */
        val awaiting: MutableSet<String>,
        /** When the query was issued, bounding how long a silent relay holds it. */
        val issuedAtMs: Long
    )

    private val lock = Any()
    private val queries = HashMap<String, QueryProgress>()

    /**
     * Records a query as issued to [relays].
     *
     * Recorded against the relays the REQ actually went to, so a relay that
     * connects later is never waited on for an answer it was never asked for.
     */
    fun issue(subscriptionId: String, relays: Collection<String>, nowMs: Long) {
        synchronized(lock) {
            queries[subscriptionId] = QueryProgress(relays.toMutableSet(), nowMs)
        }
    }

    /**
     * Whether events under this subscription id are resolution records rather
     * than inbound messages. The two go to different entry points.
     */
    fun isActive(subscriptionId: String): Boolean =
        synchronized(lock) { queries.containsKey(subscriptionId) }

    /**
     * Records one relay's end-of-stored-events.
     *
     * Returns true when that was the last relay owed, meaning the caller should
     * release the query. An EOSE for an unknown or already-finished query
     * returns false.
     */
    fun noteEndOfStoredEvents(subscriptionId: String, relayUrl: String): Boolean =
        synchronized(lock) {
            val progress = queries[subscriptionId] ?: return@synchronized false
            if (!progress.awaiting.remove(relayUrl)) return@synchronized false
            if (progress.awaiting.isEmpty()) {
                queries.remove(subscriptionId)
                true
            } else {
                false
            }
        }

    /**
     * Stops waiting on a relay that went away, across every query it owed.
     *
     * Returns the queries that are now complete. A disconnected relay will
     * never send its EOSE, so without this the last query it was asked would
     * wait out the whole deadline instead of finishing as soon as the relays
     * that *can* answer have.
     */
    fun dropRelay(relayUrl: String): List<String> = synchronized(lock) {
        val finished = mutableListOf<String>()
        // Over a snapshot, so the removals below cannot interact with the walk.
        for ((subscriptionId, progress) in queries.entries.toList()) {
            if (!progress.awaiting.remove(relayUrl)) continue
            if (progress.awaiting.isEmpty()) {
                queries.remove(subscriptionId)
                finished.add(subscriptionId)
            }
        }
        finished
    }

    /**
     * Queries whose deadline has passed, left in place for the caller to
     * finish. Non-destructive so that finishing stays one path.
     */
    fun staleQueries(nowMs: Long): List<String> = synchronized(lock) {
        val cutoff = nowMs - COMPLETION_TIMEOUT_MS
        queries.filterValues { it.issuedAtMs < cutoff }.keys.toList()
    }

    /**
     * Ends a query now, returning the relays it was still waiting on so the
     * caller can close their subscriptions. Null if it was not in flight.
     */
    fun finish(subscriptionId: String): Set<String>? =
        synchronized(lock) { queries.remove(subscriptionId)?.awaiting }

    /**
     * Drops every query, returning their ids.
     *
     * For the case where the relays are gone entirely: nothing will ever
     * answer, so holding the ids only pins subscription state until something
     * else evicts it.
     */
    fun clear(): List<String> = synchronized(lock) {
        val ids = queries.keys.toList()
        queries.clear()
        ids
    }
}
