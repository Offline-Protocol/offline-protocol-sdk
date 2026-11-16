package com.offlineprotocol

import org.junit.Assert.assertEquals
import org.junit.Assert.assertFalse
import org.junit.Assert.assertTrue
import org.junit.Test

/**
 * The hints mirror `lifecycleHints.ts`: each fires only on a real ordering
 * mistake, once per key, and nothing at all when disabled (how React Native
 * runs the host). Plain JUnit: the hints write to a sink, never `android.util.Log`.
 */
class IntegratorLifecycleHintsTest {
    private val messages = mutableListOf<String>()
    private fun hints(enabled: Boolean = true) = IntegratorLifecycleHints(enabled) { messages.add(it) }

    @Test
    fun disabledHintsSayNothing() {
        val h = hints(enabled = false)
        h.callBeforeEngineStart("sendMessage", engineRunning = false)
        h.startWithoutMls(encryptionEnabled = true, mlsReady = false)
        h.sendWithoutMls("sendMessage", encryptionEnabled = true, mlsReady = false, engineRunning = true)
        h.dataStorageUnavailable("dataCreateDoc")
        h.interestApplied("notes", listOf("a"), engineRunning = true)
        assertEquals(emptyList<String>(), messages)
    }

    @Test
    fun engineHintFiresOncePerContextAndNeverWhileRunning() {
        val h = hints()
        h.callBeforeEngineStart("sendMessage", engineRunning = true)
        assertTrue(messages.isEmpty())
        h.callBeforeEngineStart("sendMessage", engineRunning = false)
        h.callBeforeEngineStart("sendMessage", engineRunning = false)
        h.callBeforeEngineStart("enableTransport(ble)", engineRunning = false)
        assertEquals(2, messages.size)
        assertTrue(messages[0].contains("sendMessage: call start() before this API"))
        assertTrue(messages[1].contains("enableTransport(ble)"))
    }

    @Test
    fun mlsHintsNeedEncryptionAndAMissingMls() {
        val h = hints()
        h.startWithoutMls(encryptionEnabled = false, mlsReady = false)
        h.startWithoutMls(encryptionEnabled = true, mlsReady = true)
        h.sendWithoutMls("sendMessage", encryptionEnabled = true, mlsReady = false, engineRunning = false)
        h.sendWithoutMls("sendMessage", encryptionEnabled = true, mlsReady = true, engineRunning = true)
        h.sendWithoutMls("sendMessage", encryptionEnabled = false, mlsReady = false, engineRunning = true)
        assertTrue(messages.isEmpty())
        h.startWithoutMls(encryptionEnabled = true, mlsReady = false)
        h.sendWithoutMls("sendMessage", encryptionEnabled = true, mlsReady = false, engineRunning = true)
        assertEquals(2, messages.size)
        assertTrue(messages[0].startsWith("[OfflineProtocol] start() with encryption enabled"))
        assertTrue(messages[1].contains("sendMessage: encryption is enabled but MLS is not initialized"))
    }

    @Test
    fun dataHintNamesTheRefusedCallOnce() {
        val h = hints()
        h.dataStorageUnavailable("dataCreateDoc")
        h.dataStorageUnavailable("dataCreateDoc")
        h.dataStorageUnavailable("dataMapSet")
        assertEquals(2, messages.size)
        assertTrue(messages[0].contains("dataCreateDoc: the data layer has no storage yet"))
        assertTrue(messages[1].contains("dataMapSet"))
    }

    @Test
    fun interestWarnsOnlyOnAPostStartNarrowing() {
        val h = hints()
        // Before start: a narrowing is the recommended order.
        h.interestApplied("notes", listOf("a*"), engineRunning = false)
        assertTrue(messages.isEmpty())
        // After start, widening is the supported use.
        h.interestApplied("notes", listOf("a*", "b*"), engineRunning = true)
        h.interestApplied("notes", listOf("*"), engineRunning = true)
        assertTrue(messages.isEmpty())
        h.interestApplied("notes", listOf("a*"), engineRunning = true)
        h.interestApplied("notes", listOf("*"), engineRunning = true)
        h.interestApplied("notes", listOf("a1"), engineRunning = true)
        assertEquals(1, messages.size)
        assertTrue(messages[0].contains("setInterest(\"notes\") after start() narrowed"))
        // A space with no interest yet has the default ["*"].
        h.interestApplied("photos", listOf("x"), engineRunning = true)
        assertEquals(2, messages.size)
    }

    @Test
    fun resetForgetsWarnedKeysAndAppliedInterest() {
        val h = hints()
        h.callBeforeEngineStart("sendMessage", engineRunning = false)
        h.interestApplied("notes", listOf("a*"), engineRunning = false)
        h.reset()
        h.callBeforeEngineStart("sendMessage", engineRunning = false)
        // After reset the space is back to ["*"], so "a*" narrows again.
        h.interestApplied("notes", listOf("a*"), engineRunning = true)
        assertEquals(3, messages.size)
    }

    @Test
    fun narrowingMatchesTheTypeScriptRule() {
        val n = { previous: List<String>, next: List<String> ->
            IntegratorLifecycleHints.isInterestNarrowing(previous, next)
        }
        assertFalse(n(listOf("*"), listOf("*")))
        assertFalse(n(listOf("a*"), listOf("*")))
        assertFalse(n(listOf("a1"), listOf("a*")))
        assertFalse(n(listOf("ab*"), listOf("a*")))
        assertTrue(n(listOf("*"), listOf("a*")))
        assertTrue(n(listOf("a*"), listOf("a1")))
        assertTrue(n(listOf("a*"), listOf("ab*", "a")))
        assertTrue(n(listOf("a1", "b1"), listOf("a1")))
        assertFalse(n(emptyList(), listOf("a")))
    }
}
