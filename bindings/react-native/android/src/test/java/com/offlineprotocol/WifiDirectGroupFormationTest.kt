package com.offlineprotocol

import com.offlineprotocol.WifiDirectGroupFormation.Action
import com.offlineprotocol.WifiDirectGroupFormation.Advert
import com.offlineprotocol.WifiDirectGroupFormation.Local
import org.junit.Assert.assertEquals
import org.junit.Assert.assertFalse
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

    @Test
    fun `a record with an entry this version does not know still parses`() {
        // A later revision may add entries under txtvers=1; ignoring them is
        // what lets it.
        val txt = WifiDirectGroupFormation.txtRecord("off1q", app) + ("future" to "x")
        assertEquals(Advert("mac", "off1q", app, 0), WifiDirectGroupFormation.parse("mac", txt, 0))
    }

    // --- The credentials -------------------------------------------------------------

    @Test
    fun `the network name is one Android accepts`() {
        val name = WifiDirectGroupFormation.networkName("com.example.app")
        assertTrue(name, WifiDirectGroupFormation.isValidNetworkName(name))
        assertTrue(name.toByteArray().size <= 32)
    }

    @Test
    fun `names Android refuses are refused`() {
        assertFalse(WifiDirectGroupFormation.isValidNetworkName("op-1234"))
        assertFalse(WifiDirectGroupFormation.isValidNetworkName("DIRECT-"))
        assertFalse(WifiDirectGroupFormation.isValidNetworkName("DIRECT-op-" + "a".repeat(23)))
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

    // --- Quiet surroundings ------------------------------------------------------

    @Test
    fun `discovery backs off while nothing is heard, to a ceiling`() {
        assertEquals(15_000L, WifiDirectGroupFormation.discoveryPeriodMs(0))
        assertEquals(30_000L, WifiDirectGroupFormation.discoveryPeriodMs(1))
        assertEquals(60_000L, WifiDirectGroupFormation.discoveryPeriodMs(2))
        assertEquals(60_000L, WifiDirectGroupFormation.discoveryPeriodMs(50))
    }

    @Test
    fun `probes toward an owner nobody vouched for back off, to a ceiling`() {
        // A printer or television owns a group no probe ever joins.
        assertEquals(60_000L, WifiDirectGroupFormation.probePeriodMs(0))
        assertEquals(120_000L, WifiDirectGroupFormation.probePeriodMs(1))
        assertEquals(240_000L, WifiDirectGroupFormation.probePeriodMs(2))
        assertEquals(240_000L, WifiDirectGroupFormation.probePeriodMs(1_000))
        assertEquals(60_000L, WifiDirectGroupFormation.probePeriodMs(-1))
    }

    // --- An owner that was left ---------------------------------------------------

    @Test
    fun `a join that lands on an owner this device left is recognised`() {
        // Every group of an application has one name, so a join by name landed
        // on the owner just left, ten seconds after leaving it.
        assertTrue(WifiDirectGroupFormation.joinedLeftOwner(true, "aa:01", avoided = setOf("aa:01")))
        assertFalse(WifiDirectGroupFormation.joinedLeftOwner(true, "aa:02", avoided = setOf("aa:01")))
        assertFalse(WifiDirectGroupFormation.joinedLeftOwner(true, null, avoided = setOf("aa:01")))
        // A group paired in the system settings is never left.
        assertFalse(WifiDirectGroupFormation.joinedLeftOwner(false, "aa:01", avoided = setOf("aa:01")))
    }

    @Test
    fun `captures by a dead owner add up to the takeover`() {
        // A join cannot be steered away from the dead owner (the supplicant
        // pins a named join to an interface address no application sees), so
        // each capture is left at once and counted. Resetting the count on
        // joining, as any other group does, never reached the takeover.
        var failed = 0
        repeat(WifiDirectGroupFormation.TAKEOVER_FAILED_JOINS) {
            failed = WifiDirectGroupFormation.failedJoinsAfterGroupJoined(failed, leftOwner = true)
        }
        assertEquals(WifiDirectGroupFormation.TAKEOVER_FAILED_JOINS, failed)
        // decide() does not count the dead owner as nearby, so the device that
        // heard a peer creates.
        assertEquals(Action.Create, decide(local("off1b", failedJoins = failed), "off1a"))
        // Any other group starts the count over.
        assertEquals(0, WifiDirectGroupFormation.failedJoinsAfterGroupJoined(failed, leftOwner = false))
    }

    @Test
    fun `a device that heard nobody does not probe an owner it left`() {
        assertEquals(Action.Wait, decide(local("off1b", ownerNearby = false)))
    }

    @Test
    fun `an owner counts as new only after it was out of sight a while`() {
        // A television at the edge of range flickers in and out of the peer
        // list; each return re-armed the fast probe.
        val m = WifiDirectGroupFormation.OWNER_MEMORY_MS
        val seen = mapOf("tv" to 1_000L)
        assertEquals(emptySet<String>(), WifiDirectGroupFormation.ownersNewlySeen(setOf("tv"), seen, 1_000L + m - 1))
        assertEquals(setOf("tv"), WifiDirectGroupFormation.ownersNewlySeen(setOf("tv"), seen, 1_000L + m))
        assertEquals(setOf("new"), WifiDirectGroupFormation.ownersNewlySeen(setOf("tv", "new"), seen, 2_000L))
    }

    @Test
    fun `a dead owner is avoided for longer than a revived owner takes to dissolve`() {
        // An owner whose application comes back dissolves its empty group in
        // 30 to 60 s; avoiding it no longer than that would rejoin it.
        assertTrue(WifiDirectGroupFormation.DEAD_OWNER_TTL_MS > 2 * 30_000L)
    }

    // --- The switch ----------------------------------------------------------------

    @Test
    fun `an enable that does not name autoAccept keeps the current setting`() {
        // enableTransport('wifiDirect') after a permission grant passes no config.
        assertTrue(WifiDirectGroupFormation.formGroupsAfterEnable(current = true, configured = null))
        assertEquals(false, WifiDirectGroupFormation.formGroupsAfterEnable(current = false, configured = null))
        assertEquals(false, WifiDirectGroupFormation.formGroupsAfterEnable(current = true, configured = false))
        assertTrue(WifiDirectGroupFormation.formGroupsAfterEnable(current = false, configured = true))
    }
}
