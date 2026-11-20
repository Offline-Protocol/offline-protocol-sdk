package com.offlineprotocol

import java.net.InetAddress
import java.net.NetworkInterface

/**
 * Where a WebRTC Direct listener (ADR 0039) binds: the rule, kept apart from
 * the framework so it is tested on plain lists (docs/bridges/kotlin.md K10).
 *
 * The listener binds one private IPv4 address that the
 * pages meant to dial it can reach, and never an address of a network that
 * is not a local one. In order:
 *
 * 1. **The Wi-Fi network this device is a client of**, when it gives the
 *    device such an address. It comes first because it is the network the
 *    LAN carrier follows and the one a socket can be bound to as a
 *    `Network` (ADR 0028), so nothing the listener sends follows the default
 *    network onto cellular.
 * 2. **This device's own hotspot** (a tethering interface: Wi-Fi hotspot,
 *    USB or Bluetooth tethering), when it is not a Wi-Fi client: the common
 *    case with no internet, where a laptop or another phone joins this
 *    phone's hotspot and its browser dials the application here. It is told
 *    apart from cellular, a VPN or the Wi-Fi client by the connectivity
 *    service, with no privileged `TetheringManager`: those are each a
 *    `Network` that is not a local network, and a tethering interface is
 *    either no `Network` at all (before Android 15) or a local one
 *    (`NET_CAPABILITY_LOCAL_NETWORK`, which Android 16 on a Samsung gives its
 *    hotspot). So an interface of any `Network` that is not local is never
 *    chosen. Binding a VPN's or the cellular interface's private address
 *    instead would show a code no page nearby can reach, and would answer
 *    checks from a network the code was never shown on.
 *
 * A Wi-Fi Direct group interface (`p2p*`) is no `Network` either, and is
 * never chosen: its group belongs to the Wi-Fi Direct carrier or to a fast
 * link (ADR 0038), whose host admits only its invitee, and a dialled stream
 * counts as a LAN stream, so a listener there would let anyone on that group
 * past the fast link's admission.
 */
internal object WebrtcDirectAddress {

    /** One interface, as the choice sees it. */
    class Interface(
        val name: String,
        val up: Boolean,
        val loopback: Boolean,
        val addresses: List<InetAddress>,
    )

    /** A `Network` the connectivity service knows, by its interface. */
    class KnownNetwork(val interfaceName: String, val local: Boolean)

    /** What a listener binds. */
    sealed class Choice {
        abstract val ip: InetAddress

        /** The Wi-Fi network this device is a client of: bind the socket to that `Network` too. */
        class WifiClient(override val ip: InetAddress) : Choice()

        /** This device's own hotspot: there is no `Network`, so the address alone. */
        class Hotspot(override val ip: InetAddress, val interfaceName: String) : Choice()
    }

    /**
     * The address a listener binds, by the rule above. [wifiClient] holds the
     * Wi-Fi client network's own addresses, or null when there is none;
     * [interfaces] reads every interface on the device and [networks] every
     * `Network` the connectivity service knows, each only when the Wi-Fi
     * network gave no address. Throws [IllegalStateException] when there is
     * no such address.
     */
    fun choose(
        wifiClient: List<InetAddress>?,
        interfaces: () -> List<Interface>,
        networks: () -> List<KnownNetwork>,
    ): Choice {
        wifiClient?.let(WebrtcDirectListener::listenAddress)?.let { return Choice.WifiClient(it) }
        val known = networks().filterNot { it.local }.map { it.interfaceName }.toSet()
        for (candidate in interfaces()) {
            if (!candidate.up || candidate.loopback) continue
            if (candidate.name in known || candidate.name.startsWith("p2p")) continue
            val ip = WebrtcDirectListener.listenAddress(candidate.addresses) ?: continue
            return Choice.Hotspot(ip, candidate.name)
        }
        throw IllegalStateException(
            if (wifiClient != null) {
                "the Wi-Fi network gave this device no private IPv4 address, and it shares no hotspot"
            } else {
                "not on a Wi-Fi network and sharing no hotspot: a WebRTC Direct listener binds " +
                    "a private IPv4 address of one or the other"
            },
        )
    }

    /**
     * Whether a hotspot listener's [ip] is still this device's on [hotspot]
     * (the interface by its name now, or null when it is gone). False once
     * the interface is down or the address went, when the code names an
     * address no page can reach.
     */
    fun holds(hotspot: Interface?, ip: InetAddress): Boolean =
        hotspot != null && hotspot.up && ip in hotspot.addresses

    /** Every interface on this device, or none when they cannot be read. */
    fun interfaces(): List<Interface> = try {
        NetworkInterface.getNetworkInterfaces()?.toList().orEmpty().mapNotNull(::read)
    } catch (_: Exception) {
        emptyList()
    }

    /** The interface named [name] now, or null when there is none. */
    fun interfaceNamed(name: String): Interface? = try {
        NetworkInterface.getByName(name)?.let(::read)
    } catch (_: Exception) {
        null
    }

    private fun read(nif: NetworkInterface): Interface? = try {
        Interface(nif.name, nif.isUp, nif.isLoopback, nif.inetAddresses.toList())
    } catch (_: Exception) {
        // Gone between the listing and the read.
        null
    }
}
