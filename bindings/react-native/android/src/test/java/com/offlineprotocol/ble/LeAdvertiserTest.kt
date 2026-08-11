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
 * Pins the gate behaviour of [LeAdvertiser.start] on the two paths that decide
 * whether a device is discoverable at all after the user toggles Bluetooth: a
 * start with no platform advertiser attached (`getBluetoothLeAdvertiser()`
 * returns null while the adapter is off, and the facade's adapter-reset path
 * re-attaches whatever it reads), and a start that follows an adapter-off which
 * left the in-flight gate raised over an advertisement the platform has already
 * torn down.
 *
 * Three properties, all invisible in the source once written and all permanent
 * if broken:
 *
 *  1. A null advertiser must leave the in-flight gate *down*. It is raised
 *     immediately before the platform call and otherwise lowered only by
 *     `stop()` or a terminal `onStartFailure` — neither of which runs when
 *     there is no call to make. Raising it against a null advertiser wedges
 *     advertising off for the rest of the process lifetime.
 *  2. A null advertiser must still latch the deferral. The null check sits
 *     *below* the GATT-readiness check precisely so an advertiser attached
 *     while the service registration is still in flight is picked up by
 *     [LeAdvertiser.onGattServerReady] rather than dropped on the floor.
 *  3. After a successful start, the gate stays raised until an explicit
 *     `stop()` — including across an adapter-off, which the platform performs
 *     without any callback. This is why the facade's recovery runnable stops
 *     before it re-starts; re-attaching and re-starting alone is a no-op.
 *
 * None of it is directly observable — `startInFlight` and
 * `pendingAdvertiseReason` are private, and Robolectric's shadow advertiser
 * accepts `startAdvertising` without invoking the callback, so `isAdvertising`
 * never flips. The probe is [FakeHost.advertiseDataBuilds]: `buildAdvertiseData`
 * is called just-in-time *inside* `start`, past all three gates, so it counts
 * exactly the starts that reached the platform call and nothing else.
 */
@RunWith(RobolectricTestRunner::class)
@Config(sdk = [34])
class LeAdvertiserTest {

    private class FakeHost(var gattReady: Boolean) : LeAdvertiser.Host {
        /**
         * Number of starts that got past every gate and reached the platform
         * call. `buildAdvertiseData` is invoked just-in-time after the in-flight
         * gate, the GATT-readiness check and the null-advertiser bail, so this
         * is an exact count of admitted starts.
         */
        var advertiseDataBuilds = 0

        override fun isGattServerReady(): Boolean = gattReady
        override fun buildAdvertiseData(): AdvertiseData {
            advertiseDataBuilds++
            return AdvertiseData.Builder().build()
        }
        override fun buildScanResponse(): AdvertiseData = AdvertiseData.Builder().build()
        override fun refreshPublishedIdentity() {}
        // Defeat throttling so every admitted call is individually observable.
        override fun shouldLog(key: String, intervalMs: Long): Boolean = true
    }

    private val seen = mutableListOf<String>()
    private val host = FakeHost(gattReady = true)
    private val advertiser = LeAdvertiser(
        bleHandler = Handler(Looper.getMainLooper()),
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
        assertEquals(0, host.advertiseDataBuilds)
        assertEquals(
            listOf("Deferring BLE advertising — advertiser unavailable"),
            seen,
        )

        // Adapter comes back and the facade re-attaches. This start must be
        // admitted — if the previous one had raised the gate, it would return
        // at `if (startInFlight) return` and advertising would be wedged off
        // for good.
        advertiser.attachAdvertiser(platformAdvertiser())
        advertiser.start("after-adapter-recovery")
        assertEquals(1, host.advertiseDataBuilds)
    }

    @Test
    fun `a start surviving an adapter cycle is admitted only after a stop`() {
        host.gattReady = true
        advertiser.attachAdvertiser(platformAdvertiser())
        advertiser.start("initial")
        assertEquals(1, host.advertiseDataBuilds)

        // Bluetooth goes off and comes back. The platform stops advertising
        // without delivering onStartFailure, so nothing lowered the gate that
        // this successful start raised — it now guards an advertisement that is
        // already dead. A recovery that only re-attaches and re-starts, which is
        // the obvious shape, is swallowed whole and the device stays
        // discoverable to nobody.
        advertiser.attachAdvertiser(platformAdvertiser())
        advertiser.start("adapter_recovery")
        assertEquals(1, host.advertiseDataBuilds)

        // Which is what makes the stop in the facade's recovery runnable
        // load-bearing rather than defensive.
        advertiser.stop()
        advertiser.start("adapter_recovery")
        assertEquals(2, host.advertiseDataBuilds)
    }

    @Test
    fun `a null advertiser still latches the deferral for onGattServerReady`() {
        // GATT registration in flight *and* no advertiser yet — the null check
        // must not short-circuit the deferral that the GATT branch sets up.
        host.gattReady = false
        advertiser.start("deferred-while-adapter-off")
        assertEquals(listOf("Waiting for GATT service registration"), seen)
        assertEquals(0, host.advertiseDataBuilds)

        // Service registration lands after the adapter came back. The latched
        // reason is what makes this start happen at all.
        advertiser.attachAdvertiser(platformAdvertiser())
        host.gattReady = true
        advertiser.onGattServerReady()
        assertEquals(1, host.advertiseDataBuilds)
    }

    @Test
    fun `onGattServerReady is inert when no start was ever deferred`() {
        // Guards the negative half of the test above: the admitted start there
        // has to come from the latched deferral, not from onGattServerReady
        // starting unconditionally.
        host.gattReady = true
        advertiser.attachAdvertiser(platformAdvertiser())

        advertiser.onGattServerReady()
        assertEquals(0, host.advertiseDataBuilds)

        advertiser.stop()
        assertEquals(emptyList<String>(), seen)
    }
}
