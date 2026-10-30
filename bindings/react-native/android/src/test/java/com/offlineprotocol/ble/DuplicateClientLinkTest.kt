package com.offlineprotocol.ble

import org.junit.Assert.assertEquals
import org.junit.Assert.assertFalse
import org.junit.Assert.assertTrue
import org.junit.Test
import org.junit.runner.RunWith
import org.robolectric.RobolectricTestRunner
import org.robolectric.annotation.Config
import org.robolectric.shadows.ShadowBluetoothDevice
import org.robolectric.shadows.ShadowBluetoothGatt

/**
 * A macOS peripheral rotates its address and advertises under the old and the
 * new one at once, so the scanner dials both and both verify as one peer. The
 * second client link is the duplicate: it is refused, and its address is not
 * redialed until the suppression lapses. A link in the other direction is not
 * a duplicate.
 */
@RunWith(RobolectricTestRunner::class)
@Config(sdk = [34])
class DuplicateClientLinkTest {

    private fun gatt(address: String) =
        ShadowBluetoothGatt.newInstance(ShadowBluetoothDevice.newInstance(address))

    @Test
    fun `a second client link to a peer already held is a duplicate`() {
        val registry = MeshConnectionRegistry()
        registry.registerGatt("AA:AA:AA:AA:AA:01", gatt("AA:AA:AA:AA:AA:01"))
        registry.setDeviceIdentifier("AA:AA:AA:AA:AA:01", "off1mac")
        registry.registerGatt("AA:AA:AA:AA:AA:02", gatt("AA:AA:AA:AA:AA:02"))

        assertTrue(registry.hasOtherUsableLink("off1mac", excluding = "AA:AA:AA:AA:AA:02") { false })
        assertFalse(
            "the kept link is never its own duplicate",
            registry.hasOtherUsableLink("off1mac", excluding = "AA:AA:AA:AA:AA:01") { false },
        )
    }

    @Test
    fun `an unsubscribed server link to the same peer is not a duplicate`() {
        val registry = MeshConnectionRegistry()
        registry.setDeviceIdentifier("BB:BB:BB:BB:BB:01", "off1mac")
        registry.trackServerConnection("BB:BB:BB:BB:BB:01")
        registry.registerGatt("BB:BB:BB:BB:BB:02", gatt("BB:BB:BB:BB:BB:02"))

        assertFalse(registry.hasOtherUsableLink("off1mac", excluding = "BB:BB:BB:BB:BB:02") { false })
    }

    @Test
    fun `a closed client link no longer makes the next one a duplicate`() {
        val registry = MeshConnectionRegistry()
        registry.registerGatt("CC:CC:CC:CC:CC:01", gatt("CC:CC:CC:CC:CC:01"))
        registry.setDeviceIdentifier("CC:CC:CC:CC:CC:01", "off1mac")
        registry.removeGatt("CC:CC:CC:CC:CC:01")

        assertFalse(registry.hasOtherUsableLink("off1mac", excluding = "CC:CC:CC:CC:CC:02") { false })
    }

    /** A registry holding a client link to `off1mac` at the kept address. */
    private fun holding(kept: String): MeshConnectionRegistry {
        val registry = MeshConnectionRegistry()
        registry.registerGatt(kept, gatt(kept))
        registry.setDeviceIdentifier(kept, "off1mac")
        return registry
    }

    @Test
    fun `a refused address stays undialed until the suppression lapses`() {
        val registry = holding("DD:DD:DD:DD:DD:01")
        registry.suppressDuplicate("DD:DD:DD:DD:DD:02", "off1mac", untilMs = 1_000L)

        assertTrue(registry.isSuppressedDuplicate("DD:DD:DD:DD:DD:02", nowMs = 999L))
        assertFalse(registry.isSuppressedDuplicate("DD:DD:DD:DD:DD:02", nowMs = 1_000L))
        assertFalse(
            "a lapsed refusal is forgotten",
            registry.isSuppressedDuplicate("DD:DD:DD:DD:DD:02", nowMs = 0L),
        )
        assertFalse(registry.isSuppressedDuplicate("DD:DD:DD:DD:DD:03", nowMs = 0L))
    }

    @Test
    fun `a refusal lapses early once the kept link is gone`() {
        val registry = holding("FF:FF:FF:FF:FF:01")
        registry.suppressDuplicate("FF:FF:FF:FF:FF:02", "off1mac", untilMs = Long.MAX_VALUE)
        assertTrue(registry.isSuppressedDuplicate("FF:FF:FF:FF:FF:02", nowMs = 0L))

        registry.removeGatt("FF:FF:FF:FF:FF:01")

        assertFalse(
            "the peer must be redialable at the address it still advertises",
            registry.isSuppressedDuplicate("FF:FF:FF:FF:FF:02", nowMs = 0L),
        )
    }

    @Test
    fun `clear forgets every refusal`() {
        val registry = holding("EE:EE:EE:EE:EE:01")
        registry.suppressDuplicate("EE:EE:EE:EE:EE:02", "off1mac", untilMs = Long.MAX_VALUE)
        registry.clear()
        // The kept link comes back, so only a forgotten refusal answers false.
        registry.registerGatt("EE:EE:EE:EE:EE:01", gatt("EE:EE:EE:EE:EE:01"))
        registry.setDeviceIdentifier("EE:EE:EE:EE:EE:01", "off1mac")

        assertFalse(registry.isSuppressedDuplicate("EE:EE:EE:EE:EE:02", nowMs = 0L))
    }

    /**
     * Run 5 of the 2026-10-19 volume test: the iPhone held a link to our
     * server at its old address and was subscribed; its address rotated, we
     * dialled the "new" device and kept it. That second Link Layer connection
     * between the same two radios is what made the reconnect impossible after
     * the first link dropped. A subscribed server link makes it a duplicate.
     */
    @Test
    fun `a subscribed server link to the peer makes a client link a duplicate`() {
        val registry = MeshConnectionRegistry()
        registry.setDeviceIdentifier("41:9B:ED:3F:CC:72", "off1iphone")
        registry.trackServerConnection("41:9B:ED:3F:CC:72")
        registry.registerGatt("4A:2A:C8:EB:B3:4D", gatt("4A:2A:C8:EB:B3:4D"))

        assertTrue(
            registry.hasOtherUsableLink("off1iphone", excluding = "4A:2A:C8:EB:B3:4D") {
                it == "41:9B:ED:3F:CC:72"
            },
        )
        assertFalse(
            "a central that never subscribed carries nothing, so our link is the usable one",
            registry.hasOtherUsableLink("off1iphone", excluding = "4A:2A:C8:EB:B3:4D") { false },
        )
    }

    @Test
    fun `a client link over the peer's own link to us is not a duplicate`() {
        // Android reports our dial to an address the peer is connected from
        // as the same address on both sides: one Link Layer connection.
        val registry = MeshConnectionRegistry()
        registry.setDeviceIdentifier("41:9B:ED:3F:CC:72", "off1iphone")
        registry.trackServerConnection("41:9B:ED:3F:CC:72")
        registry.registerGatt("41:9B:ED:3F:CC:72", gatt("41:9B:ED:3F:CC:72"))

        assertFalse(
            registry.hasOtherUsableLink("off1iphone", excluding = "41:9B:ED:3F:CC:72") { true },
        )
    }

    @Test
    fun `a refusal held by a subscribed server link lapses when that link goes`() {
        val registry = MeshConnectionRegistry()
        registry.setDeviceIdentifier("41:9B:ED:3F:CC:72", "off1iphone")
        registry.trackServerConnection("41:9B:ED:3F:CC:72")
        registry.suppressDuplicate("4A:2A:C8:EB:B3:4D", "off1iphone", untilMs = Long.MAX_VALUE)
        val subscribed: (String) -> Boolean = { it == "41:9B:ED:3F:CC:72" }

        assertTrue(registry.isSuppressedDuplicate("4A:2A:C8:EB:B3:4D", nowMs = 0L, isSubscribed = subscribed))
        registry.untrackServerConnection("41:9B:ED:3F:CC:72")
        assertFalse(registry.isSuppressedDuplicate("4A:2A:C8:EB:B3:4D", nowMs = 0L, isSubscribed = subscribed))
    }

    @Test
    fun `a subscription at an address with no server link is not a link`() {
        // The AND with the server connection: a stale subscription entry
        // alone must never block dialling the peer.
        val registry = MeshConnectionRegistry()
        registry.setDeviceIdentifier("41:9B:ED:3F:CC:72", "off1iphone")
        registry.registerGatt("4A:2A:C8:EB:B3:4D", gatt("4A:2A:C8:EB:B3:4D"))

        assertFalse(
            registry.hasOtherUsableLink("off1iphone", excluding = "4A:2A:C8:EB:B3:4D") { true },
        )
    }

    @Test
    fun `a refusal lapses when the peer unsubscribes but stays connected`() {
        val registry = MeshConnectionRegistry()
        registry.setDeviceIdentifier("41:9B:ED:3F:CC:72", "off1iphone")
        registry.trackServerConnection("41:9B:ED:3F:CC:72")
        registry.suppressDuplicate("4A:2A:C8:EB:B3:4D", "off1iphone", untilMs = Long.MAX_VALUE)
        var subscribed = true

        assertTrue(registry.isSuppressedDuplicate("4A:2A:C8:EB:B3:4D", nowMs = 0L) { subscribed })
        subscribed = false
        assertFalse(registry.isSuppressedDuplicate("4A:2A:C8:EB:B3:4D", nowMs = 0L) { subscribed })
    }

    /**
     * 2026-10-29: an iPhone whose app restarted kept its link to our server
     * and wrote over it with no hello. The dial that resolves such writes
     * verified it and was refused as a duplicate of the link we already held,
     * and nothing mapped the server link, so every frame it wrote was lost.
     */
    @Test
    fun `a duplicate dial binds the unmapped server link it proved`() {
        val registry = MeshConnectionRegistry()
        registry.setDeviceIdentifier("4A:A7:D1:56:B4:F8", "off1iphone")
        registry.trackServerConnection("4A:A7:D1:56:B4:F8")
        registry.trackServerConnection("73:57:D7:16:EF:A7")

        assertTrue(registry.bindUnmappedServerLink("73:57:D7:16:EF:A7", "off1iphone", writerPending = true))
        assertEquals("off1iphone", registry.deviceIdForAddress("73:57:D7:16:EF:A7"))
    }

    @Test
    fun `a server link already mapped is never rebound`() {
        val registry = MeshConnectionRegistry()
        registry.setDeviceIdentifier("73:57:D7:16:EF:A7", "off1other")
        registry.trackServerConnection("73:57:D7:16:EF:A7")

        assertFalse(registry.bindUnmappedServerLink("73:57:D7:16:EF:A7", "off1iphone", writerPending = true))
        assertEquals("off1other", registry.deviceIdForAddress("73:57:D7:16:EF:A7"))
    }

    @Test
    fun `the echo of our own dial is not bound`() {
        // Android reports our own dial to the server too, so every refused
        // dial has a server connection at its address. Bound, the peer was
        // repointed at an address being closed.
        val registry = MeshConnectionRegistry()
        registry.setDeviceIdentifier("4A:A7:D1:56:B4:F8", "off1mac")
        registry.trackServerConnection("6C:1D:22:90:0E:3B")

        assertFalse(registry.bindUnmappedServerLink("6C:1D:22:90:0E:3B", "off1mac", writerPending = false))
        assertEquals(null, registry.deviceIdForAddress("6C:1D:22:90:0E:3B"))
        assertEquals("4A:A7:D1:56:B4:F8", registry.addressForDevice("off1mac"))
    }

    @Test
    fun `an address with no server link is not bound`() {
        val registry = MeshConnectionRegistry()

        assertFalse(registry.bindUnmappedServerLink("73:57:D7:16:EF:A7", "off1iphone", writerPending = true))
        assertEquals(null, registry.deviceIdForAddress("73:57:D7:16:EF:A7"))
    }

    @Test
    fun `only identified links hold the advertisement`() {
        val registry = MeshConnectionRegistry()
        registry.trackServerConnection("41:9B:ED:3F:CC:72")
        assertTrue(registry.establishedLinkCount() == 1)
        assertTrue("an unverified central does not count", registry.identifiedLinkCount() == 0)
        registry.setDeviceIdentifier("41:9B:ED:3F:CC:72", "off1iphone")
        assertTrue(registry.identifiedLinkCount() == 1)
    }
}
