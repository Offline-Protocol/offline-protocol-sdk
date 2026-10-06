package com.offlineprotocol.ble

import android.bluetooth.BluetoothAdapter

/**
 * What one `BluetoothAdapter.ACTION_STATE_CHANGED` broadcast means for a
 * running transport. A Bluetooth stack crash reaches apps as the same
 * ON -> TURNING_OFF transition a user's toggle does, so both take the
 * radio-lost path.
 */
internal enum class AdapterStateTransition {
    /** Every link and registration this app held is gone or going. */
    RADIO_LOST,

    /** The adapter is on again: rebuild now, not on the recovery ladder's next rung. */
    RADIO_BACK,

    /** An intermediate state (turning on, LE-only); nothing to do yet. */
    NONE;

    companion object {
        fun of(adapterState: Int): AdapterStateTransition = when (adapterState) {
            BluetoothAdapter.STATE_TURNING_OFF, BluetoothAdapter.STATE_OFF -> RADIO_LOST
            BluetoothAdapter.STATE_ON -> RADIO_BACK
            else -> NONE
        }
    }
}
