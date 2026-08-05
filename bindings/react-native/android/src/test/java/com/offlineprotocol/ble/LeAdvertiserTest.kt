package com.offlineprotocol.ble

import android.bluetooth.BluetoothManager
import android.bluetooth.le.AdvertiseData
import android.content.Context
import android.os.Handler
import android.os.Looper
import org.junit.Assert.assertEquals
import org.junit.Test
import org.junit.runner.RunWith
import org.robolectric.RobolectricTestRunner
import org.robolectric.RuntimeEnvironment
import org.robolectric.annotation.Config

/**
 * Pins what [LeAdvertiser.start] must do when it has no platform advertiser to
 * start against — the state this class is in whenever the user has Bluetooth
 * switched off, since `getBluetoothLeAdvertiser()` returns null for exactly
 * that reason and the facade's adapter-reset path re-attaches whatever it
 * reads.
 *
 * Two properties, both invisible in the source once written and both permanent
 * if broken:
 *
 *  1. The in-flight gate must stay *down*. It is raised immediately before the
 *     platform call and otherwise lowered only by `stop()` or a terminal
 *     `onStartFailure` — neither of which runs when there is no call to make.
 *     Raising it against a null advertiser therefore wedges advertising off for
 *     the rest of the process lifetime.
 *  2. The deferral must still be latched. The null check sits *below* the
 *     GATT-readiness check precisely so an advertiser attached while the
 *     service registration is still in flight is picked up by
 *     [LeAdvertiser.onGattServerReady] rather than dropped on the floor.
 *
 * Neither is directly observable — `startInFlight` and `pendingAdvertiseReason`
 * are private, and Robolectric's shadow advertiser accepts `startAdvertising`
 * without invoking the callback, so `isAdvertising` never flips. Both tests
 * therefore observe the same downstream tell: `stop()` emits "Stopped BLE
 * advertising" only when a start actually reached the platform call and left an
 * `advertiseCallback` behind. A start that was swallowed by a wedged gate, or
 * never triggered because the deferral was dropped, leaves that emission out.
 */
@RunWith(RobolectricTestRunner::class)
@Config(sdk = [34])
class LeAdvertiserTest {

    private class FakeHost(var gattReady: Boolean) : LeAdvertiser.Host {
        override fun isGattServerReady(): Boolean = gattReady
        override fun buildAdvertiseData(): AdvertiseData = AdvertiseData.Builder().build()
        override fun buildScanResponse(): AdvertiseData = AdvertiseData.Builder().build()
        override fun refreshPublishedIdentity() {}
        // Defeat throttling so every admitted call is individually observable.
        override fun shouldLog(key: String, intervalMs: Long): Boolean = true
    }

    private val seen = mutableListOf<String>()
    private val host = FakeHost(gattReady = true)
    private val advertiser = LeAdvertiser(
        mainHandler = Handler(Looper.getMainLooper()),
        host = host,
        diagnosticEmitter = { _, message, _ -> seen.add(message) },
    )

    /** A real (Robolectric-shadowed) platform advertiser to attach. */
    private fun platformAdvertiser() =
        ((RuntimeEnvironment.getApplication() as Context)
            .getSystemService(Context.BLUETOOTH_SERVICE) as BluetoothManager)
            .adapter!!
            .bluetoothLeAdvertiser

    @Test
    fun `start against a null advertiser leaves the in-flight gate down`() {
        host.gattReady = true

        // Bluetooth off: nothing attached, so this start has nothing to do.
        advertiser.start("while-adapter-off")

        // Adapter comes back and the facade re-attaches. This start must be
        // admitted — if the previous one had raised the gate, it would return
        // at `if (startInFlight) return` and advertising would be wedged off
        // for good.
        advertiser.attachAdvertiser(platformAdvertiser())
        advertiser.start("after-adapter-recovery")

        advertiser.stop()
        assertEquals(listOf("Stopped BLE advertising"), seen)
    }

    @Test
    fun `a null advertiser still latches the deferral for onGattServerReady`() {
        // GATT registration in flight *and* no advertiser yet — the null check
        // must not short-circuit the deferral that the GATT branch sets up.
        host.gattReady = false
        advertiser.start("deferred-while-adapter-off")
        assertEquals(listOf("Waiting for GATT service registration"), seen)

        // Service registration lands after the adapter came back. The latched
        // reason is what makes this start happen at all.
        advertiser.attachAdvertiser(platformAdvertiser())
        host.gattReady = true
        advertiser.onGattServerReady()

        advertiser.stop()
        assertEquals(
            listOf("Waiting for GATT service registration", "Stopped BLE advertising"),
            seen,
        )
    }

    @Test
    fun `onGattServerReady is inert when no start was ever deferred`() {
        // Guards the negative half of the test above: the "Stopped BLE
        // advertising" tell has to come from the latched deferral, not from
        // onGattServerReady starting unconditionally.
        host.gattReady = true
        advertiser.attachAdvertiser(platformAdvertiser())

        advertiser.onGattServerReady()

        advertiser.stop()
        assertEquals(emptyList<String>(), seen)
    }
}
