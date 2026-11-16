package com.offlineprotocol

import org.junit.Assert.assertEquals
import org.junit.Assert.assertFalse
import org.junit.Assert.assertTrue
import org.junit.Test
import uniffi.offline_protocol.OverflowPolicy
import uniffi.offline_protocol.ProtocolException

class DevelopmentCleartextConfigTest {
    @Test
    fun developmentCleartextMatchesReactNativePreset() {
        val config = OfflineProtocolDevelopmentCleartextConfig.make("app", "user")
        assertTrue(config.bleEnabled)
        assertFalse(config.wifiDirectEnabled)
        assertFalse(config.internetEnabled)
        assertFalse(config.encryptionEnabled)
        assertFalse(config.autoKeyExchange)
        assertFalse(config.storePending)
        assertFalse(config.requireEncryption)
        assertFalse(config.reticulumEnabled)
        assertFalse(config.nostrEnabled)
        assertFalse(config.preferOnline)
        assertEquals(8u.toUByte(), config.initialTtl)
        assertEquals(64uL, config.maxPendingPerPeer)
        assertEquals(4096uL, config.maxPendingGlobal)
        assertEquals(86_400_000uL, config.pendingTtlMs)
        assertEquals(OverflowPolicy.DROP_OLDEST, config.overflowPolicy)
    }

    @Test
    fun encryptFailedDiagnosticsFireOnce() {
        OfflineProtocolEncryptFailedDiagnostics.resetForTests()
        val sink = mutableListOf<String>()
        val error = ProtocolException.EncryptFailed("boom")
        // MLS initialized: the error has another cause, so nothing is logged
        // and the once-guard is not spent.
        OfflineProtocolEncryptFailedDiagnostics.logFirstRemedyIfNeeded("sendMessage", error, true, sink::add)
        assertEquals(0, sink.size)
        OfflineProtocolEncryptFailedDiagnostics.logFirstRemedyIfNeeded("sendMessage", error, false, sink::add)
        OfflineProtocolEncryptFailedDiagnostics.logFirstRemedyIfNeeded("sendMessage", error, false, sink::add)
        assertEquals(1, sink.size)
        assertTrue(sink[0].contains("OfflineProtocolDevelopmentCleartextConfig"))
        assertTrue(sink[0].contains("OfflineProtocolReferenceStorage.initializeMls"))
    }

    @Test
    fun onlyEncryptFailedIsClassifiedAsEncryptFailed() {
        assertTrue(OfflineProtocolEncryptFailedDiagnostics.isEncryptFailed(ProtocolException.EncryptFailed("x")))
        assertFalse(OfflineProtocolEncryptFailedDiagnostics.isEncryptFailed(ProtocolException.InvalidArgument("x")))
        assertFalse(OfflineProtocolEncryptFailedDiagnostics.isEncryptFailed(IllegalStateException("EncryptFailed")))
    }
}
