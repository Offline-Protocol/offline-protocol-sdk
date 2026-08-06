package com.offlineprotocol

import org.junit.Assert.assertEquals
import org.junit.Assert.assertFalse
import org.junit.Assert.assertTrue
import org.junit.Test

/**
 * Pins the redelivery semantics one-shot bridge events depend on: an event that
 * nothing will ever restate must survive a window where JS was not listening,
 * exactly once per hold, without a stale copy outliving a fresh one.
 *
 * No iOS twin — the Stop-action event this backs is Android-only.
 */
class StickyEventBufferTest {

    @Test
    fun heldEventIsReturnedByTheNextDrain() {
        // The core case: mesh_stopped_by_user emitted while JS had no
        // listeners, collected when a subscription finally arrives.
        val buffer = StickyEventBuffer()
        buffer.hold("mesh_stopped_by_user", """{"type":"mesh_stopped_by_user"}""")

        val drained = buffer.drain()

        assertEquals(1, drained.size)
        assertEquals("mesh_stopped_by_user", drained[0].key)
        assertEquals("""{"type":"mesh_stopped_by_user"}""", drained[0].eventJson)
    }

    @Test
    fun drainEmptiesTheBuffer() {
        // A delivered event must not re-fire on every later subscribe.
        val buffer = StickyEventBuffer()
        buffer.hold("mesh_stopped_by_user", "{}")

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
        buffer.hold("mesh_stopped_by_user", """{"seq":1}""")
        buffer.hold("mesh_stopped_by_user", """{"seq":2}""")

        val drained = buffer.drain()

        assertEquals(1, drained.size)
        assertEquals("""{"seq":2}""", drained[0].eventJson)
    }

    @Test
    fun distinctKeysDrainOldestFirst() {
        // A superseded relay session and a mesh stop can both be waiting; they
        // redeliver in the order they happened.
        val buffer = StickyEventBuffer()
        buffer.hold("internet_session_superseded", """{"a":1}""")
        buffer.hold("mesh_stopped_by_user", """{"b":2}""")

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
        buffer.hold("first", "1")
        buffer.hold("second", "2")
        buffer.hold("first", "1-updated")

        val drained = buffer.drain()

        assertEquals(listOf("second", "first"), drained.map { it.key })
        assertEquals("1-updated", drained[1].eventJson)
    }

    @Test
    fun restorePutsBackAnEntryTheEmitRefused() {
        // The flush drained, the React instance went down mid-emit, nothing was
        // delivered — the event must still be waiting for the next trigger.
        val buffer = StickyEventBuffer()
        buffer.hold("mesh_stopped_by_user", """{"type":"mesh_stopped_by_user"}""")
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
        buffer.hold("mesh_stopped_by_user", """{"seq":1}""")
        val inFlight = buffer.drain()
        buffer.hold("mesh_stopped_by_user", """{"seq":2}""")

        buffer.restore(inFlight)

        val drained = buffer.drain()
        assertEquals(1, drained.size)
        assertEquals("""{"seq":2}""", drained[0].eventJson)
    }

    @Test
    fun restoringAnEmptyListLeavesTheBufferAlone() {
        // The common case — every entry delivered — must not disturb anything
        // that arrived since.
        val buffer = StickyEventBuffer()
        buffer.hold("live", "1")

        buffer.restore(emptyList())

        assertEquals(1, buffer.size)
    }

    @Test
    fun overflowEvictsTheOldestEntry() {
        // Backstop against a caller minting unbounded keys: the buffer fills
        // precisely when nobody is watching it.
        val buffer = StickyEventBuffer(maxEntries = 2)
        buffer.hold("a", "1")
        buffer.hold("b", "2")
        buffer.hold("c", "3")

        val drained = buffer.drain()

        assertEquals(listOf("b", "c"), drained.map { it.key })
    }

    @Test
    fun restoreRespectsTheCap() {
        // Restore is a second write path into the same map; it must not be the
        // one that lets the bound slip.
        val buffer = StickyEventBuffer(maxEntries = 2)

        buffer.restore(
            listOf(
                StickyEventBuffer.Entry("a", "1"),
                StickyEventBuffer.Entry("b", "2"),
                StickyEventBuffer.Entry("c", "3")
            )
        )

        assertEquals(2, buffer.size)
        assertEquals(listOf("b", "c"), buffer.drain().map { it.key })
    }

    @Test
    fun clearDiscardsEverything() {
        // destroy() makes redelivery moot: the app tore the SDK down itself.
        val buffer = StickyEventBuffer()
        buffer.hold("mesh_stopped_by_user", "{}")

        buffer.clear()

        assertTrue(buffer.isEmpty())
    }
}
