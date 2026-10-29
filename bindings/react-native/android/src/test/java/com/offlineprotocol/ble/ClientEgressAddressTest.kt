package com.offlineprotocol.ble

import org.junit.Assert.assertEquals
import org.junit.Test

/**
 * [resolveClientEgressAddress] picks the address a central writes a peer's
 * fragments to. The failure it exists for, reproduced on 2026-10-29: an iPhone
 * linked both ways to an Android phone restarted its app. Its hello had made
 * the address it connected to our server from the peer's address, and the new
 * process kept that link but not its subscription. Android held no client link
 * at that address, so it deferred every send as "not yet ready" for good,
 * beside a ready client link to the same iPhone, until the Android app
 * restarted.
 */
class ClientEgressAddressTest {
    @Test
    fun `a peer resolved to a server-only address is written over its ready client link`() {
        val ready = setOf("client")
        assertEquals(
            "client",
            resolveClientEgressAddress("server", listOf("server", "client")) { it in ready },
        )
    }

    @Test
    fun `a ready resolved address is kept`() {
        val ready = setOf("a", "b")
        assertEquals("b", resolveClientEgressAddress("b", listOf("a", "b")) { it in ready })
    }

    @Test
    fun `with no ready client link the resolved address is unchanged`() {
        // The caller still defers, or dials, at the address it resolved.
        assertEquals(
            "server",
            resolveClientEgressAddress("server", listOf("server", "dialing")) { false },
        )
    }
}
