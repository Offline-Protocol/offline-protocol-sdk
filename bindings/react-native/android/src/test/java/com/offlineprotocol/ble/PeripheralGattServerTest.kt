package com.offlineprotocol.ble

import org.junit.Assert.assertArrayEquals
import org.junit.Assert.assertEquals
import org.junit.Assert.assertSame
import org.junit.Assert.assertTrue
import org.junit.Test

/**
 * Unit tests for [sliceForReadOffset], the helper that backs the long-read
 * offset handling in [PeripheralGattServer.onCharacteristicReadRequest].
 *
 * Regression guard: the pre-fix code returned the full value on every read
 * regardless of `offset`, which silently corrupted any GATT read larger than
 * `ATT_MTU - 1` bytes — the exact path used to serve signed-identity blobs
 * to iOS centrals that hadn't yet negotiated a larger MTU.
 */
class PeripheralGattServerTest {

    @Test
    fun `offset 0 returns full value`() {
        val value = byteArrayOf(1, 2, 3, 4, 5)
        assertArrayEquals(value, sliceForReadOffset(value, 0))
    }

    @Test
    fun `negative offset is treated as 0`() {
        val value = byteArrayOf(1, 2, 3, 4, 5)
        assertArrayEquals(value, sliceForReadOffset(value, -1))
        assertArrayEquals(value, sliceForReadOffset(value, -100))
    }

    @Test
    fun `offset in the middle returns the tail`() {
        val value = byteArrayOf(1, 2, 3, 4, 5)
        assertArrayEquals(byteArrayOf(3, 4, 5), sliceForReadOffset(value, 2))
        assertArrayEquals(byteArrayOf(5), sliceForReadOffset(value, 4))
    }

    @Test
    fun `offset equal to size returns empty`() {
        val value = byteArrayOf(1, 2, 3, 4, 5)
        assertEquals(0, sliceForReadOffset(value, 5).size)
    }

    @Test
    fun `offset past end returns empty`() {
        val value = byteArrayOf(1, 2, 3, 4, 5)
        assertEquals(0, sliceForReadOffset(value, 6).size)
        assertEquals(0, sliceForReadOffset(value, 1000).size)
    }

    @Test
    fun `empty value always returns empty`() {
        val value = ByteArray(0)
        assertEquals(0, sliceForReadOffset(value, 0).size)
        assertEquals(0, sliceForReadOffset(value, 5).size)
    }

    @Test
    fun `simulated long-read reassembles the original value`() {
        // Reproduce the Android long-read flow end-to-end: a central issues
        // repeated reads with increasing offsets, advancing by 20 bytes on
        // each call (a pessimistic default-ATT-MTU payload), and appends
        // each returned slice to a buffer. The concatenation must equal the
        // original value; this is the property the pre-fix code violated.
        val original = ByteArray(137) { i -> (i and 0xFF).toByte() }
        val mtuPayload = 20
        val reassembled = ArrayList<Byte>(original.size)
        var offset = 0
        while (true) {
            val slice = sliceForReadOffset(original, offset)
            if (slice.isEmpty()) break
            val chunk = slice.copyOf(minOf(mtuPayload, slice.size))
            chunk.forEach { reassembled.add(it) }
            offset += chunk.size
            if (chunk.size < mtuPayload) break
        }
        assertArrayEquals(original, reassembled.toByteArray())
    }

    @Test
    fun `MAX_INBOUND_WRITE_BYTES bound is above mesh fragment size`() {
        // Sanity guard so the inbound size cap can't drift below the
        // facade's MAX_FRAGMENT_SIZE (185) without someone noticing.
        // A mesh fragment plus MTU-negotiation headroom must still fit.
        // NB: use JUnit's assertTrue, not Kotlin's `assert`, because the
        // latter is a no-op unless the JVM is started with `-ea`, which the
        // Gradle test task does not do by default.
        val meshFragmentMax = 185
        assertTrue(
            "MAX_INBOUND_WRITE_BYTES (${PeripheralGattServer.MAX_INBOUND_WRITE_BYTES}) " +
                "must exceed mesh fragment size ($meshFragmentMax)",
            PeripheralGattServer.MAX_INBOUND_WRITE_BYTES > meshFragmentMax,
        )
    }

    // --- classifyCccdWrite ---
    //
    // CCCD writes flip subscribed-central bookkeeping. Three properties
    // must hold, and they are the ones a future change would most plausibly
    // break by accident: the spec-exact notify/indicate bytes map to ENABLE,
    // the zero value maps to DISABLE, and any malformed payload also maps
    // to DISABLE (so a garbage write can only narrow the subscriber set,
    // never widen it).

    @Test
    fun `classifyCccdWrite maps notify-enable bytes to ENABLE`() {
        assertSame(
            CccdAction.ENABLE,
            classifyCccdWrite(byteArrayOf(0x01, 0x00)),
        )
    }

    @Test
    fun `classifyCccdWrite maps indicate-enable bytes to ENABLE`() {
        assertSame(
            CccdAction.ENABLE,
            classifyCccdWrite(byteArrayOf(0x02, 0x00)),
        )
    }

    @Test
    fun `classifyCccdWrite maps explicit disable bytes to DISABLE`() {
        assertSame(
            CccdAction.DISABLE,
            classifyCccdWrite(byteArrayOf(0x00, 0x00)),
        )
    }

    @Test
    fun `classifyCccdWrite maps empty payload to DISABLE`() {
        assertSame(CccdAction.DISABLE, classifyCccdWrite(ByteArray(0)))
    }

    @Test
    fun `classifyCccdWrite maps malformed payload to DISABLE`() {
        // A hostile central could write unexpected bytes to the CCCD. The
        // subscriber set must only ever narrow in response, never widen.
        assertSame(CccdAction.DISABLE, classifyCccdWrite(byteArrayOf(0x03, 0x00)))
        assertSame(CccdAction.DISABLE, classifyCccdWrite(byteArrayOf(0xFF.toByte())))
        assertSame(
            CccdAction.DISABLE,
            classifyCccdWrite(byteArrayOf(0x01, 0x00, 0x00)),
        )
    }

    @Test
    fun `classifyCccdWrite rejects byte-swapped notify value`() {
        // The wire format is little-endian 0x0001 → [0x01, 0x00]. A
        // big-endian encoding must not be accepted; a central that sends
        // one is out of spec and should not be counted as subscribed.
        assertSame(
            CccdAction.DISABLE,
            classifyCccdWrite(byteArrayOf(0x00, 0x01)),
        )
    }

    @Test
    fun `CCCD enable byte constants match BT spec`() {
        // Guard against a refactor quietly changing the exported
        // byte-pattern constants that back [classifyCccdWrite].
        assertArrayEquals(byteArrayOf(0x01, 0x00), CCCD_ENABLE_NOTIFICATION_BYTES)
        assertArrayEquals(byteArrayOf(0x02, 0x00), CCCD_ENABLE_INDICATION_BYTES)
    }
}
