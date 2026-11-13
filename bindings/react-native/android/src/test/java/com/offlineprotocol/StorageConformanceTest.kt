package com.offlineprotocol

import org.json.JSONException
import org.junit.Assert.assertEquals
import org.junit.Assert.assertFalse
import org.junit.Assert.assertThrows
import org.junit.Assert.assertTrue
import org.junit.Test

/**
 * The typed report [StorageConformance] decodes from the suite's JSON.
 *
 * Running the suite needs the Rust library, which a JVM unit test does not
 * load; the decoding is what can drift from the Rust report on this side.
 */
class StorageConformanceTest {
    /** A failure keeps its check and its detail, and the report is not green. */
    @Test
    fun aFailingReportDecodesWithItsChecks() {
        val report = StorageConformance.decode(
            """{"passed":["store_then_load"],"failures":[{"check":"values_are_bytes","detail":"loaded 3 bytes"}]}""",
        )

        assertEquals(listOf("store_then_load"), report.passed)
        assertEquals(listOf(StorageConformanceFailure("values_are_bytes", "loaded 3 bytes")), report.failures)
        assertFalse(report.isGreen)
    }

    @Test
    fun aReportWithPassesAndNoFailuresIsGreen() {
        val report = StorageConformance.decode("""{"passed":["store_then_load"],"failures":[]}""")

        assertTrue(report.isGreen)
    }

    /** A report with no failures and no passes ran nothing, so it is not green. */
    @Test
    fun anEmptyReportIsNotGreen() {
        assertFalse(StorageConformance.decode("""{"passed":[],"failures":[]}""").isGreen)
    }

    /** A key the suite renamed is an error, never a report with no failures. */
    @Test
    fun aRenamedKeyIsAnError() {
        assertThrows(JSONException::class.java) {
            StorageConformance.decode("""{"passed":["store_then_load"],"failed":[]}""")
        }
    }
}
