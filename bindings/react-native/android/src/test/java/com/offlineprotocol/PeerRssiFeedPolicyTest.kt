package com.offlineprotocol

import android.os.ParcelUuid
import java.util.UUID
import org.junit.Assert.assertEquals
import org.junit.Assert.assertFalse
import org.junit.Assert.assertNull
import org.junit.Assert.assertTrue
import org.junit.Test
import org.junit.runner.RunWith
import org.robolectric.RobolectricTestRunner

/**
 * `PeerRssiFeedPolicy`: whether a reading reaches the core at all, and
 * whether the node hash goes on air. Both fail silently, as no peer in range.
 * The feed half mirrors ios/tests/PeerRssiFeedPolicyTests.swift.
 */
@RunWith(RobolectricTestRunner::class)
class PeerRssiFeedPolicyTest {
    private val serviceUuid = UUID.fromString("6E400001-B5A3-F393-E0A9-E50E24DCCA9E")

    @Test
    fun oneReadingPerIntervalPerAddress() {
        val fed = HashMap<String, Long>()
        assertTrue(PeerRssiFeedPolicy.admit(fed, "a", 1_000, linked = true, hasServiceData = false))
        assertFalse(PeerRssiFeedPolicy.admit(fed, "a", 1_100, linked = true, hasServiceData = false))
        assertTrue("another address is not held back", PeerRssiFeedPolicy.admit(fed, "b", 1_100, linked = false, hasServiceData = true))
        assertTrue(PeerRssiFeedPolicy.admit(fed, "a", 1_000 + PeerRssiFeedPolicy.INTERVAL_MS, linked = true, hasServiceData = false))
    }

    @Test
    fun aResultTheCoreCouldNotAttributeIsNeverFed() {
        val fed = HashMap<String, Long>()
        assertFalse(PeerRssiFeedPolicy.admit(fed, "a", 1_000, linked = false, hasServiceData = false))
        assertTrue("and does not use up the address's slot", PeerRssiFeedPolicy.admit(fed, "a", 1_001, linked = true, hasServiceData = false))
    }

    @Test
    fun theIntervalLeavesRoomUnderTheCoreCap() {
        // The core drops anything under 250 ms; equal intervals halved the rate.
        assertTrue(PeerRssiFeedPolicy.INTERVAL_MS > 250)
    }

    @Test
    fun theScanResponseCarriesTheHashInPlaceOfTheUuidAndFits() {
        val hash = ByteArray(8) { it.toByte() }
        val data = PeerRssiFeedPolicy.scanResponse(serviceUuid, hash)
        assertEquals(1, data.serviceData.size)
        assertEquals(hash.toList(), data.serviceData[ParcelUuid(serviceUuid)]!!.toList())
        assertTrue(data.serviceUuids.isNullOrEmpty())
        assertFalse(data.includeDeviceName)
        assertEquals(26, adBytes(data))
        assertTrue(adBytes(data) <= 31)
    }

    @Test
    fun withoutTheHashTheScanResponseIsTheBareUuid() {
        val data = PeerRssiFeedPolicy.scanResponse(serviceUuid, null)
        assertEquals(listOf(ParcelUuid(serviceUuid)), data.serviceUuids)
        assertTrue(data.serviceData.isEmpty())
        assertNull(data.serviceData[ParcelUuid(serviceUuid)])
        assertEquals(18, adBytes(data))
    }

    /** Over-the-air size of the structures these builders use (128-bit UUIDs). */
    private fun adBytes(data: android.bluetooth.le.AdvertiseData): Int =
        (if (data.serviceUuids.isNullOrEmpty()) 0 else 2 + 16 * data.serviceUuids.size) +
            data.serviceData.values.sumOf { 2 + 16 + it.size }
}
