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
 * **Every entry is stamped with the session generation it was emitted for, and
 * that is what keeps redelivery honest across a teardown.** Redelivering a
 * *stale* one-shot event is not a milder version of dropping it — it is the
 * same failure inverted: `mesh_stopped_by_user` handed to a session that has
 * since restarted tells the app the mesh is down while it is coming up, and
 * being one-shot, nothing will ever restate it. The window is not theoretical.
 * The teardown that produces that event runs on its own thread and emits only
 * once every transport is stopped, so an app calling `destroy()` in the
 * meantime can invalidate the session *between* the failed emit and the hold;
 * a flush already carrying entries can reach [restore] after the same teardown.
 * [hold] and [restore] therefore both refuse entries from a generation that is
 * no longer current — a check the caller cannot make for itself, since the
 * generation has to be read before the emit is attempted and compared under the
 * same lock [invalidateSession] takes.
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

    /**
     * A held event: its collapsing [key], the JSON to re-emit, and the session
     * [generation] it was emitted for.
     *
     * The generation travels with the entry rather than being re-read when it
     * is put back, because a flush already carrying an entry has no other way
     * to tell whether the session it belongs to still exists.
     */
    data class Entry(val key: String, val eventJson: String, val generation: Long)

    private val lock = Any()

    // Insertion-ordered so a flush redelivers in the order the events were
    // emitted. Re-holding an existing key moves it to the tail: the newest
    // information about a key is also the newest information overall.
    private val held = LinkedHashMap<String, Entry>()

    // Bumped by [invalidateSession]. Only ever read and compared under [lock],
    // which is what closes the race against an emit that is already in flight.
    private var sessionGeneration: Long = INITIAL_GENERATION

    /**
     * The session events are currently being held for.
     *
     * Callers read this *before* attempting an emit and pass it back to [hold],
     * so an emit that fails against a session torn down while it was in flight
     * cannot leave anything behind.
     */
    fun currentGeneration(): Long = synchronized(lock) { sessionGeneration }

    /**
     * Holds [eventJson] for redelivery, replacing any event already held under
     * [key]. Returns whether it was held.
     *
     * Refuses, returning false, when [generation] is no longer current: the
     * session this event was emitted for has been torn down, so the app is not
     * waiting to be told about it and a later subscribe must not be handed it.
     * Callers use the return value to decide whether there is now anything
     * worth flushing.
     *
     * Last-wins rather than append: two stops report the same fact, and an app
     * reconciling against a terminal state gains nothing from the older copy.
     * Past [maxEntries] the oldest entry is evicted — the newest one-shot event
     * is the one most likely to still matter.
     */
    fun hold(key: String, eventJson: String, generation: Long): Boolean {
        synchronized(lock) {
            if (generation != sessionGeneration) return false
            held.remove(key)
            held[key] = Entry(key, eventJson, generation)
            trimToCapLocked()
            return true
        }
    }

    /**
     * Restores entries a flush could not deliver.
     *
     * Skips any key that is held again already: while the flush was in flight a
     * *newer* event may have arrived for the same key, and restoring the copy
     * the flush was carrying would resurrect stale state over it. Silent
     * last-wins in [hold] would otherwise become silent first-wins here.
     *
     * Skips entries from a superseded generation for the reason [hold] refuses
     * them: a flush can be holding entries when the session is torn down under
     * it, and putting those back would quietly undo [invalidateSession].
     *
     * Restored entries go back at the *head*, not the tail. Every one of them
     * was drained before anything now held was taken, so appending would
     * redeliver older news behind newer and break [drain]'s oldest-first
     * contract in precisely the race this method exists for — a flush that
     * could not deliver while a fresh event landed under a different key.
     * Eviction still takes from the head, so an overflow drops the restored
     * (older) entries first, which is the same "newest matters most" rule
     * [hold] applies.
     */
    fun restore(entries: List<Entry>) {
        if (entries.isEmpty()) return
        synchronized(lock) {
            val restorable = entries.filter { entry ->
                entry.generation == sessionGeneration && !held.containsKey(entry.key)
            }
            if (restorable.isEmpty()) return
            val newer = held.values.toList()
            held.clear()
            for (entry in restorable) {
                held[entry.key] = entry
            }
            for (entry in newer) {
                held[entry.key] = entry
            }
            trimToCapLocked()
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
            val drained = held.values.toList()
            held.clear()
            return drained
        }
    }

    /**
     * Ends the current session: discards everything held, and refuses whatever
     * is still in flight for it.
     *
     * Called from the module's `destroy()`, where the app tore the SDK down
     * itself and is not waiting to be told about a teardown it did not
     * initiate. Clearing alone would not be enough — an emit that failed just
     * before the teardown can reach [hold] just after it, and a flush that had
     * already drained can reach [restore] after it — so both paths gate on the
     * generation this bumps rather than on the map being empty.
     */
    fun invalidateSession() {
        synchronized(lock) {
            sessionGeneration += 1
            held.clear()
        }
    }

    /** Whether nothing is waiting — lets callers skip the flush entirely. */
    fun isEmpty(): Boolean = synchronized(lock) { held.isEmpty() }

    /** How many events are held. Exposed for tests. */
    val size: Int
        get() = synchronized(lock) { held.size }

    /** Caller must hold [lock]. */
    private fun trimToCapLocked() {
        while (held.size > maxEntries) {
            val oldest = held.keys.first()
            held.remove(oldest)
        }
    }

    companion object {
        /**
         * Backstop only. Keys come from a fixed set of call sites — two today —
         * so this bounds a bug, not ordinary use.
         */
        const val DEFAULT_MAX_ENTRIES: Int = 8

        /** The generation a freshly constructed buffer holds events for. */
        const val INITIAL_GENERATION: Long = 0L
    }
}
