package com.offlineprotocol

import org.junit.Assert.assertEquals
import org.junit.Assert.assertFalse
import org.junit.Assert.assertTrue
import org.junit.Test

/**
 * Pins the three orderings [StickyEventDispatcher] exists to get right. Each is
 * invisible in a diff and silent when it regresses, and none of them can be
 * reached through the module — `react-android` is `compileOnly` here — which is
 * why the dispatcher is a separate class at all.
 */
class StickyEventDispatcherTest {

    /**
     * A scriptable stand-in for the module's three seams. `emitResults` is
     * consumed one per emit, defaulting to false once exhausted.
     */
    private class Harness(
        var canEmit: Boolean = true,
        emitResults: List<Boolean> = emptyList(),
    ) {
        val buffer = StickyEventBuffer()
        val emitted = mutableListOf<String>()
        val scheduled = mutableListOf<Runnable>()
        var scheduleSucceeds: Boolean = true
        var canEmitCalls: Int = 0
        var onEmit: (() -> Unit)? = null

        /**
         * Runs after each gate read, so a test can land a subscribe or a
         * teardown in the window the dispatcher has to be correct across.
         */
        var onCanEmit: (() -> Unit)? = null
        private val results = emitResults.toMutableList()

        val dispatcher = StickyEventDispatcher(
            buffer = buffer,
            canEmit = {
                canEmitCalls += 1
                val answer = canEmit
                onCanEmit?.invoke()
                answer
            },
            emit = { json ->
                emitted.add(json)
                onEmit?.invoke()
                if (results.isEmpty()) false else results.removeAt(0)
            },
            schedule = { runnable ->
                if (scheduleSucceeds) {
                    scheduled.add(runnable)
                    true
                } else {
                    false
                }
            },
        )

        /** Runs every runnable the dispatcher posted, as the JS queue would. */
        fun runScheduled() {
            val pending = scheduled.toList()
            scheduled.clear()
            pending.forEach { it.run() }
        }
    }

    // MARK: - Ordering 1: the generation is read before the emit is attempted

    @Test
    fun anEventEmittedForASessionEndedMidEmitIsNotHeld() {
        // The window that makes clearing the buffer on destroy() insufficient:
        // the teardown thread reads the generation, spends the length of a full
        // transport shutdown emitting, and destroy() lands in between. Holding
        // it would hand a terminal mesh event to whichever session subscribes
        // next — the original bug with the sign flipped.
        val harness = Harness(canEmit = true)
        harness.onEmit = { harness.dispatcher.endSession() }

        harness.dispatcher.send("mesh_stopped_by_user", "{}")

        assertTrue("held an event from an ended session", harness.buffer.isEmpty())
        // A refused hold must not post a runnable either — nothing was held, so
        // there would be nothing for it to collect.
        assertTrue(harness.scheduled.isEmpty())
    }

    @Test
    fun anEventEmittedAfterTheSessionEndedIsNotHeldEither() {
        // The wider half of the same window, and the reason endSession() is not
        // just a generation bump. send() reads the generation when it attempts
        // the emit, so a destroy() that has *already* run is invisible to it —
        // it reads the current generation and the stamp has nothing to refuse.
        // The teardown that produces the event shares a lock with destroy()'s,
        // so emitting after destroy() completes is the ordinary interleaving,
        // and holding there hands a terminal mesh event to the next session.
        val harness = Harness(canEmit = false)
        harness.dispatcher.endSession()

        harness.dispatcher.send("mesh_stopped_by_user", "{}")

        assertTrue("held an event for a session that had already ended", harness.buffer.isEmpty())
        assertTrue(harness.scheduled.isEmpty())
    }

    @Test
    fun anEventEmittedAfterANewSessionBeginsIsHeld() {
        // Keeps the test above honest: ending a session must not retire the
        // buffer. start() reopens it, and everything after belongs to the
        // session it began.
        val harness = Harness(canEmit = false)
        harness.dispatcher.endSession()
        harness.dispatcher.beginSession()

        harness.dispatcher.send("mesh_stopped_by_user", "{}")

        assertFalse(harness.buffer.isEmpty())
    }

    // MARK: - Ordering 2: a successful hold re-runs the flush

    @Test
    fun aHoldRetriesTheFlushSoAGateThatOpenedMidEmitIsNotMissed() {
        // A subscribe can land between send()'s gate check and its hold: it
        // opens the gate, finds the buffer still empty, and so schedules
        // nothing. With the SDK subscribing once in its constructor the next
        // addListener never comes, so unless the hold itself re-runs the flush
        // the event waits for a foreground that may be hours away.
        //
        // Note there is no explicit flush() call below — the delivery is
        // entirely the hold's doing, which is the whole point.
        val harness = Harness(canEmit = false)
        harness.onCanEmit = { harness.canEmit = true }

        harness.dispatcher.send("mesh_stopped_by_user", """{"type":"mesh_stopped_by_user"}""")
        harness.runScheduled()

        assertEquals(listOf("""{"type":"mesh_stopped_by_user"}"""), harness.emitted)
    }

    // MARK: - Ordering 3: delivery hops the JS queue, never inline

    @Test
    fun flushNeverEmitsInline() {
        // NativeEventEmitter.addListener calls the native addListener *before*
        // it registers the JS-side listener, so an emit issued synchronously
        // from the flush would arrive at an emitter with nothing subscribed —
        // re-losing the event through a subtler version of the same hole.
        val harness = Harness(canEmit = false)
        harness.dispatcher.send("mesh_stopped_by_user", "{}")
        harness.canEmit = true
        harness.emitted.clear()

        harness.dispatcher.flush()

        assertTrue("flush emitted without hopping the queue", harness.emitted.isEmpty())
        assertEquals(1, harness.scheduled.size)
    }

    @Test
    fun aScheduleThatFailsLeavesTheEventHeld() {
        // runOnJSQueueThread returns false once the JS thread has finished. The
        // event has to survive that for the next trigger.
        val harness = Harness(canEmit = false)
        harness.dispatcher.send("mesh_stopped_by_user", "{}")
        harness.scheduleSucceeds = false
        harness.canEmit = true

        harness.dispatcher.flush()

        assertFalse(harness.buffer.isEmpty())
    }

    // MARK: - Delivery and restore

    @Test
    fun aDeliverableEventGoesStraightOutAndIsNeverHeld() {
        val harness = Harness(canEmit = true, emitResults = listOf(true))

        harness.dispatcher.send("mesh_stopped_by_user", """{"type":"mesh_stopped_by_user"}""")

        assertEquals(listOf("""{"type":"mesh_stopped_by_user"}"""), harness.emitted)
        assertTrue(harness.buffer.isEmpty())
        assertTrue(harness.scheduled.isEmpty())
    }

    @Test
    fun aThrowOnTheFirstEmitStillHoldsTheEvent() {
        // The twin of aThrowMidFlushRestoresEverythingItHadNotDelivered, on the
        // path that runs first. emit() catches its own exceptions in the module,
        // but it builds a JNI-backed payload outside that catch and an Error
        // escapes it regardless — and a throw treated as anything but a failed
        // attempt drops the one-shot event, which is the hole this class exists
        // to close.
        val harness = Harness(canEmit = true)
        harness.onEmit = { throw OutOfMemoryError("simulated") }

        try {
            harness.dispatcher.send("mesh_stopped_by_user", """{"type":"mesh_stopped_by_user"}""")
            throw AssertionError("expected the throwable to propagate")
        } catch (e: OutOfMemoryError) {
            // expected: the caller still learns something went wrong
        }

        assertEquals(
            listOf("""{"type":"mesh_stopped_by_user"}"""),
            harness.buffer.drain().map { it.eventJson }
        )
    }

    @Test
    fun aDeliveredEventDropsAStaleCopyHeldUnderTheSameKey() {
        // A held copy from an earlier failed attempt must not outlive the event
        // that superseded it: redelivered afterwards it would hand an app that
        // already reconciled the older news, with nothing to correct it.
        // canEmit starts false, so the first send short-circuits before emit
        // and the scripted `true` is still waiting for the second one.
        val harness = Harness(canEmit = false, emitResults = listOf(true))
        harness.dispatcher.send("mesh_stopped_by_user", """{"seq":1}""")
        assertFalse(harness.buffer.isEmpty())

        harness.canEmit = true
        harness.dispatcher.send("mesh_stopped_by_user", """{"seq":2}""")

        assertTrue("a stale copy outlived the event that superseded it", harness.buffer.isEmpty())
    }

    @Test
    fun aFlushThatThrowsDoesNotReplaceTheEmitThatFailed() {
        // The retry a successful hold runs reads the gate and posts to the JS
        // queue, either of which can throw — and out of a `finally` that second
        // throwable replaces the first, so the caller is told about the retry
        // instead of about the emit that actually failed. The original wins and
        // the retry's failure rides along suppressed.
        val harness = Harness(canEmit = true)
        harness.onEmit = { throw OutOfMemoryError("emit") }
        // Throws on the way back in through flush(), not on the first read.
        harness.onCanEmit = { if (harness.canEmitCalls > 1) throw IllegalStateException("flush") }

        try {
            harness.dispatcher.send("mesh_stopped_by_user", "{}")
            throw AssertionError("expected the emit failure to propagate")
        } catch (e: OutOfMemoryError) {
            assertEquals("emit", e.message)
            assertEquals(listOf("flush"), e.suppressed.map { it.message })
        }

        assertFalse("the event was lost with the throwable", harness.buffer.isEmpty())
    }

    // MARK: - Superseded inside a live session

    @Test
    fun discardDropsAHeldEventTheSessionItselfSuperseded() {
        // internet_session_superseded stops being an unrestated one-shot the
        // moment enableTransport('internet') clears the supersede latch: the
        // relay session it reported has been replaced by the one that call just
        // brought up. Redelivered afterwards it tells an app with a live socket
        // that it is connected elsewhere, with nothing to correct it. The
        // generation cannot see this — it is one transition inside one session.
        val harness = Harness(canEmit = false)
        harness.dispatcher.send("internet_session_superseded", "{}")
        assertFalse(harness.buffer.isEmpty())

        harness.dispatcher.discard("internet_session_superseded")

        assertTrue(harness.buffer.isEmpty())
    }

    @Test
    fun discardLeavesEveryOtherKeyHeld() {
        // Re-enabling the internet transport says nothing about a mesh stop.
        val harness = Harness(canEmit = false)
        harness.dispatcher.send("internet_session_superseded", "1")
        harness.dispatcher.send("mesh_stopped_by_user", "2")

        harness.dispatcher.discard("internet_session_superseded")

        assertEquals(listOf("mesh_stopped_by_user"), harness.buffer.drain().map { it.key })
    }

    @Test
    fun theGateIsCheckedBeforeThePayloadIsBuilt() {
        // canEmit guards emit rather than emit checking for itself, so a caller
        // whose emit builds a JNI-backed WritableMap does not build one it will
        // not use.
        val harness = Harness(canEmit = false)

        harness.dispatcher.send("mesh_stopped_by_user", "{}")

        assertTrue(harness.emitted.isEmpty())
    }

    @Test
    fun anEmitRefusedDuringTheFlushPutsTheEventBack() {
        val harness = Harness(canEmit = false)
        harness.dispatcher.send("mesh_stopped_by_user", "{}")
        harness.canEmit = true

        harness.dispatcher.flush()
        harness.runScheduled() // emit returns false: results list is empty

        assertFalse(harness.buffer.isEmpty())
    }

    @Test
    fun aThrowMidFlushRestoresEverythingItHadNotDelivered() {
        // drain() removes on read, so a throwable escaping the delivery loop
        // would destroy exactly what the buffer exists to preserve. emit()
        // catches its own exceptions in the module, but it also builds a
        // JNI-backed payload outside that catch, and an Error escapes it
        // regardless.
        val harness = Harness(canEmit = true, emitResults = listOf(true))
        harness.canEmit = false
        harness.dispatcher.send("a", "1")
        harness.dispatcher.send("b", "2")
        harness.dispatcher.send("c", "3")
        harness.canEmit = true
        harness.emitted.clear()
        harness.onEmit = { if (harness.emitted.size == 2) throw OutOfMemoryError("simulated") }

        harness.dispatcher.flush()
        try {
            harness.runScheduled()
        } catch (e: OutOfMemoryError) {
            // expected
        }

        // "a" was delivered; "b" threw and "c" was never attempted. Both are back.
        assertEquals(listOf("b", "c"), harness.buffer.drain().map { it.key })
    }

    @Test
    fun aSessionChangeUnderAnInFlightFlushStopsTheRestOfItGoingOut() {
        // drain() removes on read, so once a flush is running the buffer has
        // nothing left to refuse: without a per-entry generation check, a
        // start() or destroy() landing on the NativeModules thread while the JS
        // queue is partway through the batch bumps the generation and still
        // watches every remaining entry go out. That is the discard start()
        // documents silently not happening.
        val harness = Harness(canEmit = true, emitResults = listOf(true))
        harness.canEmit = false
        harness.dispatcher.send("a", "1")
        harness.dispatcher.send("b", "2")
        harness.canEmit = true
        harness.emitted.clear()
        harness.dispatcher.flush()
        harness.onEmit = { harness.dispatcher.beginSession() }

        harness.runScheduled()

        assertEquals("an entry from the ended session went out", listOf("1"), harness.emitted)
        // And "b" is dropped rather than restored: the session it belongs to is
        // over, so there is nobody left to redeliver it to.
        assertTrue(harness.buffer.isEmpty())
    }

    @Test
    fun endSessionDiscardsWhatWasWaiting() {
        val harness = Harness(canEmit = false)
        harness.dispatcher.send("mesh_stopped_by_user", "{}")

        harness.dispatcher.endSession()

        assertTrue(harness.buffer.isEmpty())
    }

    @Test
    fun flushOnAnEmptyBufferDoesNotEvenCheckTheGate() {
        // addListener and onHostResume both call flush unconditionally; the
        // common case must cost one lock acquisition, not a bridge round-trip.
        val harness = Harness(canEmit = true)

        harness.dispatcher.flush()

        assertEquals(0, harness.canEmitCalls)
        assertTrue(harness.scheduled.isEmpty())
    }
}
