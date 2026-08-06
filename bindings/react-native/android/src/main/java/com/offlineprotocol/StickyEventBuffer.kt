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
 * meantime can end the session *between* the failed emit and the hold;
 * a flush already carrying entries can reach [restore] after the same teardown.
 * [hold] and [restore] therefore both refuse entries from a generation that is
 * no longer current — a check the caller cannot make for itself, since the
 * generation has to be read before the emit is attempted and compared under the
 * same lock [endSession] takes.
 *
 * **A generation stamp alone covers only half of that window, which is why the
 * two lifecycle transitions are separate methods.** The stamp is read when the
 * emit is attempted, so it refuses a teardown landing between that read and the
 * hold — nanoseconds — while the teardown that actually produces the event runs
 * for as long as the transports take to stop. A `destroy()` landing anywhere in
 * *that* window is simply not seen by the stamp: the emit reads the generation
 * afterwards, finds it current, and holds. [endSession] therefore also closes
 * the buffer to new holds, and only [beginSession] reopens it. Nothing is lost
 * by refusing in between, because no event that qualifies for this buffer can
 * be produced before a `start()`: both enrolled events require a transport that
 * `start()` is what brings up.
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
 * No iOS twin, and after review that is a design conclusion rather than the
 * scope note it started as. iOS solves the same problem for the same event by
 * **re-deriving** the report instead of replaying it: `SupersededLatchPolicy`
 * already holds the authoritative fact (and now the reason with it), so the
 * bridge re-emits from that state on every app foreground while the transport
 * is superseded. Where the two approaches differ is instructive, and the
 * reasons are platform-specific enough that neither should be ported over the
 * other without re-deriving:
 *
 *  - **Re-deriving needs backing state; this buffer exists because
 *    `mesh_stopped_by_user` has none.** It is emitted after the transports,
 *    the scheduler and the core are already down — there is nothing left to
 *    ask. That is exactly why a buffer is the only option here, and why the
 *    iOS-style restatement could not serve it.
 *  - **Re-deriving has no staleness class**, so it needs no generation stamp,
 *    no [endSession]/[beginSession] pair and no [discard] site: a restatement
 *    is true when it is delivered rather than being a replay of a past edge.
 *    The cost is that it repeats, which is fine for a state report and would
 *    not be for a terminal one.
 *  - **A buffer only sees the losses the module itself can observe.** It takes
 *    a hold on the `canEmitToJs` false branch; an emit that passes that gate
 *    and then dies downstream (React re-checking its own listener count, an
 *    invalidated instance, a JS-side unsubscribe racing the native
 *    `removeListeners`) leaves nothing held. Re-deriving covers those too,
 *    because it never depends on having noticed the failure.
 *
 * The most common downstream loss is not on that list because it is not the
 * bridge's to catch: an event that reaches JavaScript intact and finds the
 * *app* has not registered a handler yet. The SDK subscribes to the emitter in
 * its own constructor, so a flush triggered by that subscribe (the usual one)
 * arrives while the app necessarily has nothing bound. Held entries are
 * dropped once handed over — an emit is not a delivery receipt — so nothing
 * here can recover it afterwards. The SDK's TypeScript layer holds one-shot
 * events across that second gap and replays them on the first matching
 * `on(...)`; see `src/index.ts` and `ONE_SHOT_EVENT_TYPES`, whose set must
 * stay equal to the tags enrolled here.
 *
 * docs/react-native-integration.md §6.1 states the resulting contract for both
 * platforms: at-least-once, state rather than edge, idempotent handlers.
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

    // Bumped by [beginSession] and [endSession]. Only ever read and compared
    // under [lock], which is what closes the race against an emit that is
    // already in flight.
    private var sessionGeneration: Long = INITIAL_GENERATION

    // Whether [hold] will take anything. False between [endSession] and the
    // next [beginSession] — the window where the app has torn the SDK down and
    // a teardown started before it can still be walking towards its emit.
    //
    // Starts true rather than false: a freshly constructed buffer has had no
    // lifecycle call at all, and defaulting a *fresh* module to refusing would
    // trade a documented window for an undocumented one. Nothing can be emitted
    // before the first `start()` anyway, so the initial value is unobservable in
    // practice; open is the value that fails towards holding.
    private var accepting: Boolean = true

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
     * Refuses, returning false, in either of the two ways a session can be over
     * by the time an event reaches here. **[generation] is no longer current:**
     * the session this event was emitted for was torn down while the emit was
     * in flight. **Or the buffer is closed** — [endSession] has run and no
     * [beginSession] has followed, so the event belongs to a session that ended
     * before the emit was even attempted, which is the wider of the two windows
     * and the one a stamp read at emit time cannot see. Either way the app is
     * not waiting to be told, and a later subscribe must not be handed it.
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
            if (!accepting || generation != sessionGeneration) return false
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
     * it, and putting those back would quietly undo [endSession]. No separate
     * check on whether the buffer is accepting is needed — both lifecycle
     * transitions bump the generation, so an entry drained before either one
     * already fails the comparison.
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
     * Drops whatever is held under [key], because a newer event for that key
     * has just gone out directly and the held copy is now stale news.
     *
     * Without this, a key held from a failed emit outlives a later successful
     * one and redelivers *after* it — an app that reconciled against the fresh
     * event is then handed the old one, with nothing to correct it. That is the
     * stale-one-shot failure [endSession] exists to prevent, arriving through a
     * different door.
     *
     * Deliberately unconditional on content: the point is that the held copy is
     * *older*, and there is nothing in an event's JSON to establish that. The
     * cost is a theoretical last-writer-wins race — a concurrent [hold] for the
     * same key landing between a caller's successful emit and this call would
     * be discarded — which neither enrolled event can produce (one is emitted
     * from a single teardown thread with a constant payload; the other is
     * latch-idempotent at its source). Stale news delivered last is the worse
     * of the two failures, so this takes the race.
     *
     * Refuses for a superseded [generation] like [hold] does, so a caller whose
     * session ended mid-emit cannot reach into the next one's buffer.
     */
    fun discard(key: String, generation: Long) {
        synchronized(lock) {
            if (generation != sessionGeneration) return
            held.remove(key)
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
     * Ends the current session: discards everything held, refuses whatever is
     * still in flight for it, and **stops taking new holds** until
     * [beginSession].
     *
     * Called from the module's `destroy()`, where the app tore the SDK down
     * itself and is not waiting to be told about a teardown it did not
     * initiate. Clearing alone would not be enough — an emit that failed just
     * before the teardown can reach [hold] just after it, and a flush that had
     * already drained can reach [restore] after it — so both paths gate on the
     * generation this bumps rather than on the map being empty.
     *
     * Nor is bumping the generation enough on its own, which is the whole
     * reason this is not the same method as [beginSession]. The generation an
     * emit compares against is read *at emit time*, so it catches a teardown
     * landing in the instant between that read and the hold and nothing more.
     * The teardown that produces `mesh_stopped_by_user` emits only after every
     * transport has stopped, and shares a lock with `destroy()`'s own teardown
     * — so `destroy()` running to completion first, and the stop thread then
     * emitting into a buffer that has already been cleared, is the *likely*
     * interleaving, not the exotic one. With the buffer still open that hold is
     * accepted under the new generation and handed to whichever session
     * subscribes next: the original bug with the sign flipped, which is what
     * closing costs nothing to prevent.
     */
    fun endSession() {
        synchronized(lock) {
            sessionGeneration += 1
            held.clear()
            accepting = false
        }
    }

    /**
     * Begins a new session: discards anything held for the previous one and
     * takes holds again.
     *
     * Called from the module's `start()`. A one-shot event held from the
     * *previous* mesh session is moot the moment a new one comes up —
     * redelivering it would tell an app that is bringing the mesh up that it is
     * down, with nothing to restate it.
     *
     * Separate from [endSession] because the two transitions mean opposite
     * things about what should happen next: a session beginning must take the
     * events that follow, a session ending must not. One method serving both
     * looks like a tidy `invalidate`, and is exactly how an event emitted by
     * the session `destroy()` just ended gets held for the session after it.
     */
    fun beginSession() {
        synchronized(lock) {
            sessionGeneration += 1
            held.clear()
            accepting = true
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
