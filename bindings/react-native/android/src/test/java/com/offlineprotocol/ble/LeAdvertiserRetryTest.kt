package com.offlineprotocol.ble

import android.bluetooth.BluetoothManager
import android.bluetooth.le.AdvertiseCallback
import android.bluetooth.le.AdvertiseData
import android.bluetooth.le.AdvertiseSettings
import android.bluetooth.le.BluetoothLeAdvertiser
import android.content.Context
import android.os.Handler
import android.os.Looper
import java.util.concurrent.TimeUnit
import org.junit.After
import org.junit.Assert.assertEquals
import org.junit.Test
import org.junit.runner.RunWith
import org.robolectric.RobolectricTestRunner
import org.robolectric.RuntimeEnvironment
import org.robolectric.Shadows.shadowOf
import org.robolectric.annotation.Config
import org.robolectric.annotation.Implementation
import org.robolectric.annotation.Implements
import org.robolectric.annotation.LooperMode

/**
 * Delivers a chosen [AdvertiseCallback] outcome synchronously from
 * `startAdvertising`. Robolectric's own shadow accepts the call and invokes
 * nothing, so a terminal failure — the only advertising death no other
 * self-healing path in the BLE stack owns — is otherwise unreachable in a test.
 */
@Implements(BluetoothLeAdvertiser::class)
@Suppress("UNUSED_PARAMETER")
class OutcomeBluetoothLeAdvertiserShadow {
    companion object {
        /** Error code to report, or `null` to report success. */
        var failWithCode: Int? = null
    }

    @Implementation
    protected fun startAdvertising(
        settings: AdvertiseSettings,
        advertiseData: AdvertiseData,
        scanResponse: AdvertiseData,
        callback: AdvertiseCallback,
    ) {
        val code = failWithCode
        if (code != null) {
            callback.onStartFailure(code)
        } else {
            callback.onStartSuccess(AdvertiseSettings.Builder().build())
        }
    }

    @Implementation
    protected fun stopAdvertising(callback: AdvertiseCallback) = Unit
}

/**
 * Pins the retry that follows a terminal `onStartFailure`.
 *
 * The failure this covers is the one advertising death that ends outside every
 * other recovery loop in the BLE stack: the adapter is on and the scan is
 * healthy, so neither the facade's adapter-off episode nor any scan-health
 * timer ever runs, and `onStartFailure` itself clears [LeAdvertiser.isAdvertising]
 * and the in-flight gate without scheduling anything. Before this retry existed
 * the device stayed discoverable to nobody until an unrelated identity refresh
 * or an app restart, while the transport kept reporting RUNNING.
 *
 * Two properties carry the weight and neither is visible in the source once
 * written:
 *
 *  1. **Only transient codes retry.** `FEATURE_UNSUPPORTED` is hardware truth
 *     and `ALREADY_STARTED` means an advertisement is running, so retrying
 *     either loops for the process lifetime.
 *  2. **A deliberate stop is authoritative over an armed retry.** The cancel
 *     sits above `stop()`'s `cb == null` early return, because a retry is armed
 *     exactly when a failure has already nulled the callback reference — so
 *     that return is the common path out of a stop following a failure. Below
 *     it, a paused or stopped transport would put itself back on air.
 */
@RunWith(RobolectricTestRunner::class)
@Config(sdk = [34], shadows = [OutcomeBluetoothLeAdvertiserShadow::class])
@LooperMode(LooperMode.Mode.PAUSED)
class LeAdvertiserRetryTest {

    private class FakeHost : LeAdvertiser.Host {
        /** Starts that got past every gate and reached the platform call. */
        var advertiseDataBuilds = 0

        override fun isGattServerReady(): Boolean = true
        override fun buildAdvertiseData(): AdvertiseData {
            advertiseDataBuilds++
            return AdvertiseData.Builder().build()
        }
        override fun buildScanResponse(): AdvertiseData = AdvertiseData.Builder().build()
        override fun refreshPublishedIdentity() = Unit
        override fun shouldLog(key: String, intervalMs: Long): Boolean = true
    }

    private val looper = shadowOf(Looper.getMainLooper())
    private val host = FakeHost()
    private val advertiser = LeAdvertiser(
        bleHandler = Handler(Looper.getMainLooper()),
        host = host,
        diagnosticEmitter = { _, _, _ -> },
    )
    private var elapsedMs = 0L

    @After
    fun resetShadow() {
        OutcomeBluetoothLeAdvertiserShadow.failWithCode = null
    }

    private fun platformAdvertiser() =
        ((RuntimeEnvironment.getApplication() as Context)
            .getSystemService(Context.BLUETOOTH_SERVICE) as BluetoothManager)
            .adapter!!
            .bluetoothLeAdvertiser

    /** Advance virtual time, running everything due on the main looper. */
    private fun advanceTo(targetMs: Long) {
        looper.idleFor(targetMs - elapsedMs, TimeUnit.MILLISECONDS)
        elapsedMs = targetMs
    }

    /** Start once and let the arming post run. */
    private fun startAndSettle(reason: String) {
        advertiser.start(reason)
        looper.idle()
    }

    @Test
    fun `a slot-exhaustion failure is retried`() {
        advertiser.attachAdvertiser(platformAdvertiser())
        OutcomeBluetoothLeAdvertiserShadow.failWithCode =
            AdvertiseCallback.ADVERTISE_FAILED_TOO_MANY_ADVERTISERS

        startAndSettle("initial")
        assertEquals(1, host.advertiseDataBuilds)

        // Another app released its advertising instance in the meantime. Nothing
        // else in the stack would ever try again.
        OutcomeBluetoothLeAdvertiserShadow.failWithCode = null
        advanceTo(9_999L)
        assertEquals(1, host.advertiseDataBuilds)
        advanceTo(10_000L)
        assertEquals(2, host.advertiseDataBuilds)
    }

    @Test
    fun `an internal stack error is retried`() {
        advertiser.attachAdvertiser(platformAdvertiser())
        OutcomeBluetoothLeAdvertiserShadow.failWithCode =
            AdvertiseCallback.ADVERTISE_FAILED_INTERNAL_ERROR

        startAndSettle("initial")
        OutcomeBluetoothLeAdvertiserShadow.failWithCode = null
        advanceTo(10_000L)

        assertEquals(2, host.advertiseDataBuilds)
    }

    @Test
    fun `an unsupported-feature failure is never retried`() {
        advertiser.attachAdvertiser(platformAdvertiser())
        OutcomeBluetoothLeAdvertiserShadow.failWithCode =
            AdvertiseCallback.ADVERTISE_FAILED_FEATURE_UNSUPPORTED

        startAndSettle("initial")
        assertEquals(1, host.advertiseDataBuilds)

        // The hardware cannot do this. A retry ladder here would burn wakeups
        // for the whole process lifetime and never succeed.
        advanceTo(120_000L)
        assertEquals(1, host.advertiseDataBuilds)
    }

    @Test
    fun `an already-started failure is never retried`() {
        advertiser.attachAdvertiser(platformAdvertiser())
        OutcomeBluetoothLeAdvertiserShadow.failWithCode =
            AdvertiseCallback.ADVERTISE_FAILED_ALREADY_STARTED

        startAndSettle("initial")
        advanceTo(120_000L)

        // An advertisement is already running, so every retry earns this same
        // code back.
        assertEquals(1, host.advertiseDataBuilds)
    }

    @Test
    fun `an oversized payload is never retried`() {
        advertiser.attachAdvertiser(platformAdvertiser())
        OutcomeBluetoothLeAdvertiserShadow.failWithCode =
            AdvertiseCallback.ADVERTISE_FAILED_DATA_TOO_LARGE

        startAndSettle("initial")
        advanceTo(120_000L)

        // The payload is built by the SDK and does not vary between attempts.
        assertEquals(1, host.advertiseDataBuilds)
    }

    @Test
    fun `an unknown failure code is retried`() {
        advertiser.attachAdvertiser(platformAdvertiser())
        OutcomeBluetoothLeAdvertiserShadow.failWithCode = 42

        startAndSettle("initial")
        OutcomeBluetoothLeAdvertiserShadow.failWithCode = null
        advanceTo(10_000L)

        assertEquals(2, host.advertiseDataBuilds)
    }

    @Test
    fun `a deliberate stop cancels an armed retry`() {
        advertiser.attachAdvertiser(platformAdvertiser())
        OutcomeBluetoothLeAdvertiserShadow.failWithCode =
            AdvertiseCallback.ADVERTISE_FAILED_INTERNAL_ERROR

        startAndSettle("initial")
        assertEquals(1, host.advertiseDataBuilds)

        // Every deliberate teardown in the facade — stop, shutdown, refresh, the
        // adapter-off repair, the BLE reset — funnels through stop(). The
        // failure above already nulled the callback reference, so this stop
        // takes the `cb == null` early return: a cancel placed below it would
        // let the transport put itself back on air behind the app's back.
        advertiser.stop()

        OutcomeBluetoothLeAdvertiserShadow.failWithCode = null
        advanceTo(120_000L)
        assertEquals(1, host.advertiseDataBuilds)
    }

    @Test
    fun `a successful start retires the retry and resets the ladder`() {
        advertiser.attachAdvertiser(platformAdvertiser())
        OutcomeBluetoothLeAdvertiserShadow.failWithCode =
            AdvertiseCallback.ADVERTISE_FAILED_INTERNAL_ERROR
        startAndSettle("initial")

        // The retry lands on a healthy stack and succeeds.
        OutcomeBluetoothLeAdvertiserShadow.failWithCode = null
        advanceTo(10_000L)
        assertEquals(2, host.advertiseDataBuilds)

        // Nothing further is pending: the success cancelled the ladder rather
        // than leaving a retry armed behind a live advertisement.
        advanceTo(120_000L)
        assertEquals(2, host.advertiseDataBuilds)

        // And the ladder is back on its bottom rung, so the next outage is
        // retried at 10s rather than at the cap it had climbed to.
        advertiser.stop()
        OutcomeBluetoothLeAdvertiserShadow.failWithCode =
            AdvertiseCallback.ADVERTISE_FAILED_INTERNAL_ERROR
        startAndSettle("second-outage")
        assertEquals(3, host.advertiseDataBuilds)
        OutcomeBluetoothLeAdvertiserShadow.failWithCode = null
        advanceTo(130_000L)
        assertEquals(4, host.advertiseDataBuilds)
    }

    @Test
    fun `a retry against a detached advertiser is inert`() {
        advertiser.attachAdvertiser(platformAdvertiser())
        OutcomeBluetoothLeAdvertiserShadow.failWithCode =
            AdvertiseCallback.ADVERTISE_FAILED_INTERNAL_ERROR
        startAndSettle("initial")
        assertEquals(1, host.advertiseDataBuilds)

        // The adapter went down after the failure was armed. Healing that —
        // scanner, GATT server and advertising together — belongs to the
        // facade's adapter-off episode, so the retry must land on the
        // null-advertiser bail rather than raise a gate no callback can lower.
        advertiser.attachAdvertiser(null)
        advanceTo(10_000L)
        assertEquals(1, host.advertiseDataBuilds)

        // And the bail must not have wedged the gate: once the episode
        // re-attaches, advertising starts.
        OutcomeBluetoothLeAdvertiserShadow.failWithCode = null
        advertiser.attachAdvertiser(platformAdvertiser())
        startAndSettle("adapter_recovery")
        assertEquals(2, host.advertiseDataBuilds)
    }
}
