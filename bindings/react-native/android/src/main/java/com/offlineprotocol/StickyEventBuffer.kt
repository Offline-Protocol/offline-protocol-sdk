package com.offlineprotocol

/**
 * Holds bridge events that JS could not take, so a *one-shot* event survives a
 * window where nothing was listening.
 *
 * Most events on this bridge are safe to drop: they are periodic, re-derivable,
 * or followed by another carrying the same state. A few are not. A one-shot
 * event reports a state change nothing will ever restate — `mesh_stopped_by_user`
 * is emitted after the transports, the scheduler and the core are already down,
 * so it has no successor to correct it. Dropped, the app keeps reporting an
 * active mesh against a protocol that is fully stopped, indefinitely.
 *
 * The subtlety this class exists to survive is that redelivery cannot rely on a
 * *later* event. There is no later event — that is what makes these one-shot.
 * So the buffer holds until a subscribe or a foreground comes to collect, and
 * the caller drives both (see OfflineProtocolModule.flushStickyEvents).
 *
 * Entries collapse last-wins per key so a repeated stop cannot accumulate
 * copies, and the whole buffer is capped: it holds events emitted while nobody
 * was watching, which is exactly the situation where a leak would go unnoticed.
 * With keys supplied by a fixed set of call sites the cap is unreachable, and
 * it stays a backstop rather than a policy.
 *
 * Values are event JSON, not React Native's `WritableMap`. A `WritableNativeMap`
 * is JNI-backed and handed to the bridge by transfer, so holding one across two
 * emits is at best unspecified; rebuilding the map at flush time sidesteps the
 * question, and keeping this class free of Android and React types is what lets
 * it be unit-tested without Robolectric.
 *
 * Unlike most policy classes here, this one **is** internally synchronized —
 * the caller cannot own the threading, because the whole point is that writers
 * and readers are different threads that share no lock: a hold arrives from
 * whichever thread emitted (including the `"mesh-user-stop"` thread), while
 * flushes come from React Native's JS and UI threads. Follows RelayRateLimiter's
 * lock idiom.
 *
 * In-memory and per-module by design, with the residual stated plainly: a JS
 * reload replaces the module and a process kill takes the whole thing, so a
 * held event does not survive either. Persisting it would not help — the
 * process-death case never generates the event in the first place, since with
 * no module registered MeshForegroundService takes its no-host fallback. Apps
 * that must be right across those windows reconcile with `getState()` on
 * foreground; see docs/react-native-integration.md.
 *
 * No iOS twin, deliberately: the notification Stop action that produces the
 * event this was built for is Android-only, and iOS has no one-shot event of
 * this shape to hold. Mirroring it there now would be an abstraction with no
 * caller.
 */
class StickyEventBuffer(private val maxEntries: Int = DEFAULT_MAX_ENTRIES) {

    /** A held event: its collapsing [key] and the JSON to re-emit. */
    data class Entry(val key: String, val eventJson: String)

    private val lock = Any()

    // Insertion-ordered so a flush redelivers in the order the events were
    // emitted. Re-holding an existing key moves it to the tail: the newest
    // information about a key is also the newest information overall.
    private val held = LinkedHashMap<String, String>()

    /**
     * Holds [eventJson] for redelivery, replacing any event already held under
     * [key].
     *
     * Last-wins rather than append: two stops report the same fact, and an app
     * reconciling against a terminal state gains nothing from the older copy.
     * Past [maxEntries] the oldest entry is evicted — the newest one-shot event
     * is the one most likely to still matter.
     */
    fun hold(key: String, eventJson: String) {
        synchronized(lock) {
            held.remove(key)
            held[key] = eventJson
            while (held.size > maxEntries) {
                val oldest = held.keys.first()
                held.remove(oldest)
            }
        }
    }

    /**
     * Restores entries a flush could not deliver.
     *
     * Skips any key that is held again already: while the flush was in flight a
     * *newer* event may have arrived for the same key, and restoring the copy
     * the flush was carrying would resurrect stale state over it. Silent
     * last-wins in [hold] would otherwise become silent first-wins here.
     */
    fun restore(entries: List<Entry>) {
        if (entries.isEmpty()) return
        synchronized(lock) {
            for (entry in entries) {
                if (!held.containsKey(entry.key)) {
                    held[entry.key] = entry.eventJson
                }
            }
            while (held.size > maxEntries) {
                val oldest = held.keys.first()
                held.remove(oldest)
            }
        }
    }

    /**
     * Removes and returns everything held, oldest first.
     *
     * Drain-and-take rather than peek-then-acknowledge: the caller has no
     * delivery receipt to acknowledge with (React Native reports an emit failure
     * asynchronously on the New Architecture, or not at all), so the honest
     * shape is to hand the entries over and let the caller [restore] whatever
     * the emit refused.
     */
    fun drain(): List<Entry> {
        synchronized(lock) {
            if (held.isEmpty()) return emptyList()
            val drained = held.map { (key, json) -> Entry(key, json) }
            held.clear()
            return drained
        }
    }

    /** Discards everything held, for a teardown that makes redelivery moot. */
    fun clear() {
        synchronized(lock) { held.clear() }
    }

    /** Whether nothing is waiting — lets callers skip the flush entirely. */
    fun isEmpty(): Boolean = synchronized(lock) { held.isEmpty() }

    /** How many events are held. Exposed for tests. */
    val size: Int
        get() = synchronized(lock) { held.size }

    companion object {
        /**
         * Backstop only. Keys come from a fixed set of call sites — two today —
         * so this bounds a bug, not ordinary use.
         */
        const val DEFAULT_MAX_ENTRIES: Int = 8
    }
}
