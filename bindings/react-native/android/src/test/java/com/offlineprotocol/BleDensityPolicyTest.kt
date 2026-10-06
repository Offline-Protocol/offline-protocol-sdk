package com.offlineprotocol

import org.junit.Assert.assertEquals
import org.junit.Assert.assertFalse
import org.junit.Assert.assertTrue
import org.junit.Test

/**
 * [BleDensityPolicy]: two phones in a room full of other Bluetooth devices
 * passed each other over for minutes, because every advert in range counted
 * as mesh density and the pass-over was fixed per address.
 */
class BleDensityPolicyTest {

    private val low = 10
    private val high = 50

    @Test
    fun `density counts distinct candidates, not callbacks`() {
        val seen = HashMap<String, Long>()
        var count = 0
        // One peer advertising ten times a second for five seconds.
        for (t in 0L until 5_000L step 100L) {
            count = BleDensityPolicy.recordAndCount(seen, "AA:BB:CC:DD:EE:01", t, 5_000L)
        }
        assertEquals(1, count)
        assertEquals(2, BleDensityPolicy.recordAndCount(seen, "AA:BB:CC:DD:EE:02", 5_000L, 5_000L))
    }

    @Test
    fun `candidates not seen within the window stop counting`() {
        val seen = HashMap<String, Long>()
        BleDensityPolicy.recordAndCount(seen, "old", 0L, 5_000L)
        assertEquals(1, BleDensityPolicy.recordAndCount(seen, "new", 5_001L, 5_000L))
    }

    @Test
    fun `a sparse mesh passes nobody over`() {
        assertEquals(0.0, BleDensityPolicy.skipShare(low, low, high), 0.0)
        for (i in 0 until 500) {
            assertFalse(BleDensityPolicy.shouldSkip("peer-$i", meshPeerCount = 2, now = 0L, lowThreshold = low, highThreshold = high))
        }
    }

    @Test
    fun `the share rises with density and stops at the cap`() {
        assertEquals(0.4, BleDensityPolicy.skipShare(30, low, high), 1e-9)
        assertEquals(BleDensityPolicy.MAX_SKIP_SHARE, BleDensityPolicy.skipShare(500, low, high), 0.0)
    }

    @Test
    fun `a peer passed over in one slot is reconsidered in a later one`() {
        // At the cap 80% are passed over per slot. Keyed on the address alone,
        // the same 80% were passed over in every slot; each slot is a fresh
        // draw now, so over 100 slots (0.8^100 apart) every one comes up.
        val slot = BleDensityPolicy.SKIP_SLOT_MS
        for (i in 0 until 200) {
            val address = "5B:E6:7E:6F:%02X:%02X".format(i / 256, i % 256)
            val considered = (0 until 100).count { s ->
                !BleDensityPolicy.shouldSkip(address, meshPeerCount = 500, now = s * slot, lowThreshold = low, highThreshold = high)
            }
            assertTrue("$address was passed over in all 100 slots", considered > 0)
        }
    }

    @Test
    fun `the share passed over in a slot is the configured share`() {
        val skipped = (0 until 2_000).count { i ->
            BleDensityPolicy.shouldSkip("peer-$i", meshPeerCount = 30, now = 0L, lowThreshold = low, highThreshold = high)
        }
        // 0.4 of 2000, with room for the hash.
        assertTrue("skipped $skipped of 2000", skipped in 700..900)
    }

    @Test
    fun `one address is not stuck in neighbouring buckets across slots`() {
        val buckets = (0L until 10L).map { BleDensityPolicy.bucket("51:AC:A5:39:6C:00", it) }
        assertTrue("buckets $buckets", buckets.maxOrNull()!! - buckets.minOrNull()!! > 0.3)
    }

    @Test
    fun `within a slot the decision holds`() {
        val slot = BleDensityPolicy.SKIP_SLOT_MS
        val first = BleDensityPolicy.shouldSkip("51:AC:A5:39:6C:00", 30, 3 * slot, low, high)
        for (t in 3 * slot until 4 * slot step 1_000L) {
            assertEquals(first, BleDensityPolicy.shouldSkip("51:AC:A5:39:6C:00", 30, t, low, high))
        }
    }

    @Test
    fun `the bucket matches the iOS hash`() {
        // ios/tests/BleDensityPolicyTests.swift pins the same values, so a
        // mixed Android and iOS mesh agrees on what "dense" passes over.
        assertEquals(0.133, BleDensityPolicy.bucket("51:AC:A5:39:6C:00", 7), 1e-9)
        assertEquals(0.314, BleDensityPolicy.bucket("51:AC:A5:39:6C:00", 0), 1e-9)
    }
}
