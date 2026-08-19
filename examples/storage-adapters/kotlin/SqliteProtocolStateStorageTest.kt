package com.offlineprotocol.examples

import androidx.test.core.app.ApplicationProvider
import org.json.JSONObject
import org.junit.After
import org.junit.Assert.assertTrue
import org.junit.Test
import org.junit.runner.RunWith
import org.robolectric.RobolectricTestRunner
import org.robolectric.annotation.Config
import uniffi.offline_protocol.runStorageConformance

/**
 * The gate every storage adapter must pass.
 *
 * One suite, defined once in Rust and reachable from every binding, so an
 * adapter written here is held to exactly the same contract as one written
 * in Swift or Python.
 */
@RunWith(RobolectricTestRunner::class)
@Config(sdk = [24])
class SqliteProtocolStateStorageTest {

    private lateinit var storage: SqliteProtocolStateStorage

    @After
    fun tearDown() {
        if (this::storage.isInitialized) storage.close()
    }

    @Test
    fun `passes the storage conformance suite`() {
        storage = SqliteProtocolStateStorage(ApplicationProvider.getApplicationContext())

        val report = JSONObject(runStorageConformance(storage))
        val failures = report.getJSONArray("failures")
        val detail = buildString {
            for (index in 0 until failures.length()) {
                val failure = failures.getJSONObject(index)
                append(failure.getString("check"))
                append(": ")
                append(failure.getString("detail"))
                append('\n')
            }
        }
        assertTrue("conformance failures:\n$detail", failures.length() == 0)
        assertTrue("the suite reported no checks", report.getJSONArray("passed").length() > 0)
    }
}
