package com.offlineprotocol.ble

import android.bluetooth.BluetoothAdapter
import org.junit.Assert.assertEquals
import org.junit.Test

/**
 * [AdapterStateTransition]: a Bluetooth stack crash on an Android 13 phone
 * reached the app only as ON -> TURNING_OFF, and the transport, which polled
 * the adapter once a minute, never noticed it.
 */
class AdapterStateTransitionTest {
    @Test
    fun `turning off and off are the radio going`() {
        assertEquals(AdapterStateTransition.RADIO_LOST, AdapterStateTransition.of(BluetoothAdapter.STATE_TURNING_OFF))
        assertEquals(AdapterStateTransition.RADIO_LOST, AdapterStateTransition.of(BluetoothAdapter.STATE_OFF))
    }

    @Test
    fun `on is the radio back`() {
        assertEquals(AdapterStateTransition.RADIO_BACK, AdapterStateTransition.of(BluetoothAdapter.STATE_ON))
    }

    @Test
    fun `turning on and unknown states wait`() {
        assertEquals(AdapterStateTransition.NONE, AdapterStateTransition.of(BluetoothAdapter.STATE_TURNING_ON))
        assertEquals(AdapterStateTransition.NONE, AdapterStateTransition.of(BluetoothAdapter.ERROR))
        // BLE_ON (15), the stack's LE-only state on the way down and up.
        assertEquals(AdapterStateTransition.NONE, AdapterStateTransition.of(15))
    }
}
