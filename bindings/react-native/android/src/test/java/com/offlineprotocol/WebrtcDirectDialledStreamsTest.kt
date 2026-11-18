package com.offlineprotocol

import org.junit.After
import org.junit.Assert.assertArrayEquals
import org.junit.Assert.assertEquals
import org.junit.Assert.assertFalse
import org.junit.Assert.assertNull
import org.junit.Assert.assertTrue
import org.junit.Test
import java.io.DataInputStream
import java.io.IOException
import java.net.InetAddress
import java.net.InetSocketAddress
import java.net.ServerSocket
import java.net.Socket
import java.net.SocketTimeoutException
import java.util.Collections
import java.util.concurrent.LinkedBlockingQueue
import java.util.concurrent.TimeUnit
import kotlin.concurrent.thread

/**
 * Streams a page dialled to the WebRTC Direct listener, as rows in
 * [PeerStreamSockets]'s one table (ADR 0039, stream-framing.md "What a
 * receiver owes"; docs/bridges/kotlin.md K10).
 *
 * The listener itself (admission, DTLS, the channel, the preamble) is the
 * engine's and is pinned in Rust. What this bridge owes is the table in front
 * of it: one announcement and one loss per address whichever kind carries
 * it, the newer of two dialled streams wins, a TCP stream keeps its address
 * against a dial and the other way round, a dial counts as inbound from its
 * page's address under the same caps, and a dial proving this device's own
 * address is refused. The listener is a fake that records what the table
 * asks of it; the TCP streams are real loopback sockets.
 *
 * Each address pair is chosen so the lower-address rule alone would decide
 * the other way, so a table that forgot the dialled rule fails here.
 */
class WebrtcDirectDialledStreamsTest {

    private val cleanup = Collections.synchronizedList(mutableListOf<AutoCloseable>())

    @After
    fun tearDown() {
        for (c in cleanup) try { c.close() } catch (_: Exception) {}
    }

    private class Host(private val me: String) : PeerStreamSockets.Host {
        val events = LinkedBlockingQueue<String>()
        val bodies = LinkedBlockingQueue<Pair<String, ByteArray>>()

        override fun identityAssertion(): ByteArray = assertion(me)
        override fun localAddress(): String = me
        override fun verify(assertion: ByteArray): String {
            val name = assertion.takeWhile { it != 0.toByte() }.toByteArray()
            val address = String(name, Charsets.US_ASCII)
            if (!Regex("peer-[a-z0-9-]+").matches(address)) throw IllegalStateException("signature")
            return address
        }
        override fun peerConnected(address: String) { events.add("connected:$address") }
        override fun messageReceived(address: String, body: ByteArray) {
            events.add("message:$address:${body.size}")
            bodies.add(address to body)
        }
        override fun peerDisconnected(address: String) { events.add("lost:$address") }
        override fun diagnostic(level: String, message: String, context: Map<String, Any?>) {}

        fun next(ms: Long = 5_000): String? = events.poll(ms, TimeUnit.MILLISECONDS)
        fun quiet(ms: Long = 300): String? = events.poll(ms, TimeUnit.MILLISECONDS)
    }

    /**
     * The listener, as the table sees it. Like the real one, a stream the
     * table closes is reported closed afterwards (by [reportClosed], which a
     * test calls where the listener's pump would), and a send to a stream
     * that is not open is refused.
     */
    private class FakeDials : PeerStreamSockets.Dials {
        val open = Collections.synchronizedSet(mutableSetOf<Long>())
        val closed = Collections.synchronizedList(mutableListOf<Long>())
        val sent = Collections.synchronizedList(mutableListOf<Pair<Long, ByteArray>>())

        override fun send(stream: Long, body: ByteArray) {
            if (stream !in open) throw IllegalStateException("no such stream")
            sent.add(stream to body)
        }

        override fun close(stream: Long) {
            closed.add(stream)
        }

        fun prove(sockets: PeerStreamSockets, stream: Long, address: String, ip: String = PAGE_IP): Boolean {
            open.add(stream)
            return sockets.dialProved(this, stream, address, ip, accepting = true)
        }

        /** What the listener does after the table closed streams: a Closed for each. */
        fun reportClosed(sockets: PeerStreamSockets) {
            for (stream in closed.toList()) {
                if (open.remove(stream)) sockets.dialClosed(stream)
            }
        }
    }

    companion object {
        const val PAGE_IP = "192.168.1.20"

        fun assertion(address: String): ByteArray =
            address.toByteArray().copyOf(PeerStreamFraming.PREAMBLE_MIN_BYTES)
    }

    private val fast = PeerStreamSockets.Limits(preambleTimeoutMs = 1_000)

    private fun listen(sockets: PeerStreamSockets): Int {
        val server = ServerSocket(0, 50, InetAddress.getLoopbackAddress())
        cleanup.add(server)
        thread(isDaemon = true) {
            while (!server.isClosed) {
                val s = try { server.accept() } catch (_: IOException) { return@thread }
                cleanup.add(s)
                thread(isDaemon = true) {
                    sockets.run(s, outbound = false, PeerStreamSockets.Carrier.LAN) { true }
                }
            }
        }
        return server.localPort
    }

    /** An inbound TCP stream from loopback that proves [address]. */
    private fun tcpPeer(port: Int, address: String): Socket {
        val s = Socket()
        s.connect(InetSocketAddress(InetAddress.getLoopbackAddress(), port), 2_000)
        s.soTimeout = 5_000
        cleanup.add(s)
        PeerStreamFraming.readBody(DataInputStream(s.getInputStream()), preamble = true)
        s.getOutputStream().write(PeerStreamFraming.frame(assertion(address)))
        s.getOutputStream().flush()
        return s
    }

    private fun Socket.closedByPeer(): Boolean {
        val buffer = ByteArray(4096)
        return try {
            while (true) {
                if (getInputStream().read(buffer) < 0) return true
            }
            @Suppress("UNREACHABLE_CODE") false
        } catch (_: SocketTimeoutException) {
            false
        } catch (_: IOException) {
            true
        }
    }

    // --- one dialled stream ---------------------------------------------------------

    @Test
    fun `a dialled stream is announced once, carries both ways, and is lost once`() {
        val host = Host("peer-a")
        val sockets = PeerStreamSockets(host, fast)
        val dials = FakeDials()

        assertTrue(dials.prove(sockets, 1, "peer-m"))
        assertEquals("connected:peer-m", host.next())
        assertTrue(sockets.holds("peer-m"))

        sockets.dialBody(1, "hello".toByteArray())
        assertEquals("message:peer-m:5", host.next())

        // Outbound bodies for the address go to the listener, unframed: it frames.
        assertEquals(PeerStreamSockets.SendResult.QUEUED, sockets.send("peer-m", "hi".toByteArray()))
        assertEquals(1L, dials.sent.single().first)
        assertArrayEquals("hi".toByteArray(), dials.sent.single().second)
        assertEquals(
            PeerStreamSockets.SendResult.OUT_OF_BOUNDS,
            sockets.send("peer-m", ByteArray(PeerStreamFraming.MAX_BODY_BYTES + 1)),
        )

        // The page went: the listener reports it, once.
        dials.open.remove(1L)
        sockets.dialClosed(1)
        assertEquals("lost:peer-m", host.next())
        sockets.dialClosed(1)
        assertNull(host.quiet())
        assertFalse(sockets.holds("peer-m"))
        // Never told to close a stream it already reported closed.
        assertTrue(dials.closed.isEmpty())
    }

    @Test
    fun `a refused send drops the body and leaves the stream to the listener`() {
        val host = Host("peer-a")
        val sockets = PeerStreamSockets(host, fast)
        val dials = FakeDials()
        assertTrue(dials.prove(sockets, 1, "peer-m"))
        assertEquals("connected:peer-m", host.next())
        dials.open.remove(1L)
        assertEquals(PeerStreamSockets.SendResult.QUEUE_FULL, sockets.send("peer-m", "x".toByteArray()))
        assertNull(host.quiet())
    }

    // --- one stream per address -----------------------------------------------------

    @Test
    fun `the newer of two dialled streams wins with one announcement and no loss`() {
        // We are the lower address: under the lower-address rule alone a
        // second inbound stream from peer-m would be refused, and a page that
        // reloaded would wait out the dead stream's consent.
        val host = Host("peer-a")
        val sockets = PeerStreamSockets(host, fast)
        val dials = FakeDials()

        assertTrue(dials.prove(sockets, 1, "peer-m"))
        assertEquals("connected:peer-m", host.next())

        assertTrue(dials.prove(sockets, 2, "peer-m"))
        // The older is closed through the listener, and the core hears
        // neither a second announcement nor a loss.
        assertEquals(listOf(1L), dials.closed.toList())
        dials.reportClosed(sockets)
        assertNull(host.quiet())

        // The older says nothing for the address any more; the newer does.
        sockets.dialBody(1, "stale".toByteArray())
        sockets.dialBody(2, "fresh".toByteArray())
        assertEquals("message:peer-m:5", host.next())
        assertEquals("fresh", String(host.bodies.poll(1, TimeUnit.SECONDS)!!.second))
        assertNull(host.quiet())

        assertEquals(PeerStreamSockets.SendResult.QUEUED, sockets.send("peer-m", "to".toByteArray()))
        assertEquals(2L, dials.sent.single().first)

        dials.open.remove(2L)
        sockets.dialClosed(2)
        assertEquals("lost:peer-m", host.next())
        assertNull(host.quiet())
        assertEquals(0, sockets.openCount)
    }

    @Test
    fun `a TCP stream keeps its address against a dial`() {
        // We are the higher address: under the lower-address rule alone an
        // inbound stream from peer-m would supersede the TCP one.
        val host = Host("peer-z")
        val sockets = PeerStreamSockets(host, fast)
        val tcp = tcpPeer(listen(sockets), "peer-m")
        assertEquals("connected:peer-m", host.next())

        val dials = FakeDials()
        assertFalse(dials.prove(sockets, 7, "peer-m"))
        assertEquals(listOf(7L), dials.closed.toList())
        dials.reportClosed(sockets)
        assertNull(host.quiet())

        // The TCP stream still carries the address.
        sockets.dialBody(7, "nope".toByteArray())
        tcp.getOutputStream().write(PeerStreamFraming.frame("still here".toByteArray()))
        tcp.getOutputStream().flush()
        assertEquals("message:peer-m:10", host.next())
        assertEquals(PeerStreamSockets.SendResult.QUEUED, sockets.send("peer-m", "x".toByteArray()))
        assertTrue(dials.sent.isEmpty())

        tcp.close()
        assertEquals("lost:peer-m", host.next())
        assertNull(host.quiet())
    }

    @Test
    fun `a dial keeps its address against a TCP stream`() {
        // Higher again, so an inbound TCP stream would supersede by the
        // lower-address rule, and must not take the address from a page.
        val host = Host("peer-z")
        val sockets = PeerStreamSockets(host, fast)
        val dials = FakeDials()
        assertTrue(dials.prove(sockets, 1, "peer-m"))
        assertEquals("connected:peer-m", host.next())

        val tcp = tcpPeer(listen(sockets), "peer-m")
        assertTrue(tcp.closedByPeer())
        assertNull(host.quiet())
        assertTrue(dials.closed.isEmpty())

        assertEquals(PeerStreamSockets.SendResult.QUEUED, sockets.send("peer-m", "x".toByteArray()))
        assertEquals(1L, dials.sent.single().first)
    }

    @Test
    fun `a dial that proves this device's own address is refused`() {
        val host = Host("peer-a")
        val sockets = PeerStreamSockets(host, fast)
        val dials = FakeDials()
        assertFalse(dials.prove(sockets, 1, "peer-a"))
        assertEquals(listOf(1L), dials.closed.toList())
        dials.reportClosed(sockets)
        assertNull(host.quiet())
        assertEquals(0, sockets.openCount)
    }

    // --- caps ---------------------------------------------------------------------------

    @Test
    fun `one page address holds at most its inbound share, and a reload at the cap is admitted`() {
        val host = Host("peer-a")
        val sockets = PeerStreamSockets(host, PeerStreamSockets.Limits(maxInboundPerHost = 4))
        val dials = FakeDials()
        for (i in 1..4) {
            assertTrue(dials.prove(sockets, i.toLong(), "peer-m$i"))
            assertEquals("connected:peer-m$i", host.next())
        }
        // A fifth address from the same page address is past the cap.
        assertFalse(dials.prove(sockets, 5, "peer-m5"))
        assertEquals(listOf(5L), dials.closed.toList())
        assertNull(host.quiet())
        // Another address of the network is not.
        assertTrue(dials.prove(sockets, 6, "peer-m6", ip = "192.168.1.21"))
        assertEquals("connected:peer-m6", host.next())

        // One of the four reloads: its new dial replaces its old stream, so
        // the cap does not count the old one against it.
        assertTrue(dials.prove(sockets, 7, "peer-m4"))
        assertEquals(listOf(5L, 4L), dials.closed.toList())
        dials.reportClosed(sockets)
        assertNull(host.quiet())
        assertEquals(5, sockets.openCount)
    }

    @Test
    fun `dials count against the inbound share and the stream limit`() {
        val host = Host("peer-a")
        val sockets = PeerStreamSockets(host, PeerStreamSockets.Limits(maxStreams = 16, maxInbound = 12))
        val dials = FakeDials()
        for (i in 1..12) {
            assertTrue(dials.prove(sockets, i.toLong(), "peer-m$i", ip = "192.168.1.${100 + i}"))
            assertEquals("connected:peer-m$i", host.next())
        }
        assertFalse(dials.prove(sockets, 13, "peer-m13", ip = "192.168.1.200"))
        assertEquals(listOf(13L), dials.closed.toList())
        assertNull(host.quiet())
        assertEquals(12, sockets.openCount)
    }

    @Test
    fun `TCP streams and dials from one address share its cap`() {
        val host = Host("peer-a")
        val sockets = PeerStreamSockets(host, PeerStreamSockets.Limits(preambleTimeoutMs = 2_000, maxInboundPerHost = 4))
        val port = listen(sockets)
        for (i in 1..3) {
            tcpPeer(port, "peer-t$i")
            assertEquals("connected:peer-t$i", host.next())
        }
        val dials = FakeDials()
        val loopback = InetAddress.getLoopbackAddress().hostAddress!!
        assertTrue(dials.prove(sockets, 1, "peer-m1", ip = loopback))
        assertEquals("connected:peer-m1", host.next())
        assertFalse(dials.prove(sockets, 2, "peer-m2", ip = loopback))
        assertNull(host.quiet())
    }

    @Test
    fun `a dial that arrives while stopped is closed and announces nothing`() {
        val host = Host("peer-a")
        val sockets = PeerStreamSockets(host, fast)
        val dials = FakeDials()
        dials.open.add(1)
        assertFalse(sockets.dialProved(dials, 1, "peer-m", PAGE_IP, accepting = false))
        assertEquals(listOf(1L), dials.closed.toList())
        assertNull(host.quiet())
        assertEquals(0, sockets.openCount)
    }

    // --- closing --------------------------------------------------------------------

    @Test
    fun `closing the dials reports each held address once and nothing for a superseded stream`() {
        val host = Host("peer-a")
        val sockets = PeerStreamSockets(host, fast)
        val dials = FakeDials()
        assertTrue(dials.prove(sockets, 1, "peer-m"))
        assertTrue(dials.prove(sockets, 2, "peer-n"))
        assertTrue(dials.prove(sockets, 3, "peer-m"))
        assertEquals(setOf("connected:peer-m", "connected:peer-n"), setOf(host.next(), host.next()))

        sockets.closeDials()
        assertEquals(setOf("lost:peer-m", "lost:peer-n"), setOf(host.next(), host.next()))
        assertNull(host.quiet())
        // The listener's own Closed for each arrives afterwards and finds nothing.
        for (stream in 1L..3L) sockets.dialClosed(stream)
        assertNull(host.quiet())
        assertEquals(0, sockets.dialledCount)
        assertEquals(0, sockets.openCount)
    }

    @Test
    fun `closeAll ends dials beside TCP streams, each loss once`() {
        val host = Host("peer-z")
        val sockets = PeerStreamSockets(host, fast)
        tcpPeer(listen(sockets), "peer-t")
        assertEquals("connected:peer-t", host.next())
        val dials = FakeDials()
        assertTrue(dials.prove(sockets, 1, "peer-m"))
        assertEquals("connected:peer-m", host.next())

        sockets.closeAll()
        assertEquals(setOf("lost:peer-t", "lost:peer-m"), setOf(host.next(), host.next()))
        // The dial was closed through the listener, whose Closed then finds nothing.
        assertEquals(listOf(1L), dials.closed.toList())
        dials.reportClosed(sockets)
        assertNull(host.quiet())
    }
}
