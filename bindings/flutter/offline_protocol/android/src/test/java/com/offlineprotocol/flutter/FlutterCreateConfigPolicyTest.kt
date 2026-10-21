package com.offlineprotocol.flutter

import org.junit.Assert.assertEquals
import org.junit.Assert.fail
import org.junit.Test

class FlutterCreateConfigPolicyTest {

    @Test
    fun createConfig_rejectsReticulumFlag() {
        try {
            FlutterCreateConfigPolicy.requireCreateConfigSupported(
                """{"appId":"a","profile":"p","reticulumEnabled":true}"""
            )
            fail("expected rejection")
        } catch (e: IllegalArgumentException) {
            assertEquals(FlutterCreateConfigPolicy.UNSUPPORTED_TRANSPORT_MESSAGE, e.message)
        }
    }

    @Test
    fun enableTransport_rejectsNostr() {
        try {
            FlutterCreateConfigPolicy.requireTransportTypeSupported("nostr")
            fail("expected rejection")
        } catch (e: IllegalArgumentException) {
            assertEquals(FlutterCreateConfigPolicy.UNSUPPORTED_TRANSPORT_MESSAGE, e.message)
        }
    }
}
