package com.offlineprotocol

import org.junit.Assert.assertEquals
import org.junit.Test

/**
 * [stopsBeforeEnable]: an enable stops a transport that is RUNNING or STARTING
 * before it starts it again. The Wi-Fi Direct branch used to stop only a
 * RUNNING manager, so one left in STARTING by a start that never finished was
 * started over its own channel, receiver and listener.
 */
class TransportEnableRestartTest {

    @Test
    fun `an enable stops a running or starting transport and nothing else`() {
        val stops = TransportState.values().filter(::stopsBeforeEnable)
        assertEquals(listOf(TransportState.STARTING, TransportState.RUNNING), stops)
    }
}
