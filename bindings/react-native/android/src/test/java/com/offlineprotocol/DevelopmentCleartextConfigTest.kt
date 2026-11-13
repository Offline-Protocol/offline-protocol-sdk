package com.offlineprotocol

import org.junit.Assert.assertEquals
import org.junit.Assert.assertFalse
import org.junit.Assert.assertTrue
import org.junit.Test
import uniffi.offline_protocol.ProtocolException

class DevelopmentCleartextConfigTest {
    @Test
    fun developmentCleartextMatchesReactNativePreset() {
        val config = OfflineProtocolDevelopmentCleartextConfig.make("app", "user")
        assertTrue(config.bleEnabled)
        assertFalse(config.wifiDirectEnabled)
        assertFalse(config.internetEnabled)
        assertFalse(config.encryptionEnabled)
        assertFalse(config.requireEncryption)
    }

    @Test
    fun encryptFailedDiagnosticsFireOnce() {
        OfflineProtocolEncryptFailedDiagnostics.resetForTests()
        val sink = mutableListOf<String>()
        val error = ProtocolException.EncryptFailed("boom")
        OfflineProtocolEncryptFailedDiagnostics.logFirstRemedyIfNeeded("sendMessage", error, sink::add)
        OfflineProtocolEncryptFailedDiagnostics.logFirstRemedyIfNeeded("sendMessage", error, sink::add)
        assertEquals(1, sink.size)
        assertTrue(sink[0].contains("OfflineProtocolDevelopmentCleartextConfig"))
    }
}
