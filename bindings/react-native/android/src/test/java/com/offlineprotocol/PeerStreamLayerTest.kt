package com.offlineprotocol

import org.junit.Assert.assertEquals
import org.junit.Assert.assertTrue
import org.junit.Test
import java.util.Collections

/**
 * The peer-stream layer is up while a stream can be established, an open
 * WebRTC Direct listener included, and goes down only after the streams of
 * whatever went were reported lost ([PeerStreamLayer], docs/bridges/kotlin.md
 * K10).
 *
 * Found on a phone sharing its hotspot with Wi-Fi P2P off and on no Wi-Fi
 * network: a page dialled its listener and proved its address, the phone
 * counted it nearby, and no MLS session ever formed, because the layer was
 * reported up only for Wi-Fi P2P or the LAN and the core holds a link usable
 * only while the layer is up.
 *
 * The streams are [PeerStreamSockets]'s real table with a listener that
 * records nothing; the "manager" here does what [WifiDirectManager] does: the
 * listener is open while its field is set, and a close clears the field and
 * ends the dialled streams inside [PeerStreamLayer.afterEnding].
 */
class PeerStreamLayerTest {

    /** Every loss and every layer report, in the order they reached the core. */
    private val core = Collections.synchronizedList(mutableListOf<String>())

    private val host = object : PeerStreamSockets.Host {
        override fun identityAssertion(): ByteArray = ByteArray(PeerStreamFraming.PREAMBLE_MIN_BYTES)
        override fun localAddress(): String = "peer-a"
        override fun verify(assertion: ByteArray): String = throw IllegalStateException("unused")
        override fun peerConnected(address: String) { core.add("connected:$address") }
        override fun messageReceived(address: String, body: ByteArray) {}
        override fun peerDisconnected(address: String) { core.add("lost:$address") }
        override fun diagnostic(level: String, message: String, context: Map<String, Any?>) {}
    }

    private val dials = object : PeerStreamSockets.Dials {
        override fun send(stream: Long, body: ByteArray) {}
        override fun close(stream: Long) {}
    }

    private val sockets = PeerStreamSockets(host, PeerStreamSockets.Limits())

    private var p2p = false
    private var lan = false
    private var listening = false

    private val layer = PeerStreamLayer(
        carriers = { PeerStreamLayer.Carriers(p2p, lan, listening) },
        report = { up -> core.add("layer:$up") },
    )

    private fun openListener() {
        listening = true
        layer.reportEdge()
    }

    private fun closeListener() = layer.afterEnding {
        listening = false
        sockets.closeDials()
    }

    private fun prove(stream: Long, address: String) {
        assertTrue(sockets.dialProved(dials, stream, address, "10.228.161.${stream + 1}", accepting = { true }))
    }

    @Test
    fun `an open listener holds the layer up when no carrier does`() {
        layer.reportEdge()
        assertEquals(emptyList<String>(), core.toList())

        openListener()
        assertEquals(listOf("layer:true"), core.toList())

        prove(1, "peer-m")
        prove(2, "peer-n")
        core.clear()

        closeListener()
        // Every dialled peer is reported lost while the core still holds the
        // layer, and the layer goes down after, once.
        assertEquals(setOf("lost:peer-m", "lost:peer-n"), core.take(2).toSet())
        assertEquals(listOf("layer:false"), core.drop(2))

        // A second close has nothing to end and no edge to report.
        core.clear()
        closeListener()
        assertEquals(emptyList<String>(), core.toList())
    }

    @Test
    fun `with the LAN up a listener opening and closing reports no edge`() {
        lan = true
        layer.reportEdge()
        assertEquals(listOf("layer:true"), core.toList())
        core.clear()

        openListener()
        prove(1, "peer-m")
        closeListener()
        assertEquals(listOf("connected:peer-m", "lost:peer-m"), core.toList())

        // P2P likewise holds it up.
        core.clear()
        lan = false
        p2p = true
        openListener()
        closeListener()
        assertEquals(emptyList<String>(), core.toList())
    }

    @Test
    fun `the LAN going leaves the layer up while the listener is open`() {
        lan = true
        layer.reportEdge()
        openListener()
        prove(1, "peer-m")
        core.clear()

        layer.afterEnding { lan = false }
        assertEquals(emptyList<String>(), core.toList())

        closeListener()
        assertEquals(listOf("lost:peer-m", "layer:false"), core.toList())
    }

    @Test
    fun `after a stop the next start reports the layer from down`() {
        openListener()
        assertEquals(listOf("layer:true"), core.toList())
        // Stop closes the listener without a report and tells the core itself.
        listening = false
        sockets.closeDials()
        layer.stopped()
        assertEquals(false, layer.reported)
        core.clear()

        openListener()
        assertEquals(listOf("layer:true"), core.toList())
    }
}
