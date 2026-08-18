package com.offlineprotocol

import org.junit.Assert.assertEquals
import org.junit.Assert.assertNotNull
import org.junit.Assert.assertNull
import org.junit.Test
import uniffi.offline_protocol.ProtocolException

class ProtocolErrorBridgeTest {

    @Test
    fun `typed protocol exceptions map to stable bridge codes`() {
        val cases: List<Pair<ProtocolException, String>> = listOf(
            ProtocolException.NoKeyPackage("bob") to "NoKeyPackage",
            ProtocolException.SessionNotReady("pending") to "SessionNotReady",
            ProtocolException.EncryptFailed("boom") to "EncryptFailed",
            ProtocolException.MediaTransferLimit("bob") to "MediaTransferLimit",
            ProtocolException.SendFailed("all transports failed") to "SendFailed",
            ProtocolException.InvalidState("cannot demote the last admin") to "InvalidState",
            // resolveUsername raises this while the protocol is stopped or
            // paused. Unmapped it fell to that method's fallback code,
            // InvalidArgument, which tells an app the name was unusable when
            // start() was all that was missing.
            ProtocolException.NotStarted("Protocol not started") to "NotStarted",
            // resolveUsername raises this for "discovery is off", where a retry
            // can never succeed, beside InvalidState for "retry shortly". One
            // code for both is the difference between an app that stops and one
            // that spins.
            ProtocolException.InvalidConfiguration("username discovery is disabled")
                to "InvalidConfiguration",
            ProtocolException.MlsNotInitialized("MLS not initialized") to "MlsNotInitialized",
            ProtocolException.TransportException("ble unavailable") to "TransportError",
            ProtocolException.SerializationException("bad json") to "SerializationError",
            ProtocolException.ServiceException("no provider") to "ServiceError",
            ProtocolException.GroupNotFound("group:missing") to "GroupNotFound",
            ProtocolException.PermissionDenied("only admins can invite") to "PermissionDenied",
            ProtocolException.InvalidArgument("group name cannot be empty") to "InvalidArgument"
        )
        for ((error, expectedCode) in cases) {
            val mapped = mapProtocolBridgeError(error)
            assertNotNull("expected a mapping for $expectedCode", mapped)
            assertEquals(expectedCode, mapped!!.code)
        }
    }

    @Test
    fun `message-carrying variants pass the engine message through`() {
        val mapped = mapProtocolBridgeError(
            ProtocolException.GroupNotFound("Group not found: group:x")
        )
        assertEquals("Group not found: group:x", mapped?.message)
    }

    @Test
    fun `unmapped errors return null so callers keep their legacy code`() {
        assertNull(mapProtocolBridgeError(ProtocolException.Other("misc")))
        assertNull(mapProtocolBridgeError(IllegalStateException("Protocol not initialized")))
    }
}
