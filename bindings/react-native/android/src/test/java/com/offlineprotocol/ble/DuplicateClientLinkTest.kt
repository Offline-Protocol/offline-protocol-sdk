package com.offlineprotocol.ble

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

        assertTrue(registry.hasOtherClientLink("off1mac", excluding = "AA:AA:AA:AA:AA:02"))
        assertFalse(
            "the kept link is never its own duplicate",
            registry.hasOtherClientLink("off1mac", excluding = "AA:AA:AA:AA:AA:01"),
        )
    }

    @Test
    fun `a server link to the same peer is not a duplicate`() {
        val registry = MeshConnectionRegistry()
        registry.setDeviceIdentifier("BB:BB:BB:BB:BB:01", "off1mac")
        registry.trackServerConnection("BB:BB:BB:BB:BB:01")
        registry.registerGatt("BB:BB:BB:BB:BB:02", gatt("BB:BB:BB:BB:BB:02"))

        assertFalse(registry.hasOtherClientLink("off1mac", excluding = "BB:BB:BB:BB:BB:02"))
    }

    @Test
    fun `a closed client link no longer makes the next one a duplicate`() {
        val registry = MeshConnectionRegistry()
        registry.registerGatt("CC:CC:CC:CC:CC:01", gatt("CC:CC:CC:CC:CC:01"))
        registry.setDeviceIdentifier("CC:CC:CC:CC:CC:01", "off1mac")
        registry.removeGatt("CC:CC:CC:CC:CC:01")

        assertFalse(registry.hasOtherClientLink("off1mac", excluding = "CC:CC:CC:CC:CC:02"))
    }

    @Test
    fun `a refused address stays undialed until the suppression lapses`() {
        val registry = MeshConnectionRegistry()
        registry.suppressDuplicate("DD:DD:DD:DD:DD:02", untilMs = 1_000L)

        assertTrue(registry.isSuppressedDuplicate("DD:DD:DD:DD:DD:02", nowMs = 999L))
        assertFalse(registry.isSuppressedDuplicate("DD:DD:DD:DD:DD:02", nowMs = 1_000L))
        assertFalse(
            "a lapsed refusal is forgotten",
            registry.isSuppressedDuplicate("DD:DD:DD:DD:DD:02", nowMs = 0L),
        )
        assertFalse(registry.isSuppressedDuplicate("DD:DD:DD:DD:DD:03", nowMs = 0L))
    }

    @Test
    fun `clear forgets every refusal`() {
        val registry = MeshConnectionRegistry()
        registry.suppressDuplicate("EE:EE:EE:EE:EE:02", untilMs = Long.MAX_VALUE)
        registry.clear()

        assertFalse(registry.isSuppressedDuplicate("EE:EE:EE:EE:EE:02", nowMs = 0L))
    }
}
