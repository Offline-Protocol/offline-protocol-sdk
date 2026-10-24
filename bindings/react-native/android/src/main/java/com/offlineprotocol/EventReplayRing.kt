package com.offlineprotocol

/**
 * The last [capacity] events handed to JavaScript, by the number each was
 * handed over with, so the TypeScript layer can fetch one that never arrived
 * (#514).
 *
 * Numbering alone only reports a loss. On the New Architecture an emit that
 * React Native drops returns normally, so the module cannot tell, and an event
 * such as `message_received` or `message_delivered` reports something the core
 * has already settled for good: the message was acknowledged, its id
 * dedup-marked, its outbox entry removed. Nothing will restate it, so a lost
 * one is a lost message unless a copy is kept where JavaScript can ask for it.
 *
 * Only events that were handed over are recorded, under the same lock that
 * gives out their numbers, so the ring holds a contiguous run and a number JS
 * expects but never saw is either here or older than the ring. The cap is a
 * real capacity: past it the oldest goes, and a gap that reaches back further
 * is reported rather than recovered.
 *
 * Values are event JSON, never React Native maps, which keeps this class free
 * of React types: it compiles into the Android library unchanged, and it is
 * unit-tested without Robolectric.
 *
 * Internally synchronized: [record] runs on whichever thread drove the core,
 * [replay] on the native-modules thread.
 */
class EventReplayRing(private val capacity: Int = DEFAULT_CAPACITY) {

    data class Entry(val seq: Int, val eventJson: String)

    private val entries = ArrayDeque<Entry>()
    private val lock = Any()

    init {
        require(capacity > 0) { "capacity must be positive" }
    }

    /** Keeps [eventJson] as the event handed over with [seq]. */
    fun record(seq: Int, eventJson: String) {
        synchronized(lock) {
            entries.addLast(Entry(seq, eventJson))
            while (entries.size > capacity) {
                entries.removeFirst()
            }
        }
    }

    /**
     * Every kept event numbered [fromSeq] to [toSeq] inclusive, in order. A
     * number older than the ring is simply absent: the caller compares what
     * came back with what it asked for.
     */
    fun replay(fromSeq: Int, toSeq: Int): List<Entry> = synchronized(lock) {
        if (toSeq < fromSeq) return emptyList()
        entries.filter { it.seq in fromSeq..toSeq }
    }

    /**
     * Forgets everything, for a module that starts numbering from zero again.
     * Without it a recreated protocol could serve an earlier session's event
     * under a number the new session reuses.
     */
    fun clear() {
        synchronized(lock) { entries.clear() }
    }

    val size: Int
        get() = synchronized(lock) { entries.size }

    companion object {
        const val DEFAULT_CAPACITY = 256

        /**
         * How a `peer_rssi` event's JSON begins: the engine writes the `type`
         * tag first. Pinned against the engine's own output by
         * `peer_rssi_is_left_out_of_the_replay_ring` in the Rust suite.
         */
        const val PEER_RSSI_PREFIX = "{\"type\":\"peer_rssi\","

        /**
         * Whether an event is numbered and kept here. Not `peer_rssi`: it
         * arrives up to four times a second per peer and the next one replaces
         * it, so a lost one needs no replay, and numbered it would push out of
         * this ring the `message_received` and `message_delivered` events that
         * do, within seconds in a crowd.
         */
        fun isReplayed(eventJson: String?): Boolean =
            eventJson == null || !eventJson.startsWith(PEER_RSSI_PREFIX)
    }
}
