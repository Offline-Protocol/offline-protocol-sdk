package com.offlineprotocol

import com.offlineprotocol.WifiDirectGroupFormation.Action
import com.offlineprotocol.WifiDirectGroupFormation.Advert
import com.offlineprotocol.WifiDirectGroupFormation.Local
import org.junit.Assert.assertEquals
import org.junit.Assert.assertNotEquals
import org.junit.Assert.assertNull
import org.junit.Assert.assertTrue
import org.junit.Test

/**
 * [WifiDirectGroupFormation]: which device of an application creates its Wi-Fi
 * Direct group, which join it, and the credentials. The manager's radio calls
 * need a device (docs/bridges C9); the decisions are pinned here, including the
 * asymmetric cases two phones produced: one hearing the other minutes before,
 * or never.
 */
class WifiDirectGroupFormationTest {

    private val app = WifiDirectGroupFormation.appTag("com.example.app")

    private fun advert(address: String, appTag: String = app) =
        Advert(device = "mac-$address", address = address, app = appTag, seenAtMs = 0)

    private fun local(
        address: String,
        inGroup: Boolean = false,
        failedJoins: Int = 0,
        ownerNearby: Boolean = false,
    ) = Local(address, app, inGroup, failedJoins, ownerNearby)

    private fun decide(local: Local, vararg heard: String) =
        WifiDirectGroupFormation.decide(local, heard.map { advert(it) })

    // --- Who creates ----------------------------------------------------------

    @Test
    fun `alone, with no group owner nearby, a device does nothing`() {
        assertEquals(Action.Wait, decide(local("off1b")))
    }

    @Test
    fun `the lowest address heard creates at once`() {
        assertEquals(Action.Create, decide(local("off1a"), "off1b", "off1c"))
    }

    @Test
    fun `the lowest joins first when an owner is nearby, then creates if that found nothing`() {
        assertEquals(Action.Join, decide(local("off1a", ownerNearby = true), "off1b"))
        assertEquals(Action.Create, decide(local("off1a", ownerNearby = true, failedJoins = 1), "off1b"))
    }

    @Test
    fun `a higher address joins the group the lower one creates`() {
        assertEquals(Action.Join, decide(local("off1b"), "off1a"))
    }

    @Test
    fun `of three that hear each other, exactly one creates`() {
        val all = listOf("off1a", "off1b", "off1c")
        val actions = all.associateWith { me -> decide(local(me), *all.filter { it != me }.toTypedArray()) }
        assertEquals(mapOf("off1a" to Action.Create, "off1b" to Action.Join, "off1c" to Action.Join), actions)
    }

    // --- Asymmetric hearing ------------------------------------------------------

    @Test
    fun `a device that heard a lower peer takes over after its joins find no group`() {
        // The phones' failure: B heard A, A never heard B, so A never created.
        val heardLower = local("off1b", failedJoins = WifiDirectGroupFormation.TAKEOVER_FAILED_JOINS - 1)
        assertEquals(Action.Join, decide(heardLower, "off1a"))
        val takeover = local("off1b", failedJoins = WifiDirectGroupFormation.TAKEOVER_FAILED_JOINS)
        assertEquals(Action.Create, decide(takeover, "off1a"))
    }

    @Test
    fun `a device that heard nothing still joins a group owner it can see`() {
        // A group owner answers no service discovery query, so the record of
        // the device that created the group may never arrive.
        assertEquals(Action.Join, decide(local("off1a", ownerNearby = true)))
        // It never creates on that alone: an owner nearby may be a television.
        assertEquals(Action.Join, decide(local("off1a", ownerNearby = true, failedJoins = 9)))
    }

    @Test
    fun `a join on an owner alone is a paced probe, not every step`() {
        // Televisions are group owners too; joining on every step kept two
        // phones deaf to each other's records for minutes.
        val notDue = Local("off1a", app, inGroup = false, ownerNearby = true, ownerProbeDue = false)
        assertEquals(Action.Wait, WifiDirectGroupFormation.decide(notDue, emptyList()))
        // A record heard is not paced: that peer is real.
        assertEquals(Action.Join, WifiDirectGroupFormation.decide(notDue.copy(address = "off1b"), listOf(advert("off1a"))))
    }

    @Test
    fun `the group name needs nothing heard from its owner`() {
        // Joiner and creator derive one name from the application alone.
        assertEquals(
            WifiDirectGroupFormation.networkName("com.example.app"),
            WifiDirectGroupFormation.networkName("com.example.app"),
        )
        assertNotEquals(
            WifiDirectGroupFormation.networkName("com.example.app"),
            WifiDirectGroupFormation.networkName("com.other.app"),
        )
    }

    @Test
    fun `after dissolving its group a device joins before it may create again`() {
        // Two phones created at once, dissolved in step, and created again.
        val lowestAfterDissolve = Local("off1a", app, inGroup = false, joinFirst = true)
        assertEquals(Action.Join, WifiDirectGroupFormation.decide(lowestAfterDissolve, listOf(advert("off1b"))))
        assertEquals(Action.Create, WifiDirectGroupFormation.decide(lowestAfterDissolve.copy(joinFirst = false), listOf(advert("off1b"))))
    }

    // --- In a group ----------------------------------------------------------------

    @Test
    fun `a device in a group does nothing here`() {
        assertEquals(Action.Wait, decide(local("off1a", inGroup = true), "off1b"))
        assertEquals(Action.Wait, decide(local("off1z", inGroup = true, failedJoins = 9, ownerNearby = true), "off1a"))
    }

    // --- What is ignored -------------------------------------------------------------

    @Test
    fun `devices of another application are invisible`() {
        val other = WifiDirectGroupFormation.appTag("com.other.app")
        assertNotEquals(app, other)
        val adverts = listOf(advert("off1a", appTag = other))
        assertEquals(Action.Wait, WifiDirectGroupFormation.decide(local("off1z"), adverts))
    }

    @Test
    fun `a record naming this device's own address is not a peer`() {
        assertEquals(Action.Wait, decide(local("off1b"), "off1b"))
    }

    // --- The record --------------------------------------------------------------------

    @Test
    fun `the record starts with txtvers and carries the address, as the stream chapter requires`() {
        val txt = WifiDirectGroupFormation.txtRecord("off1q", app)
        assertEquals(listOf("txtvers", "addr", "app"), txt.keys.toList())
        assertEquals("1", txt["txtvers"])
        assertEquals("off1q", txt["addr"])
    }

    @Test
    fun `a record round-trips`() {
        val txt = WifiDirectGroupFormation.txtRecord("off1q", app)
        assertEquals(Advert("mac", "off1q", app, 7), WifiDirectGroupFormation.parse("mac", txt, 7))
    }

    @Test
    fun `records that are not peer records are refused`() {
        val good = WifiDirectGroupFormation.txtRecord("off1q", app)
        assertNull(WifiDirectGroupFormation.parse("mac", good + ("txtvers" to "2"), 0))
        assertNull(WifiDirectGroupFormation.parse("mac", good - "addr", 0))
        assertNull(WifiDirectGroupFormation.parse("mac", good + ("addr" to ""), 0))
        assertNull(WifiDirectGroupFormation.parse("mac", good - "app", 0))
        // A service instance (the DNS-SD mapping chapter) is never a peer.
        assertNull(WifiDirectGroupFormation.parse("mac", good + ("sid" to "printer"), 0))
    }

    // --- The credentials -------------------------------------------------------------

    @Test
    fun `the network name is one Android accepts`() {
        val name = WifiDirectGroupFormation.networkName("com.example.app")
        assertTrue(name, WifiDirectGroupFormation.isValidNetworkName(name))
        assertTrue(name.toByteArray().size <= 32)
    }

    @Test
    fun `the passphrase is WPA2-sized and the same on both ends`() {
        val name = WifiDirectGroupFormation.networkName("com.example.app")
        val owner = WifiDirectGroupFormation.passphrase("com.example.app", name)
        assertEquals(owner, WifiDirectGroupFormation.passphrase("com.example.app", name))
        assertTrue(owner.length in 8..63)
    }

    @Test
    fun `another application derives another passphrase`() {
        val name = WifiDirectGroupFormation.networkName("com.example.app")
        assertNotEquals(
            WifiDirectGroupFormation.passphrase("com.example.app", name),
            WifiDirectGroupFormation.passphrase("com.other.app", name),
        )
    }
}
