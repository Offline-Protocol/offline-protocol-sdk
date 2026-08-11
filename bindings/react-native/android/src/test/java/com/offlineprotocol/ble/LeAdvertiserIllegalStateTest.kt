package com.offlineprotocol.ble

import android.bluetooth.BluetoothManager
import android.bluetooth.le.AdvertiseCallback
import android.bluetooth.le.AdvertiseData
import android.bluetooth.le.AdvertiseSettings
import android.bluetooth.le.BluetoothLeAdvertiser
import android.content.Context
import android.os.Handler
import android.os.Looper
import org.junit.After
import org.junit.Assert.assertEquals
import org.junit.Assert.assertTrue
import org.junit.Test
import org.junit.runner.RunWith
import org.robolectric.RobolectricTestRunner
import org.robolectric.RuntimeEnvironment
import org.robolectric.annotation.Config
import org.robolectric.annotation.Implementation
import org.robolectric.annotation.Implements

@Implements(BluetoothLeAdvertiser::class)
@Suppress("UNUSED_PARAMETER")
class ThrowingBluetoothLeAdvertiserShadow {
    companion object {
        var throwOnStart = false
        var throwOnStop = false
    }

    @Implementation
    protected fun startAdvertising(
        settings: AdvertiseSettings,
        advertiseData: AdvertiseData,
        scanResponse: AdvertiseData,
        callback: AdvertiseCallback,
    ) {
        if (throwOnStart) {
            throw IllegalStateException("BT Adapter is not turned ON")
        }
    }

    @Implementation
    protected fun stopAdvertising(callback: AdvertiseCallback) {
        if (throwOnStop) {
            throw IllegalStateException("BT Adapter is not turned ON")
        }
    }
}

@RunWith(RobolectricTestRunner::class)
@Config(sdk = [34], shadows = [ThrowingBluetoothLeAdvertiserShadow::class])
class LeAdvertiserIllegalStateTest {
    private class FakeHost : LeAdvertiser.Host {
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

    private val seen = mutableListOf<String>()
    private val host = FakeHost()
    private val advertiser = LeAdvertiser(
        bleHandler = Handler(Looper.getMainLooper()),
        host = host,
        diagnosticEmitter = { _, message, _ -> seen.add(message) },
    )

    @After
    fun resetShadow() {
        ThrowingBluetoothLeAdvertiserShadow.throwOnStart = false
        ThrowingBluetoothLeAdvertiserShadow.throwOnStop = false
    }

    @Test
    fun `adapter-off start failure releases the in-flight gate`() {
        val platformAdvertiser =
            ((RuntimeEnvironment.getApplication() as Context)
                .getSystemService(Context.BLUETOOTH_SERVICE) as BluetoothManager)
                .adapter!!
                .bluetoothLeAdvertiser
        advertiser.attachAdvertiser(platformAdvertiser)

        ThrowingBluetoothLeAdvertiserShadow.throwOnStart = true
        advertiser.start("adapter-off-race")
        assertEquals(1, host.advertiseDataBuilds)
        assertTrue(seen.contains("Skipping startAdvertising — BT adapter not on"))

        ThrowingBluetoothLeAdvertiserShadow.throwOnStart = false
        advertiser.start("adapter-recovered")
        assertEquals(2, host.advertiseDataBuilds)
    }

    @Test
    fun `adapter-off stop failure leaves the next start admitted`() {
        val platformAdvertiser =
            ((RuntimeEnvironment.getApplication() as Context)
                .getSystemService(Context.BLUETOOTH_SERVICE) as BluetoothManager)
                .adapter!!
                .bluetoothLeAdvertiser
        advertiser.attachAdvertiser(platformAdvertiser)
        advertiser.start("initial")
        assertEquals(1, host.advertiseDataBuilds)

        ThrowingBluetoothLeAdvertiserShadow.throwOnStop = true
        advertiser.stop()
        assertTrue(seen.contains("Skipping stopAdvertising — BT adapter not on"))

        ThrowingBluetoothLeAdvertiserShadow.throwOnStop = false
        advertiser.start("adapter-recovered")
        assertEquals(2, host.advertiseDataBuilds)
    }
}
