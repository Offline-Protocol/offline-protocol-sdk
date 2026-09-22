package com.offlineprotocol

import org.junit.Assert.assertEquals
import org.junit.Assert.assertFalse
import org.junit.Assert.assertNotEquals
import org.junit.Test

/**
 * Pins the APP_TAG value byte for byte. Mirrors iOS's BleAppTagTests; keep in
 * sync: a central on one platform matches a tag computed on the other, so the
 * two must agree on every byte.
 */
class BleAppTagTest {

    private fun hex(bytes: ByteArray): String =
        bytes.joinToString("") { "%02x".format(it.toInt() and 0xff) }

    /**
     * `SHA-256("offline-protocol-ble-app-tag-v1" || 0x00 || appId)[0..8]`,
     * computed independently with `shasum -a 256`.
     */
    @Test
    fun `vectors match the independent digest`() {
        assertEquals("2e2e6107567ff67b", hex(BleAppTag.compute("offline-messenger")))
        assertEquals("3a88052657e598e1", hex(BleAppTag.compute("offline-demo")))
        assertEquals("d5c9ed34fbf6503f", hex(BleAppTag.compute("")))
    }

    @Test
    fun `tag is eight bytes`() {
        assertEquals(8, BleAppTag.SIZE)
        assertEquals(8, BleAppTag.compute("offline-messenger").size)
    }

    /**
     * The domain separator is load-bearing: without it the tag would be the
     * prefix of a plain digest of the app id (`ce3cfe35f7b6c523` for this one).
     */
    @Test
    fun `tag is domain separated`() {
        assertNotEquals("ce3cfe35f7b6c523", hex(BleAppTag.compute("offline-messenger")))
    }

    @Test
    fun `different apps get different tags`() {
        assertFalse(
            BleAppTag.compute("offline-messenger").contentEquals(BleAppTag.compute("offline-demo"))
        )
    }
}
