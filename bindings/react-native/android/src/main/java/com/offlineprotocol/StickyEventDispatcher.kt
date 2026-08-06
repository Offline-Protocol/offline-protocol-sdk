package com.offlineprotocol

/**
 * Drives redelivery of *one-shot* bridge events: emits, holds what JS could not
 * take, and flushes the hold when a subscribe or a foreground says it can.
 *
 * Split from the module rather than inlined there because the three orderings
 * below are what make redelivery correct, and every one of them is invisible in
 * a diff and silent when it regresses. `react-android` is `compileOnly` in the
 * CI harness, so nothing that touches a `ReactContext` can be unit-tested here;
 * behind the [canEmit] / [emit] / [schedule] seams all three are ordinary
 * assertions. Same reason [StickyEventBuffer], ForegroundReconnectPolicy,
 * SupersededLatchPolicy and RelayRateLimiter are separate classes.
 *
 * **1. The session generation is read before the emit is attempted**, not when
 * the hold is taken. `mesh_stopped_by_user` is emitted at the end of a teardown
 * that runs on its own thread for as long as the transports take to stop, so an
 * app calling `destroy()` in that window would otherwise have its buffer cleared
 * and then *refilled* by the emit already in flight — handing a terminal mesh
 * event to whichever session subscribed next, telling it the mesh was down while
 * it was coming up. That is the original bug with the sign flipped. See
 * [StickyEventBuffer]'s generation contract.
 *
 * **2. A successful hold re-runs [flush].** Between the failed emit and the hold
 * a subscribe may have opened the gate and found the buffer still empty, so
 * nothing would be left to trigger redelivery. With the SDK subscribing once in
 * its constructor the next `addListener` never comes, and the event would wait
 * for a foreground that may be hours away.
 *
 * **3. Delivery hops onto the JS queue via [schedule] and must never be
 * flattened into a direct emit.** `NativeEventEmitter.addListener` calls the
 * native `addListener` *before* it registers the JS-side listener, so an emit
 * issued synchronously from there would arrive at an emitter with nothing
 * subscribed — re-losing the event through a subtler version of the hole this
 * closes. React Native's `runOnJSQueueThread` always posts and never runs
 * inline, even from the JS thread, so the flush lands after that registration
 * under both the bridge and the New Architecture.
 *
 * @param buffer where undeliverable events wait.
 * @param canEmit whether an emit has any chance of reaching JS. Checked before
 *   [emit] so a caller that builds a payload (a JNI-backed `WritableMap`) does
 *   not build one it will not use, and before [schedule] so a doomed runnable is
 *   never posted.
 * @param emit hands one event's JSON to JS, returning whether it got that far.
 *   Never a delivery receipt — nothing at this layer observes JS receiving
 *   anything — which is why [deliverHeld] drops entries once handed over rather
 *   than waiting on a confirmation that does not exist.
 * @param schedule posts onto the JS queue, returning false if that thread has
 *   already finished. False leaves the events held for the next trigger.
 */
class StickyEventDispatcher(
    private val buffer: StickyEventBuffer,
    private val canEmit: () -> Boolean,
    private val emit: (String) -> Boolean,
    private val schedule: (Runnable) -> Boolean,
) {

    /**
     * Emits a one-shot event, holding it for redelivery if JS could not take it.
     *
     * [key] identifies the event for last-wins collapsing, so a repeated stop
     * cannot accumulate copies. Callers pass the event's `type` tag.
     *
     * The hold sits in a `finally` for the same reason [deliverHeld]'s restore
     * does: an [emit] that *throws* delivered nothing, and treating a throw as
     * anything but a failed attempt would drop the event on the floor — the
     * exact hole this class closes. The module's [emit] catches its own
     * exceptions, but it builds a JNI-backed payload outside that catch and an
     * `Error` escapes it regardless. The throwable still propagates; it is only
     * the event that is no longer lost with it.
     */
    fun send(key: String, eventJson: String) {
        val generation = buffer.currentGeneration()
        var delivered = false
        try {
            delivered = canEmit() && emit(eventJson)
        } finally {
            if (delivered) {
                // A copy held by an earlier failed attempt is now stale news;
                // left behind it would redeliver *after* the event that
                // superseded it. See [StickyEventBuffer.discard].
                buffer.discard(key, generation)
            } else if (buffer.hold(key, eventJson, generation)) {
                flush()
            }
        }
    }

    /** Redelivers held events, if JS looks able to take them now. */
    fun flush() {
        if (buffer.isEmpty()) return
        if (!canEmit()) return
        schedule(Runnable { deliverHeld() })
    }

    /**
     * Ends the current session, so nothing emitted for it can still be
     * redelivered to the next one.
     */
    fun endSession() {
        buffer.invalidateSession()
    }

    /**
     * Hands the held entries over, putting back whatever did not go out.
     *
     * The restore sits in a `finally` covering the entries never attempted as
     * well as those refused, because [buffer] has already given them up:
     * [StickyEventBuffer.drain] removes on read, so a throwable escaping this
     * loop would destroy exactly what the buffer exists to preserve. [emit]
     * catches its own exceptions, but it also builds a JNI-backed payload
     * outside that catch, and an `Error` would escape it regardless.
     */
    private fun deliverHeld() {
        val drained = buffer.drain()
        if (drained.isEmpty()) return
        val undelivered = mutableListOf<StickyEventBuffer.Entry>()
        var index = 0
        try {
            while (index < drained.size) {
                val entry = drained[index]
                if (!emit(entry.eventJson)) {
                    undelivered.add(entry)
                }
                index += 1
            }
        } finally {
            for (i in index until drained.size) {
                undelivered.add(drained[i])
            }
            buffer.restore(undelivered)
        }
    }
}
