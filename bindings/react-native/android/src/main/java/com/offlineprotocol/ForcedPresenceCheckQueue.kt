package com.offlineprotocol

/**
 * Parked forced presence checks (`checkPresence(force = true)`): the
 * park / expire / fail-fast / drain policy, kept Looper-free so the JVM
 * harness can test it — the Handler shell (posting, the retry tick) stays
 * in InternetManager.
 *
 * Not thread-safe: the owner confines every call to one thread (the
 * bridge's main handler). Callers pass `nowMs` explicitly (testability),
 * from the same monotonic source as the entry's deadline. Each entry's
 * callback fires exactly once: on every non-park decision here, or after
 * the owner re-attempts an entry returned by [takeAll], or on [drainAll].
 *
 * Mirrors ios/ForcedPresenceCheckQueue.swift — keep in sync.
 */
class ForcedPresenceCheckQueue(
    private val capacity: Int = DEFAULT_CAPACITY
) {
    companion object {
        /**
         * Parked entries are app-level promises awaiting an ~8s deadline;
         * more than this many concurrent forced checks means the app is
         * calling in a loop. New checks are rejected (resolved false) at
         * capacity instead of growing the queue without bound — existing
         * entries keep their earlier deadlines and are never evicted.
         */
        const val DEFAULT_CAPACITY = 32
    }

    class Entry(
        val userId: String,
        val deadlineMs: Long,
        val callback: (Boolean) -> Unit
    )

    private val parked = mutableListOf<Entry>()

    val isEmpty: Boolean
        get() = parked.isEmpty()

    /**
     * Decides an unsendable check's fate: fail fast on a stopping/stopped
     * transport (no reconnect is coming), expire at/past its deadline,
     * reject at capacity, otherwise park. Every non-park outcome resolves
     * the callback (false) before returning. Returns true iff the entry
     * parked — the owner must then ensure a retry tick is scheduled.
     */
    fun parkOrExpire(entry: Entry, transportStopped: Boolean, nowMs: Long): Boolean {
        if (transportStopped) {
            entry.callback(false)
            return false
        }
        if (nowMs >= entry.deadlineMs) {
            entry.callback(false)
            return false
        }
        if (parked.size >= capacity) {
            entry.callback(false)
            return false
        }
        parked.add(entry)
        return true
    }

    /**
     * Removes and returns every parked entry, oldest first. The owner
     * re-attempts each (a retry tick or the authenticated edge);
     * still-unsendable entries come back via [parkOrExpire].
     */
    fun takeAll(): List<Entry> {
        if (parked.isEmpty()) return emptyList()
        val all = parked.toList()
        parked.clear()
        return all
    }

    /** Resolves every parked entry false (explicit transport stop). */
    fun drainAll() {
        for (entry in takeAll()) {
            entry.callback(false)
        }
    }
}
