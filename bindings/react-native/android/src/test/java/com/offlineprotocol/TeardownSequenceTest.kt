package com.offlineprotocol

import org.junit.Assert.assertEquals
import org.junit.Assert.assertNull
import org.junit.Assert.assertSame
import org.junit.Assert.assertTrue
import org.junit.Assert.fail
import org.junit.Test

/**
 * Pins the one property the mesh teardown depends on: a step that throws must
 * not decide whether the steps after it run.
 *
 * The teardown this backs stops five transports, the keep-alive service and the
 * protocol core. Before this helper the first throw skipped everything after it
 * — and since BLE is stopped first, and its stop reaches `stopScan` on an
 * adapter the user may have just switched off, that meant the remaining
 * transports kept running while the notification came down and JS was told the
 * mesh was off.
 */
class TeardownSequenceTest {

    @Test
    fun `a throwing step does not stop the ones after it`() {
        val ran = mutableListOf<String>()
        val teardown = TeardownSequence()

        teardown.step("scheduler") { ran.add("scheduler") }
        teardown.step("ble") {
            ran.add("ble")
            throw IllegalStateException("BT adapter is off")
        }
        teardown.step("internet") { ran.add("internet") }
        teardown.step("keep-alive") { ran.add("keep-alive") }

        assertEquals(listOf("scheduler", "ble", "internet", "keep-alive"), ran)
    }

    @Test
    fun `failures are collected in order and named by their step`() {
        val teardown = TeardownSequence()
        val bleFailure = IllegalStateException("BT adapter is off")
        val nostrFailure = RuntimeException("relay socket already closed")

        teardown.step("ble") { throw bleFailure }
        teardown.step("internet") { }
        teardown.step("nostr") { throw nostrFailure }

        assertEquals(2, teardown.failures.size)
        assertEquals("ble", teardown.failures[0].step)
        assertSame(bleFailure, teardown.failures[0].cause)
        assertEquals("nostr", teardown.failures[1].step)
        assertSame(nostrFailure, teardown.failures[1].cause)
    }

    @Test
    fun `the first failure is the one a caller rethrows`() {
        val teardown = TeardownSequence()
        val first = IllegalStateException("first")

        teardown.step("ble") { throw first }
        teardown.step("nostr") { throw RuntimeException("second") }

        // stop() rejects with this, so it must stay the exception that would
        // have propagated before — later failures must not displace it.
        assertSame(first, teardown.firstFailureOrNull())
    }

    @Test
    fun `a clean sequence reports nothing`() {
        val teardown = TeardownSequence()

        teardown.step("ble") { }
        teardown.step("internet") { }

        assertTrue(teardown.failures.isEmpty())
        assertNull(teardown.firstFailureOrNull())
    }

    @Test
    fun `an Error is not treated as a failed step`() {
        val teardown = TeardownSequence()

        // OutOfMemory/StackOverflow mean the process is already in trouble;
        // pressing on with teardown would be the wrong call, so Errors are not
        // swallowed the way a transport's Exception is.
        try {
            teardown.step("ble") { throw OutOfMemoryError("heap") }
            fail("an Error must propagate rather than be recorded")
        } catch (expected: OutOfMemoryError) {
            assertEquals("heap", expected.message)
        }

        assertTrue(teardown.failures.isEmpty())
    }
}
