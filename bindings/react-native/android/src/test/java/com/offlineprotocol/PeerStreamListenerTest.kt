package com.offlineprotocol

import org.junit.Assert.assertEquals
import org.junit.Assert.assertFalse
import org.junit.Assert.assertNotEquals
import org.junit.Assert.assertTrue
import org.junit.Test
import java.net.ServerSocket

/**
 * [PeerStreamListener]: a second SDK app on the phone listens on a port of its
 * own and knows it does not hold the one a group's clients dial.
 */
class PeerStreamListenerTest {

    @Test
    fun `the first listener takes the group port`() {
        // A port that was free a moment ago; nothing else in the test binds it.
        val port = ServerSocket(0).use { it.localPort }
        val first = PeerStreamListener.bind(port)!!
        first.socket.use {
            assertTrue(first.ownsGroupPort)
            assertEquals(port, it.localPort)
        }
    }

    @Test
    fun `a second listener takes another port and does not claim the group port`() {
        ServerSocket(0).use { held ->
            val second = PeerStreamListener.bind(held.localPort)!!
            second.socket.use {
                assertFalse(second.ownsGroupPort)
                assertNotEquals(held.localPort, it.localPort)
                assertNotEquals(0, it.localPort)
            }
        }
    }
}
