package com.offlineprotocol

import org.junit.Assert.assertEquals
import org.junit.Test

/**
 * [stopsBeforeEnable]: an enable stops a transport that is RUNNING or STARTING
 * before it starts it again. The Wi-Fi Direct branch used to stop only a
 * RUNNING manager, so one left in STARTING by a start that never finished was
 * started over its own channel, receiver and listener; the Reticulum and Nostr
 * branches did too, so an enable during a connect failed with `AlreadyRunning`.
 *
 * This pins the predicate. That every restarting branch calls it is pinned on
 * the Rust side, by
 * `react_native_android_every_enable_stops_a_starting_manager`, which reads
 * `ProtocolEngineHost.kt`.
 */
class TransportEnableRestartTest {

    @Test
    fun `an enable stops a running or starting transport and nothing else`() {
        // A set: the rule is which states, not the order the enum declares them in.
        val stops = TransportState.values().filter(::stopsBeforeEnable).toSet()
        assertEquals(setOf(TransportState.STARTING, TransportState.RUNNING), stops)
    }
}
