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
 * [WifiDirectGroupFormation]: which device of an application owns the Wi-Fi
 * Direct group, and the credentials it is formed and joined with. The manager's
 * radio calls need a device (docs/bridges C9); the decision every device makes
 * from the same records is pinned here, because two devices that disagree about
 * it form two groups that never meet.
 */
class WifiDirectGroupFormationTest {

    private val app = WifiDirectGroupFormation.appTag("com.example.app")

    private fun advert(address: String, owner: String? = null, appTag: String = app) =
        Advert(device = "mac-$address", address = address, app = appTag, ownerNetwork = owner, seenAtMs = 0)

    private fun local(
        address: String,
        inGroup: Boolean = false,
        isOwner: Boolean = false,
        clients: Int = 0,
    ) = Local(address = address, app = app, inGroup = inGroup, isOwner = isOwner, clients = clients)

    // --- Who creates -------------------------------------------------------

    @Test
    fun `alone, a device creates nothing`() {
        assertEquals(Action.Wait, WifiDirectGroupFormation.decide(local("off1b"), emptyList()))
    }

    @Test
    fun `of two devices with no group, the lower creates and the higher joins the group it will create`() {
        assertEquals(Action.Create, WifiDirectGroupFormation.decide(local("off1a"), listOf(advert("off1b"))))
        val lower = advert("off1a")
        assertEquals(
            Action.Join(WifiDirectGroupFormation.networkName("off1a"), lower),
            WifiDirectGroupFormation.decide(local("off1b"), listOf(lower)),
        )
    }

    @Test
    fun `the name the joiner expects is the name the owner creates`() {
        // The owner creates under networkName(its own address); the joiner,
        // which never hears the owner's record once it owns the group, joins
        // networkName(the address it heard before). They must be one string.
        val owner = "off1qxjee2yfec0twantxrm78pt9nzfhxjrr8q0p63fd"
        val joined = WifiDirectGroupFormation.decide(local("off1z"), listOf(advert(owner)))
        assertEquals(WifiDirectGroupFormation.networkName(owner), (joined as Action.Join).network)
    }

    @Test
    fun `the lowest of three creates, and the other two join it`() {
        val all = listOf("off1a", "off1b", "off1c")
        val actions = all.associateWith { me ->
            WifiDirectGroupFormation.decide(local(me), all.filter { it != me }.map { advert(it) })
        }
        assertEquals(Action.Create, actions["off1a"])
        val expected = WifiDirectGroupFormation.networkName("off1a")
        assertEquals(expected, (actions["off1b"] as Action.Join).network)
        assertEquals(expected, (actions["off1c"] as Action.Join).network)
    }

    // --- Who joins ---------------------------------------------------------

    @Test
    fun `a device in no group joins an owner it sees, whatever the addresses`() {
        val owner = advert("off1z", owner = "DIRECT-op-11111111")
        assertEquals(
            Action.Join("DIRECT-op-11111111", owner),
            WifiDirectGroupFormation.decide(local("off1a"), listOf(owner)),
        )
    }

    @Test
    fun `of two owners in sight, a newcomer joins the lower`() {
        val low = advert("off1b", owner = "DIRECT-op-b0000000")
        val high = advert("off1c", owner = "DIRECT-op-c0000000")
        assertEquals(
            Action.Join("DIRECT-op-b0000000", low),
            WifiDirectGroupFormation.decide(local("off1z"), listOf(high, low)),
        )
    }

    // --- Merging and staying -----------------------------------------------

    @Test
    fun `an empty owner that sees a lower owner moves to it`() {
        val lower = advert("off1a", owner = "DIRECT-op-a0000000")
        assertEquals(
            Action.Move("DIRECT-op-a0000000", lower),
            WifiDirectGroupFormation.decide(local("off1b", inGroup = true, isOwner = true), listOf(lower)),
        )
    }

    @Test
    fun `two empty owners that formed at once end in one group`() {
        val a = advert("off1a", owner = "DIRECT-op-a0000000")
        val b = advert("off1b", owner = "DIRECT-op-b0000000")
        val aAction = WifiDirectGroupFormation.decide(local("off1a", inGroup = true, isOwner = true), listOf(b))
        val bAction = WifiDirectGroupFormation.decide(local("off1b", inGroup = true, isOwner = true), listOf(a))
        assertEquals(Action.Wait, aAction)
        assertEquals(Action.Move("DIRECT-op-a0000000", a), bAction)
    }

    @Test
    fun `an owner with clients never moves`() {
        val lower = advert("off1a", owner = "DIRECT-op-a0000000")
        assertEquals(
            Action.Wait,
            WifiDirectGroupFormation.decide(local("off1b", inGroup = true, isOwner = true, clients = 1), listOf(lower)),
        )
    }

    @Test
    fun `a client never acts, including in a group someone else formed`() {
        val lower = advert("off1a", owner = "DIRECT-op-a0000000")
        assertEquals(
            Action.Wait,
            WifiDirectGroupFormation.decide(local("off1b", inGroup = true, isOwner = false), listOf(lower)),
        )
    }

    // --- What is ignored ---------------------------------------------------

    @Test
    fun `devices of another application are invisible`() {
        val other = WifiDirectGroupFormation.appTag("com.other.app")
        assertNotEquals(app, other)
        val adverts = listOf(advert("off1a", owner = "DIRECT-op-a0000000", appTag = other), advert("off1b", appTag = other))
        assertEquals(Action.Wait, WifiDirectGroupFormation.decide(local("off1z"), adverts))
    }

    @Test
    fun `a record naming this device's own address is not a peer`() {
        assertEquals(Action.Wait, WifiDirectGroupFormation.decide(local("off1b"), listOf(advert("off1b"))))
    }

    // --- The record ----------------------------------------------------------

    @Test
    fun `the record starts with txtvers and carries the address, as the stream chapter requires`() {
        val txt = WifiDirectGroupFormation.txtRecord("off1q", app, null)
        assertEquals(listOf("txtvers", "addr", "app"), txt.keys.toList())
        assertEquals("1", txt["txtvers"])
        assertEquals("off1q", txt["addr"])
        assertTrue("net" in WifiDirectGroupFormation.txtRecord("off1q", app, "DIRECT-op-12345678"))
    }

    @Test
    fun `a record round-trips, and an owner's network name comes back`() {
        val txt = WifiDirectGroupFormation.txtRecord("off1q", app, "DIRECT-op-12345678")
        assertEquals(
            Advert("mac", "off1q", app, "DIRECT-op-12345678", 7),
            WifiDirectGroupFormation.parse("mac", txt, 7),
        )
    }

    @Test
    fun `records that are not peer records are refused`() {
        val good = WifiDirectGroupFormation.txtRecord("off1q", app, null)
        assertNull(WifiDirectGroupFormation.parse("mac", good + ("txtvers" to "2"), 0))
        assertNull(WifiDirectGroupFormation.parse("mac", good - "addr", 0))
        assertNull(WifiDirectGroupFormation.parse("mac", good + ("addr" to ""), 0))
        assertNull(WifiDirectGroupFormation.parse("mac", good - "app", 0))
        // A service instance (the DNS-SD mapping chapter) is never a peer.
        assertNull(WifiDirectGroupFormation.parse("mac", good + ("sid" to "printer"), 0))
    }

    @Test
    fun `a network name Android would refuse is read as no group`() {
        val txt = WifiDirectGroupFormation.txtRecord("off1q", app, null) + ("net" to "not-a-direct-name")
        assertNull(WifiDirectGroupFormation.parse("mac", txt, 0)?.ownerNetwork)
    }

    // --- The credentials -----------------------------------------------------

    @Test
    fun `the network name is one Android accepts`() {
        val name = WifiDirectGroupFormation.networkName("off1qxjee2yfec0twantxrm78pt9nzfhxjrr8q0p63fd")
        assertTrue(name, WifiDirectGroupFormation.isValidNetworkName(name))
        assertTrue(name.toByteArray().size <= 32)
    }

    @Test
    fun `the passphrase is WPA2-sized and the same on both ends`() {
        val name = WifiDirectGroupFormation.networkName("off1a")
        val owner = WifiDirectGroupFormation.passphrase("com.example.app", name)
        val joiner = WifiDirectGroupFormation.passphrase("com.example.app", name)
        assertEquals(owner, joiner)
        assertTrue(owner.length in 8..63)
    }

    @Test
    fun `another application derives another passphrase for the same group`() {
        val name = WifiDirectGroupFormation.networkName("off1a")
        assertNotEquals(
            WifiDirectGroupFormation.passphrase("com.example.app", name),
            WifiDirectGroupFormation.passphrase("com.other.app", name),
        )
    }

    @Test
    fun `different owners get different network names`() {
        assertNotEquals(
            WifiDirectGroupFormation.networkName("off1a"),
            WifiDirectGroupFormation.networkName("off1b"),
        )
    }
}
