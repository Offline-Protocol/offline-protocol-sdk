package com.offlineprotocol

import org.junit.After
import org.junit.Assert.assertArrayEquals
import org.junit.Assert.assertEquals
import org.junit.Assert.assertNull
import org.junit.Assert.assertTrue
import org.junit.Test
import java.io.DataInputStream
import java.io.EOFException
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
 * Drives [PeerStreamSockets] on real loopback sockets: the Wi-Fi Direct
 * manager's per-socket behaviour without a Wi-Fi Direct group, which no CI
 * runner has. The verifier is a stand-in (the real one is Rust, needs the
 * native library, and is tested against the identity assertion vectors in
 * Rust): an "assertion" here is a `peer-…` address in ASCII, zero-padded to
 * the 96-byte floor, and nothing else verifies.
 *
 * What a test cannot see here is the manager's group handling (the listener,
 * the owner connect, the reconnect ladder), which needs a device. The device
 * run is recorded in the PR (docs/bridges C9).
 */
class PeerStreamSocketsTest {

    private val cleanup = Collections.synchronizedList(mutableListOf<AutoCloseable>())

    @After
    fun tearDown() {
        for (c in cleanup) try { c.close() } catch (_: Exception) {}
    }

    // --- the stand-in core -------------------------------------------------------

    private class Host(private val me: String) : PeerStreamSockets.Host {
        val events = LinkedBlockingQueue<String>()
        val bodies = LinkedBlockingQueue<Pair<String, ByteArray>>()

        override fun identityAssertion(): ByteArray = assertion(me)
        override fun localAddress(): String = me
        override fun verify(assertion: ByteArray): String {
            // Only a zero-padded `peer-…` name verifies, like only a real
            // signature does; anything else (a message body, "bad-…") throws.
            val name = assertion.takeWhile { it != 0.toByte() }.toByteArray()
            val padding = assertion.drop(name.size)
            val address = String(name, Charsets.US_ASCII)
            if (!Regex("peer-[a-z0-9-]+").matches(address) || padding.any { it != 0.toByte() }) {
                throw IllegalStateException("signature")
            }
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

    companion object {
        fun assertion(address: String): ByteArray =
            address.toByteArray().copyOf(PeerStreamFraming.PREAMBLE_MIN_BYTES)
    }

    private val fast = PeerStreamSockets.Limits(
        preambleTimeoutMs = 300,
        writeTimeoutMs = 500,
        writeStallMs = 100,
    )

    /** A listener whose accepted sockets [sockets] runs, as the group owner's does. */
    private fun listen(sockets: PeerStreamSockets, running: () -> Boolean = { true }): Int {
        val server = ServerSocket(0, 50, InetAddress.getLoopbackAddress())
        cleanup.add(server)
        thread(isDaemon = true) {
            while (!server.isClosed) {
                val s = try { server.accept() } catch (_: IOException) { return@thread }
                cleanup.add(s)
                thread(isDaemon = true) { sockets.run(s, outbound = false, accepting = running) }
            }
        }
        return server.localPort
    }

    /** A socket driven by hand, standing in for a peer (or an attacker). */
    private fun raw(port: Int, receiveBuffer: Int? = null): Socket {
        val s = Socket()
        receiveBuffer?.let { s.receiveBufferSize = it }
        s.connect(InetSocketAddress(InetAddress.getLoopbackAddress(), port), 2_000)
        s.soTimeout = 5_000
        cleanup.add(s)
        return s
    }

    private fun Socket.send(body: ByteArray) {
        getOutputStream().write(PeerStreamFraming.frame(body))
        getOutputStream().flush()
    }

    private fun Socket.sendRaw(bytes: ByteArray) {
        getOutputStream().write(bytes)
        getOutputStream().flush()
    }

    private fun Socket.readBody(): ByteArray = PeerStreamFraming.readBody(DataInputStream(getInputStream()), preamble = false)

    /** True once the far side has closed: reads drain to end of stream. */
    private fun Socket.closedByPeer(): Boolean {
        val input = getInputStream()
        val buffer = ByteArray(64 * 1024)
        return try {
            while (true) {
                if (input.read(buffer) < 0) return true
            }
            @Suppress("UNREACHABLE_CODE") false
        } catch (_: SocketTimeoutException) {
            false
        } catch (_: IOException) {
            true
        }
    }

    private fun hex(s: String): ByteArray =
        ByteArray(s.length / 2) { i -> s.substring(2 * i, 2 * i + 2).toInt(16).toByte() }

    // --- the exchange ------------------------------------------------------------

    @Test
    fun `two ends announce each other and deliver both ways`() {
        val hostA = Host("peer-a")
        val hostB = Host("peer-b")
        val a = PeerStreamSockets(hostA, fast)
        val b = PeerStreamSockets(hostB, fast)
        val port = listen(a)

        val socket = Socket()
        cleanup.add(socket)
        socket.connect(InetSocketAddress(InetAddress.getLoopbackAddress(), port))
        thread(isDaemon = true) { b.run(socket, outbound = true) { true } }

        assertEquals("connected:peer-b", hostA.next())
        assertEquals("connected:peer-a", hostB.next())

        assertEquals(PeerStreamSockets.SendResult.QUEUED, b.send("peer-a", "hello".toByteArray()))
        assertEquals("message:peer-b:5", hostA.next())
        assertEquals(PeerStreamSockets.SendResult.QUEUED, a.send("peer-b", "hi".toByteArray()))
        assertEquals("message:peer-a:2", hostB.next())

        socket.close()
        assertEquals("lost:peer-b", hostA.next())
        assertEquals("lost:peer-a", hostB.next())
        assertNull(hostA.quiet())
    }

    @Test
    fun `our preamble goes first without waiting for theirs`() {
        val port = listen(PeerStreamSockets(Host("peer-a"), fast))
        val peer = raw(port)
        // Nothing sent from this side, and the listener's preamble still arrives.
        assertArrayEquals(assertion("peer-a"), peer.readBody())
    }

    @Test
    fun `frames are ordered and never interleave`() {
        val host = Host("peer-a")
        val a = PeerStreamSockets(host, PeerStreamSockets.Limits(writeChunkBytes = 1024))
        val port = listen(a)
        val peer = raw(port)
        peer.readBody()
        peer.send(assertion("peer-b"))
        assertEquals("connected:peer-b", host.next())

        val sizes = listOf(300_000, 1, 70_000, 1_048_576, 5)
        for ((i, size) in sizes.withIndex()) {
            assertEquals(PeerStreamSockets.SendResult.QUEUED, a.send("peer-b", ByteArray(size) { i.toByte() }))
        }
        for ((i, size) in sizes.withIndex()) {
            val body = peer.readBody()
            assertEquals(size, body.size)
            assertTrue(body.all { it == i.toByte() })
        }
    }

    // --- refusals ------------------------------------------------------------------

    @Test
    fun `a preamble that does not verify is closed and announces nothing`() {
        val host = Host("peer-a")
        val port = listen(PeerStreamSockets(host, fast))
        val peer = raw(port)
        peer.readBody()
        peer.send(assertion("bad_peer"))
        assertTrue(peer.closedByPeer())
        assertNull(host.quiet())
    }

    @Test
    fun `a message before any preamble is refused`() {
        // The chapter's vector: a well-formed message frame as the first frame.
        val host = Host("peer-a")
        val port = listen(PeerStreamSockets(host, fast))
        val peer = raw(port)
        peer.sendRaw(hex("00000095f5111111112222333344445555555555552c6f6666317179736c7576776c353932327963747a643075396770723036676e336b376c64667667747767766e2c6f6666317179756c7779377335657a7a323063793232327a727730347277647333397561707176396a3872300361707001080080a0abfef96200001368656c6c6f206f76657220612073747265616d00000001000000"))
        assertTrue(peer.closedByPeer())
        assertNull(host.quiet())
    }

    @Test
    fun `our own address played back is refused`() {
        val host = Host("peer-a")
        val port = listen(PeerStreamSockets(host, fast))
        val peer = raw(port)
        peer.send(peer.readBody())
        assertTrue(peer.closedByPeer())
        assertNull(host.quiet())
    }

    @Test
    fun `a refused length after the announcement ends the stream and reports one loss`() {
        // Regression pin: the reader this replaced skipped a refused length and
        // read the skipped body's first bytes as the next prefix.
        val host = Host("peer-a")
        val port = listen(PeerStreamSockets(host, fast))
        val peer = raw(port)
        peer.readBody()
        peer.send(assertion("peer-b"))
        assertEquals("connected:peer-b", host.next())

        peer.sendRaw(hex("00100001"))
        assertEquals("lost:peer-b", host.next())
        assertTrue(peer.closedByPeer())
        assertNull(host.quiet())
    }

    @Test
    fun `a body of exactly the ceiling is delivered`() {
        // Regression pin: the reader this replaced refused `length >= 1 MiB`.
        val host = Host("peer-a")
        val port = listen(PeerStreamSockets(host, fast))
        val peer = raw(port)
        peer.readBody()
        peer.send(assertion("peer-b"))
        assertEquals("connected:peer-b", host.next())
        peer.send(ByteArray(1_048_576) { 1 })
        assertEquals("message:peer-b:1048576", host.next())
    }

    @Test
    fun `a silent socket is closed at the preamble deadline`() {
        val host = Host("peer-a")
        val port = listen(PeerStreamSockets(host, fast))
        val peer = raw(port)
        peer.readBody()
        assertTrue(peer.closedByPeer())
        assertNull(host.quiet())
    }

    @Test
    fun `a dripped prefix does not extend the deadline`() {
        // One deadline over the prefix and the body together.
        val host = Host("peer-a")
        val port = listen(PeerStreamSockets(host, fast))
        val peer = raw(port)
        peer.readBody()
        peer.sendRaw(hex("00000060"))
        val start = System.nanoTime()
        peer.sendRaw(assertion("peer-b").copyOf(40))
        assertTrue(peer.closedByPeer())
        assertTrue(TimeUnit.NANOSECONDS.toMillis(System.nanoTime() - start) < 3_000)
        assertNull(host.quiet())
    }

    @Test
    fun `the stream limit refuses the next socket`() {
        val host = Host("peer-a")
        val port = listen(PeerStreamSockets(host, fast.copy(maxStreams = 1, preambleTimeoutMs = 5_000)))
        val first = raw(port)
        first.readBody()
        val second = raw(port)
        assertTrue(second.closedByPeer())
        // The first is untouched and can still prove itself.
        first.send(assertion("peer-b"))
        assertEquals("connected:peer-b", host.next())
    }

    @Test
    fun `many sockets at once are held to the stream limit, and a slot frees on end`() {
        // Each accepted socket runs on its own thread, so these reach the
        // limit check together. The limit is a reservation taken by one
        // atomic increment; a size read followed by an add let every socket
        // that read before the others added through.
        val host = Host("peer-a")
        val limit = 2
        val a = PeerStreamSockets(host, fast.copy(maxStreams = limit, preambleTimeoutMs = 5_000))
        val port = listen(a)
        val peers = (0 until 12).map { raw(port) }
        val served = peers.count { peer ->
            try { peer.readBody(); true } catch (_: IOException) { false }
        }
        assertEquals(limit, served)
        assertEquals(limit, a.openCount)

        // Ending one frees its slot for the next socket.
        a.closeAll()
        val later = raw(port)
        assertArrayEquals(assertion("peer-a"), later.readBody())
    }

    @Test
    fun `a socket that arrives while stopped is closed unread`() {
        val host = Host("peer-a")
        val port = listen(PeerStreamSockets(host, fast)) { false }
        assertTrue(raw(port).closedByPeer())
        assertNull(host.quiet())
    }

    // --- one announced stream per address ---------------------------------------------

    @Test
    fun `a newer stream supersedes the older with one announcement and one loss`() {
        val host = Host("peer-a")
        val port = listen(PeerStreamSockets(host, fast))
        val older = raw(port)
        older.readBody()
        older.send(assertion("peer-b"))
        assertEquals("connected:peer-b", host.next())

        val newer = raw(port)
        newer.readBody()
        newer.send(assertion("peer-b"))
        // The older is closed, and neither a second announcement nor a loss
        // reaches the core: the address never stopped being reachable.
        assertTrue(older.closedByPeer())
        assertNull(host.quiet())

        newer.send("still here".toByteArray())
        assertEquals("message:peer-b:10", host.next())

        newer.close()
        assertEquals("lost:peer-b", host.next())
        assertNull(host.quiet())
    }

    @Test
    fun `closeAll reports each announced stream once and delivers nothing after`() {
        val host = Host("peer-a")
        val a = PeerStreamSockets(host, fast)
        val port = listen(a)
        val proved = raw(port)
        proved.readBody()
        proved.send(assertion("peer-b"))
        assertEquals("connected:peer-b", host.next())
        val unproved = raw(port)
        unproved.readBody()

        a.closeAll()
        assertEquals("lost:peer-b", host.next())
        assertTrue(proved.closedByPeer())
        assertTrue(unproved.closedByPeer())
        assertNull(host.quiet())
        assertTrue(a.isEmpty())
        assertEquals(0, a.openCount)
    }

    // --- sending -----------------------------------------------------------------------

    @Test
    fun `send refuses an unproved recipient and a body outside the bounds`() {
        val host = Host("peer-a")
        val a = PeerStreamSockets(host, fast)
        val port = listen(a)
        assertEquals(PeerStreamSockets.SendResult.NO_STREAM, a.send("peer-b", byteArrayOf(1)))

        val peer = raw(port)
        peer.readBody()
        // Connected but not yet proved: still nothing to send to.
        assertEquals(PeerStreamSockets.SendResult.NO_STREAM, a.send("peer-b", byteArrayOf(1)))
        peer.send(assertion("peer-b"))
        assertEquals("connected:peer-b", host.next())

        assertEquals(PeerStreamSockets.SendResult.OUT_OF_BOUNDS, a.send("peer-b", ByteArray(0)))
        assertEquals(PeerStreamSockets.SendResult.OUT_OF_BOUNDS, a.send("peer-b", ByteArray(1_048_577)))
        assertEquals(PeerStreamSockets.SendResult.QUEUED, a.send("peer-b", ByteArray(1_048_576)))
    }

    @Test
    fun `a peer that stops reading stalls only itself, and is ended when it makes no progress`() {
        val host = Host("peer-a")
        val a = PeerStreamSockets(host, fast)
        val port = listen(a)

        val stuck = raw(port, receiveBuffer = 4096)
        stuck.readBody()
        stuck.send(assertion("peer-stuck"))
        assertEquals("connected:peer-stuck", host.next())

        val healthy = raw(port)
        healthy.readBody()
        healthy.send(assertion("peer-ok"))
        assertEquals("connected:peer-ok", host.next())

        // Far more than both kernel buffers hold, toward a peer that never reads.
        repeat(16) { a.send("peer-stuck", ByteArray(1_048_576)) }
        assertEquals(PeerStreamSockets.SendResult.QUEUED, a.send("peer-ok", "through".toByteArray()))
        assertArrayEquals("through".toByteArray(), healthy.readBody())

        // No progress for the write deadline ends the stuck stream, once.
        assertEquals("lost:peer-stuck", host.next(10_000))
        assertNull(host.quiet())
    }

    @Test
    fun `a burst to a reading peer is not dropped`() {
        val host = Host("peer-a")
        val a = PeerStreamSockets(host, fast)
        val port = listen(a)
        val peer = raw(port)
        peer.readBody()
        peer.send(assertion("peer-b"))
        assertEquals("connected:peer-b", host.next())

        // Eight ceiling frames in one tick: twice the queue budget. The writer
        // has not stalled, so none is dropped. The frames are built before the
        // first send: the writer fills both loopback buffers and blocks while
        // the peer is not yet reading, so anything slow between two sends (a
        // cold JIT building a 1 MiB array) lets it pass the stall threshold,
        // and the fifth send is refused. That is the bound working, not a
        // burst dropped, and it is not what this test is about.
        val frames = List(8) { ByteArray(1_048_576) { i -> i.toByte() } }
        val results = frames.map { a.send("peer-b", it) }
        assertEquals(List(8) { PeerStreamSockets.SendResult.QUEUED }, results)
        repeat(8) { assertEquals(1_048_576, peer.readBody().size) }
    }
}
