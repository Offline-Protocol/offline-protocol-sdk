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
    }
}
