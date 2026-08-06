package com.offlineprotocol

import org.junit.Assert.assertEquals
import org.junit.Assert.assertFalse
import org.junit.Assert.assertTrue
import org.junit.Test
import java.util.concurrent.ConcurrentLinkedQueue
import java.util.concurrent.CountDownLatch

/**
 * Pins the redelivery semantics one-shot bridge events depend on: an event that
 * nothing will ever restate must survive a window where JS was not listening,
 * exactly once per hold, without a stale copy outliving a fresh one.
 *
 * No iOS twin — the Stop-action event this backs is Android-only.
 */
class StickyEventBufferTest {

    /** Holds for the buffer's current session, the way a live caller does. */
    private fun StickyEventBuffer.holdNow(key: String, eventJson: String): Boolean =
        hold(key, eventJson, currentGeneration())

    @Test
    fun heldEventIsReturnedByTheNextDrain() {
        // The core case: mesh_stopped_by_user emitted while JS had no
        // listeners, collected when a subscription finally arrives.
        val buffer = StickyEventBuffer()
        buffer.holdNow("mesh_stopped_by_user", """{"type":"mesh_stopped_by_user"}""")

        val drained = buffer.drain()

        assertEquals(1, drained.size)
        assertEquals("mesh_stopped_by_user", drained[0].key)
        assertEquals("""{"type":"mesh_stopped_by_user"}""", drained[0].eventJson)
    }

    @Test
    fun holdReportsThatItTookTheEvent() {
        // The caller flushes on a true return, so a false one here would be a
        // held event with nothing scheduled to collect it.
        val buffer = StickyEventBuffer()

        assertTrue(buffer.holdNow("mesh_stopped_by_user", "{}"))
    }

    @Test
    fun drainEmptiesTheBuffer() {
        // A delivered event must not re-fire on every later subscribe.
        val buffer = StickyEventBuffer()
        buffer.holdNow("mesh_stopped_by_user", "{}")

        buffer.drain()

        assertTrue(buffer.isEmpty())
        assertTrue(buffer.drain().isEmpty())
    }

    @Test
    fun emptyBufferStartsEmptyAndReportsIt() {
        // The flush path skips entirely on this, so it has to be right.
        val buffer = StickyEventBuffer()

        assertTrue(buffer.isEmpty())
        assertEquals(0, buffer.size)
    }

    @Test
    fun holdingTheSameKeyTwiceKeepsOnlyTheNewer() {
        // Two notification stops with no subscribe in between: the app
        // reconciles against a terminal state, so the older copy is noise.
        val buffer = StickyEventBuffer()
        buffer.holdNow("mesh_stopped_by_user", """{"seq":1}""")
        buffer.holdNow("mesh_stopped_by_user", """{"seq":2}""")

        val drained = buffer.drain()

        assertEquals(1, drained.size)
        assertEquals("""{"seq":2}""", drained[0].eventJson)
    }

    @Test
    fun distinctKeysDrainOldestFirst() {
        // A superseded relay session and a mesh stop can both be waiting; they
        // redeliver in the order they happened.
        val buffer = StickyEventBuffer()
        buffer.holdNow("internet_session_superseded", """{"a":1}""")
        buffer.holdNow("mesh_stopped_by_user", """{"b":2}""")

        val drained = buffer.drain()

        assertEquals(
            listOf("internet_session_superseded", "mesh_stopped_by_user"),
            drained.map { it.key }
        )
    }

    @Test
    fun reholdingAnExistingKeyMovesItToTheTail() {
        // Last-wins also means last-ordered: the refreshed key carries the
        // newest information, so it should not redeliver ahead of older news.
        val buffer = StickyEventBuffer()
        buffer.holdNow("first", "1")
        buffer.holdNow("second", "2")
        buffer.holdNow("first", "1-updated")

        val drained = buffer.drain()

        assertEquals(listOf("second", "first"), drained.map { it.key })
        assertEquals("1-updated", drained[1].eventJson)
    }

    @Test
    fun restorePutsBackAnEntryTheEmitRefused() {
        // The flush drained, the React instance went down mid-emit, nothing was
        // delivered — the event must still be waiting for the next trigger.
        val buffer = StickyEventBuffer()
        buffer.holdNow("mesh_stopped_by_user", """{"type":"mesh_stopped_by_user"}""")
        val drained = buffer.drain()

        buffer.restore(drained)

        assertFalse(buffer.isEmpty())
        assertEquals("""{"type":"mesh_stopped_by_user"}""", buffer.drain()[0].eventJson)
    }

    @Test
    fun restoreDoesNotOverwriteANewerEventForTheSameKey() {
        // The race the restore path exists to not lose: a fresh stop lands
        // while an undeliverable older copy is still in the flush's hands.
        // Restoring it blindly would resurrect stale state over the new one.
        val buffer = StickyEventBuffer()
        buffer.holdNow("mesh_stopped_by_user", """{"seq":1}""")
        val inFlight = buffer.drain()
        buffer.holdNow("mesh_stopped_by_user", """{"seq":2}""")

        buffer.restore(inFlight)

        val drained = buffer.drain()
        assertEquals(1, drained.size)
        assertEquals("""{"seq":2}""", drained[0].eventJson)
    }

    @Test
    fun restoredEntriesRedeliverAheadOfEventsThatLandedMidFlight() {
        // drain() promises oldest-first, and restore is the one path that could
        // break it: every restored entry was drained before anything now held
        // was taken, so appending would redeliver older news behind newer.
        val buffer = StickyEventBuffer()
        buffer.holdNow("internet_session_superseded", "older")
        val inFlight = buffer.drain()
        buffer.holdNow("mesh_stopped_by_user", "newer")

        buffer.restore(inFlight)

        assertEquals(
            listOf("internet_session_superseded", "mesh_stopped_by_user"),
            buffer.drain().map { it.key }
        )
    }

    @Test
    fun anOverflowingRestoreDropsTheRestoredEntriesBeforeTheNewerOnes() {
        // Eviction takes from the head, which after a head-first restore is the
        // restored (older) entry — the same "newest matters most" rule hold
        // applies, rather than a reversal smuggled in by the restore path.
        val buffer = StickyEventBuffer(maxEntries = 1)
        buffer.holdNow("older", "1")
        val inFlight = buffer.drain()
        buffer.holdNow("newer", "2")

        buffer.restore(inFlight)

        assertEquals(listOf("newer"), buffer.drain().map { it.key })
    }

    @Test
    fun restoringAnEmptyListLeavesTheBufferAlone() {
        // The common case — every entry delivered — must not disturb anything
        // that arrived since.
        val buffer = StickyEventBuffer()
        buffer.holdNow("live", "1")

        buffer.restore(emptyList())

        assertEquals(1, buffer.size)
    }

    @Test
    fun overflowEvictsTheOldestEntry() {
        // Backstop against a caller minting unbounded keys: the buffer fills
        // precisely when nobody is watching it.
        val buffer = StickyEventBuffer(maxEntries = 2)
        buffer.holdNow("a", "1")
        buffer.holdNow("b", "2")
        buffer.holdNow("c", "3")

        val drained = buffer.drain()

        assertEquals(listOf("b", "c"), drained.map { it.key })
    }

    @Test
    fun restoreRespectsTheCap() {
        // Restore is a second write path into the same map; it must not be the
        // one that lets the bound slip.
        val buffer = StickyEventBuffer(maxEntries = 2)
        val generation = buffer.currentGeneration()

        buffer.restore(
            listOf(
                StickyEventBuffer.Entry("a", "1", generation),
                StickyEventBuffer.Entry("b", "2", generation),
                StickyEventBuffer.Entry("c", "3", generation)
            )
        )

        assertEquals(2, buffer.size)
        assertEquals(listOf("b", "c"), buffer.drain().map { it.key })
    }

    @Test
    fun invalidateSessionDiscardsEverythingHeld() {
        // destroy() makes redelivery moot: the app tore the SDK down itself.
        val buffer = StickyEventBuffer()
        buffer.holdNow("mesh_stopped_by_user", "{}")

        buffer.invalidateSession()

        assertTrue(buffer.isEmpty())
    }

    @Test
    fun holdIsRefusedForASessionThatHasSinceEnded() {
        // The window that makes clearing alone insufficient: the teardown
        // thread reads the generation, spends the length of a full transport
        // shutdown emitting, and only then holds — by which time destroy() may
        // have run. Handing that stop to the next session would report a dead
        // mesh against a live one, with nothing to restate it.
        val buffer = StickyEventBuffer()
        val generation = buffer.currentGeneration()

        buffer.invalidateSession()

        assertFalse(buffer.hold("mesh_stopped_by_user", "{}", generation))
        assertTrue(buffer.isEmpty())
    }

    @Test
    fun restoreIsRefusedForEntriesFromASessionThatHasSinceEnded() {
        // The same race from the other side: a flush drains, destroy() runs,
        // every emit then fails and the flush tries to put its entries back.
        val buffer = StickyEventBuffer()
        buffer.holdNow("mesh_stopped_by_user", "{}")
        val inFlight = buffer.drain()

        buffer.invalidateSession()
        buffer.restore(inFlight)

        assertTrue(buffer.isEmpty())
    }

    @Test
    fun holdsForTheNewSessionAreAcceptedAfterInvalidation() {
        // Invalidation ends a session, it does not retire the buffer: a
        // create() after destroy() keeps the same module instance.
        val buffer = StickyEventBuffer()
        buffer.invalidateSession()

        assertTrue(buffer.holdNow("mesh_stopped_by_user", """{"seq":2}"""))
        assertEquals("""{"seq":2}""", buffer.drain()[0].eventJson)
    }

    @Test
    fun concurrentHoldsAndDrainsLoseNothingAndDuplicateNothing() {
        // The class exists because writers and readers are different threads
        // sharing no lock. Every held event must be drained exactly once.
        val holds = 200
        // Sized so nothing is ever evicted: this test is about the lock, not
        // the cap, and an eviction would make a missing entry ambiguous.
        val buffer = StickyEventBuffer(maxEntries = holds)
        val generation = buffer.currentGeneration()
        val drained = ConcurrentLinkedQueue<String>()
        val start = CountDownLatch(1)

        val producer = Thread {
            start.await()
            for (i in 0 until holds) {
                buffer.hold("key-$i", "$i", generation)
            }
        }
        val consumer = Thread {
            start.await()
            val deadlineNs = System.nanoTime() + TIMEOUT_MS * 1_000_000
            while (drained.size < holds && System.nanoTime() < deadlineNs) {
                buffer.drain().forEach { drained.add(it.eventJson) }
            }
        }

        producer.start()
        consumer.start()
        start.countDown()
        producer.join(TIMEOUT_MS)
        consumer.join(TIMEOUT_MS)

        assertEquals(holds, drained.size)
        assertEquals(holds, drained.toSet().size)
    }

    private companion object {
        const val TIMEOUT_MS = 10_000L
    }
}
