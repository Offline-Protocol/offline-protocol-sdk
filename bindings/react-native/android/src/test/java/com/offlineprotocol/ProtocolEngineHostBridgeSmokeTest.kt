package com.offlineprotocol

import android.content.Context
import org.junit.Assert.assertEquals
import org.junit.Assert.assertFalse
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

    @Test
    fun isBluetoothEnabled_reflectsAdapterStateWithContext() {
        val host = emptyHost()
        val context = org.robolectric.RuntimeEnvironment.getApplication() as Context
        val enabled =
            ProtocolEngineHostBridge.isBluetoothEnabled(host, context, null) as? Boolean
        assertFalse(enabled == true)
    }
}
