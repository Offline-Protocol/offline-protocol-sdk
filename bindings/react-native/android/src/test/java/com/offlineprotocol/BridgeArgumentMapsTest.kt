package com.offlineprotocol

import com.facebook.react.bridge.JavaOnlyArray
import com.facebook.react.bridge.JavaOnlyMap
import org.junit.Assert.assertEquals
import org.junit.Assert.assertNull
import org.junit.Assert.assertThrows
import org.junit.Test

/**
 * A JavaScript array reaches the shared bridge with each element as its own
 * type. The adapter declares every array parameter `ReadableArray` with no
 * element type, so a conversion that reads every element as one type is wrong
 * for some method: `getInt` made `sendReadReceipt`, whose ids are strings,
 * throw on every call in 0.31.0.
 *
 * Needs React Native's classes at test runtime, so the Android library leaves
 * it out (`reactOnlyTests` in its build file).
 */
class BridgeArgumentMapsTest {

    @Test
    fun everyElementKeepsItsOwnType() {
        val nested = JavaOnlyArray.of(1, "x")
        val map = JavaOnlyMap.of("k", "v")
        val array = JavaOnlyArray.of("id-1", 7, 1.5, true, null, nested, map)

        assertEquals(
            listOf("id-1", 7.0, 1.5, true, null, listOf(1.0, "x"), mapOf("k" to "v")),
            BridgeResults.readableArrayToList(array),
        )
    }

    @Test
    fun aTopLevelArrayAndOneInsideAMapConvertAlike() {
        val bytes = listOf(0, 127, 255)
        val topLevel = BridgeResults.readableArrayToList(JavaOnlyArray.from(bytes))
        val insideMap = BridgeResults.readableMapToArgumentMap(
            JavaOnlyMap.of("data", JavaOnlyArray.from(bytes)),
        )["data"]

        assertEquals(listOf(0.0, 127.0, 255.0), topLevel)
        assertEquals(topLevel, insideMap)
    }

    @Test
    fun aMissingArrayStaysMissing() {
        assertNull(BridgeResults.readableArrayToList(null))
    }

    @Test
    fun readReceiptIdsArriveAsStrings() {
        // With the ids readable, the call gets past argument checks and stops
        // at the missing engine. Under getInt it threw on the first id.
        val host = ProtocolEngineHost("bridge-args") { _, _, _ -> }
        val arguments = mapOf(
            "recipient" to "peer",
            "messageIds" to BridgeResults.readableArrayToList(JavaOnlyArray.of("m1", "m2")),
        )

        val error = assertThrows(IllegalStateException::class.java) {
            ProtocolEngineHostBridge.sendReadReceipt(host, null, arguments)
        }
        assertEquals("Protocol not initialized", error.message)
    }
}
