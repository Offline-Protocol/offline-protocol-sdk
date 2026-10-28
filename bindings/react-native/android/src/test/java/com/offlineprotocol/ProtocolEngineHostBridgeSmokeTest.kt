package com.offlineprotocol

import android.content.Context
import org.junit.Assert.assertEquals
import org.junit.Assert.assertFalse
import org.junit.Assert.assertThrows
import org.junit.Test
import org.junit.runner.RunWith
import org.robolectric.RobolectricTestRunner
import org.robolectric.annotation.Config

/**
 * Runtime smoke for resolve-only shared bridge polls that every app hits before
 * MLS or Internet UI: they must return a boolean without throwing when the
 * engine or transport was never started.
 */
@RunWith(RobolectricTestRunner::class)
@Config(sdk = [34])
class ProtocolEngineHostBridgeSmokeTest {

    private fun emptyHost() = ProtocolEngineHost("bridge-smoke") { _, _, _ -> }

    @Test
    fun internetIsReady_isFalseWhenInternetManagerAbsent() {
        val host = emptyHost()
        assertEquals(false, ProtocolEngineHostBridge.internetIsReady(host, null, null))
    }

    @Test
    fun isMlsInitialized_isFalseWhenProtocolAbsent() {
        val host = emptyHost()
        assertEquals(false, ProtocolEngineHostBridge.isMlsInitialized(host, null, null))
    }

    @Test
    fun isRelay_isFalseWhenProtocolAbsent() {
        val host = emptyHost()
        assertEquals(false, ProtocolEngineHostBridge.isRelay(host, null, null))
    }

    // Empty message ids are dropped before the emptiness check, as the module
    // did before the shared bridge. Flutter's Android handler delegates here.
    @Test
    fun sendReadReceipt_rejectsAListOfOnlyEmptyIds() {
        val host = emptyHost()
        val error = assertThrows(IllegalArgumentException::class.java) {
            ProtocolEngineHostBridge.sendReadReceipt(
                host,
                null,
                mapOf("recipient" to "peer", "messageIds" to listOf("", "")),
            )
        }
        assertEquals("No valid message IDs provided", error.message)
    }

    @Test
    fun sendReadReceipt_keepsTheNonEmptyIds() {
        // Past the id check, the call stops at the missing engine.
        val host = emptyHost()
        val error = assertThrows(IllegalStateException::class.java) {
            ProtocolEngineHostBridge.sendReadReceipt(
                host,
                null,
                mapOf("recipient" to "peer", "messageIds" to listOf("", "m1")),
            )
        }
        assertEquals("Protocol not initialized", error.message)
    }

    @Test
    fun isBluetoothEnabled_reflectsAdapterStateWithContext() {
        val host = emptyHost()
        val context = org.robolectric.RuntimeEnvironment.getApplication() as Context
        val enabled =
            ProtocolEngineHostBridge.isBluetoothEnabled(host, context, null) as? Boolean
        assertFalse(enabled == true)
    }
}
