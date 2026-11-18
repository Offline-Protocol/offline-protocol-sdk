package com.offlineprotocol

import org.junit.Assert.assertEquals
import org.junit.Assert.assertFalse
import org.junit.Assert.assertTrue
import org.junit.Assert.fail
import org.junit.Test
import java.net.InetAddress

/**
 * Where a WebRTC Direct listener binds (ADR 0039, docs/bridges/kotlin.md
 * K10): the Wi-Fi client network's private address first, else this
 * device's own hotspot, told apart from cellular, a VPN and the Wi-Fi client
 * by being no `Network` that is not a local one, and never a Wi-Fi Direct
 * group.
 */
class WebrtcDirectAddressTest {

    private fun a(s: String): InetAddress = InetAddress.getByName(s)

    private fun nif(name: String, vararg addresses: String, up: Boolean = true, loopback: Boolean = false) =
        WebrtcDirectAddress.Interface(name, up, loopback, addresses.map(::a))

    // A device sharing its hotspot with cellular and a VPN up, as a phone
    // with a SIM has: every one of these holds a private IPv4 address.
    private val device = listOf(
        nif("lo", "127.0.0.1", loopback = true),
        nif("rmnet_data0", "10.64.12.7"),
        nif("tun0", "10.8.0.2"),
        nif("p2p-wlan0-0", "192.168.49.1"),
        nif("swlan0", "fe80::1", "192.168.43.1"),
    )
    private val networks = setOf("rmnet_data0", "tun0")
    private fun nonLocal(names: Set<String>) = names.map { WebrtcDirectAddress.KnownNetwork(it, local = false) }

    private fun choose(
        wifi: List<String>?,
        interfaces: List<WebrtcDirectAddress.Interface> = device,
        known: Set<String> = networks,
        local: Set<String> = emptySet(),
    ) = WebrtcDirectAddress.choose(
        wifi?.map(::a),
        { interfaces },
        { nonLocal(known) + local.map { WebrtcDirectAddress.KnownNetwork(it, local = true) } },
    )

    @Test
    fun `the Wi-Fi client network wins when it gives a private address`() {
        val choice = choose(listOf("fe80::2", "192.168.1.5"))
        assertTrue(choice is WebrtcDirectAddress.Choice.WifiClient)
        assertEquals(a("192.168.1.5"), choice.ip)
    }

    @Test
    fun `the interfaces are not read when the Wi-Fi client network decides`() {
        val choice = WebrtcDirectAddress.choose(
            listOf(a("192.168.1.5")),
            { fail("interfaces read"); emptyList() },
            { fail("networks read"); emptyList() },
        )
        assertEquals(a("192.168.1.5"), choice.ip)
    }

    @Test
    fun `with no Wi-Fi client network the hotspot interface is chosen`() {
        val choice = choose(null)
        assertTrue(choice is WebrtcDirectAddress.Choice.Hotspot)
        assertEquals(a("192.168.43.1"), choice.ip)
        assertEquals("swlan0", (choice as WebrtcDirectAddress.Choice.Hotspot).interfaceName)
    }

    @Test
    fun `a hotspot the system reports as a local Network is chosen`() {
        // Android 15 and later: the tethering downstream is a Network with
        // NET_CAPABILITY_LOCAL_NETWORK (Android 16 on a Samsung, swlan0).
        val choice = choose(null, local = setOf("swlan0"))
        assertEquals("swlan0", (choice as WebrtcDirectAddress.Choice.Hotspot).interfaceName)
        // An interface with a Network that is not local stays out, even if a local one names it too.
        assertThrowsClearly(interfaces = device.filter { it.name == "swlan0" }, known = setOf("swlan0"), local = setOf("swlan0"))
    }

    @Test
    fun `a Wi-Fi client network with no private address falls back to the hotspot`() {
        val choice = choose(listOf("fe80::2", "203.0.113.9"))
        assertEquals("swlan0", (choice as WebrtcDirectAddress.Choice.Hotspot).interfaceName)
    }

    @Test
    fun `an interface that belongs to a known Network is never chosen`() {
        // Listed first, each would win on order alone.
        for (name in networks) {
            val interfaces = listOf(nif(name, "10.1.2.3")) + device.filter { it.name != name }
            assertEquals("swlan0", (choose(null, interfaces) as WebrtcDirectAddress.Choice.Hotspot).interfaceName)
        }
        // With only Networks' interfaces, nothing is chosen.
        assertThrowsClearly(interfaces = device.filter { it.name in networks })
    }

    @Test
    fun `a Wi-Fi Direct group, a down interface, loopback and public addresses are never chosen`() {
        assertThrowsClearly(
            interfaces = listOf(
                nif("p2p-wlan0-0", "192.168.49.1"),
                nif("p2p0", "192.168.49.2"),
                nif("ap0", "192.168.43.1", up = false),
                nif("lo", "127.0.0.1", loopback = true),
                nif("wlan1", "198.51.100.4", "fe80::3"),
            ),
            known = emptySet(),
        )
    }

    @Test
    fun `a link-local hotspot address is taken when it has no private one`() {
        val choice = choose(null, listOf(nif("rndis0", "fe80::9", "169.254.10.1")), emptySet())
        assertEquals(a("169.254.10.1"), choice.ip)
    }

    @Test
    fun `no private address anywhere throws a clear error`() {
        assertThrowsClearly(wifi = null, interfaces = device.take(1))
        val e = try {
            choose(listOf("203.0.113.9"), device.take(1))
            null
        } catch (e: IllegalStateException) {
            e
        }
        assertTrue(e?.message.orEmpty().contains("no private IPv4 address"))
    }

    @Test
    fun `a hotspot listener holds while its interface is up with its address`() {
        val ip = a("192.168.43.1")
        assertTrue(WebrtcDirectAddress.holds(nif("swlan0", "192.168.43.1"), ip))
        assertFalse(WebrtcDirectAddress.holds(null, ip))
        assertFalse(WebrtcDirectAddress.holds(nif("swlan0", "192.168.43.1", up = false), ip))
        // Restarted on a new subnet: the code names an address that went.
        assertFalse(WebrtcDirectAddress.holds(nif("swlan0", "192.168.205.1"), ip))
    }

    private fun assertThrowsClearly(
        wifi: List<String>? = null,
        interfaces: List<WebrtcDirectAddress.Interface>,
        known: Set<String> = networks,
        local: Set<String> = emptySet(),
    ) {
        try {
            val choice = choose(wifi, interfaces, known, local)
            fail("chose ${choice.ip}")
        } catch (e: IllegalStateException) {
            assertTrue(e.message, e.message.orEmpty().contains("hotspot"))
        }
    }
}
