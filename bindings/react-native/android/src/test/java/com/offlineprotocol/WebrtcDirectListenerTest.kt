package com.offlineprotocol

import org.junit.After
import org.junit.Assert.assertArrayEquals
import org.junit.Assert.assertEquals
import org.junit.Assert.assertFalse
import org.junit.Assert.assertNotNull
import org.junit.Assert.assertNull
import org.junit.Assert.assertTrue
import org.junit.Test
import java.net.DatagramPacket
import java.net.DatagramSocket
import java.net.InetAddress
import java.net.InetSocketAddress
import java.util.Collections
import java.util.concurrent.LinkedBlockingQueue
import java.util.concurrent.TimeUnit

/**
 * The platform half of the WebRTC Direct listener ([WebrtcDirectListener]):
 * the socket, the pump and the clock around the engine's sans-I/O listener,
 * held to the UDL contract, with the stream table behind it.
 *
 * The engine is a fake that yields scripted events and records every call
 * made into it; the UDP socket is real, on loopback (production binds the
 * Wi-Fi network's private address, which the engine checks, not this).
 */
class WebrtcDirectListenerTest {

    private val cleanup = Collections.synchronizedList(mutableListOf<AutoCloseable>())
    private var listener: WebrtcDirectListener? = null

    @After
    fun tearDown() {
        listener?.close()
        for (c in cleanup) try { c.close() } catch (_: Exception) {}
    }

    companion object {
        // Distinctive, so a leak into a diagnostic is found by search.
        const val PASSWORD = "SecretPasswordNeverLoggedAnywhere"
        const val FRAGMENT = "op-direct=1&h=127.0.0.1&p=1&c=AAAA&u=UfragUfragUfrag12&w=$PASSWORD&a=peer-a"
    }

    /** The engine's `webrtc_direct_*`, scripted. Calls are made under the listener's lock. */
    private class FakeEngine {
        val events = ArrayDeque<WebrtcDirectListener.Event>()
        val datagrams = ArrayDeque<WebrtcDirectListener.Datagram>()
        val received = LinkedBlockingQueue<Triple<String, Int, ByteArray>>()
        val calls = Collections.synchronizedList(mutableListOf<String>())
        val proved = mutableSetOf<Long>()
        var ticks = ArrayDeque<Long?>()
        @Volatile var tickCount = 0
        /** What a received datagram makes the engine queue, if anything. */
        var onDatagram: (FakeEngine, String, Int, ByteArray) -> Unit = { _, _, _, _ -> }

        val engine = object : WebrtcDirectListener.Engine {
            override fun open(localIp: String, localPort: Int): WebrtcDirectListener.Code {
                calls.add("open:$localIp")
                return WebrtcDirectListener.Code(FRAGMENT, localIp, localPort, "peer-a")
            }

            override fun close() {
                calls.add("close")
                for (stream in proved.sorted()) events.addLast(WebrtcDirectListener.Event.Closed(stream))
                proved.clear()
            }

            override fun datagramReceived(sourceIp: String, sourcePort: Int, data: ByteArray) {
                received.add(Triple(sourceIp, sourcePort, data))
                onDatagram(this@FakeEngine, sourceIp, sourcePort, data)
            }

            override fun nextDatagram(): WebrtcDirectListener.Datagram? = datagrams.removeFirstOrNull()
            override fun nextEvent(): WebrtcDirectListener.Event? = events.removeFirstOrNull()

            override fun send(stream: Long, body: ByteArray) {
                if (stream !in proved) throw IllegalStateException("no such stream")
                calls.add("send:$stream:${body.size}")
            }

            override fun closeStream(stream: Long) {
                calls.add("closeStream:$stream")
                if (proved.remove(stream)) events.addLast(WebrtcDirectListener.Event.Closed(stream))
            }

            override fun tick(): Long? {
                tickCount++
                return ticks.removeFirstOrNull()
            }
        }

        fun prove(stream: Long, address: String, ip: String = "192.168.1.20") {
            proved.add(stream)
            events.addLast(WebrtcDirectListener.Event.Proved(stream, address, ip, 50_000))
        }
    }

    private class Host(private val me: String) : PeerStreamSockets.Host {
        val events = LinkedBlockingQueue<String>()
        override fun identityAssertion(): ByteArray = ByteArray(0)
        override fun localAddress(): String = me
        override fun verify(assertion: ByteArray): String = throw IllegalStateException("unused")
        override fun peerConnected(address: String) { events.add("connected:$address") }
        override fun messageReceived(address: String, body: ByteArray) { events.add("message:$address:${body.size}") }
        override fun peerDisconnected(address: String) { events.add("lost:$address") }
        override fun diagnostic(level: String, message: String, context: Map<String, Any?>) {
            events.add("diagnostic")
        }
        fun next(ms: Long = 5_000): String? {
            while (true) {
                val e = events.poll(ms, TimeUnit.MILLISECONDS) ?: return null
                if (e != "diagnostic") return e
            }
        }
        fun quiet(ms: Long = 300): String? = next(ms)
    }

    /** Every diagnostic either side emits, rendered as text. */
    private val diagnostics = Collections.synchronizedList(mutableListOf<String>())

    private fun record(level: String, message: String, context: Map<String, Any?>) {
        diagnostics.add("$level $message $context")
    }

    /** The thread each event reached the sink on, in order. */
    private val deliveredOn = Collections.synchronizedList(mutableListOf<Thread>())

    /**
     * A listener over [fake] whose events go into a stream table on [host].
     * [beforeBody] runs on the delivering thread ahead of each body.
     */
    private fun open(
        fake: FakeEngine,
        host: Host,
        beforeBody: () -> Unit = {},
        onClosed: (String) -> Unit = {},
    ): Pair<WebrtcDirectListener, DatagramSocket> {
        val sockets = PeerStreamSockets(object : PeerStreamSockets.Host by host {
            override fun diagnostic(level: String, message: String, context: Map<String, Any?>) =
                record(level, message, context)
        })
        val sink = object : WebrtcDirectListener.Sink {
            override fun proved(listener: WebrtcDirectListener, stream: Long, address: String, remoteIp: String) {
                deliveredOn.add(Thread.currentThread())
                sockets.dialProved(listener, stream, address, remoteIp, accepting = true)
            }
            override fun body(stream: Long, data: ByteArray) {
                deliveredOn.add(Thread.currentThread())
                beforeBody()
                sockets.dialBody(stream, data)
            }
            override fun closed(stream: Long) {
                deliveredOn.add(Thread.currentThread())
                sockets.dialClosed(stream)
            }
        }
        val socket = DatagramSocket(InetSocketAddress(InetAddress.getLoopbackAddress(), 0))
        cleanup.add(socket)
        val l = WebrtcDirectListener(fake.engine, sink, ::record, onClosed)
        listener = l
        l.open(socket)
        sinkSockets = sockets
        return l to socket
    }

    private var sinkSockets: PeerStreamSockets? = null

    /** Waits up to three seconds for [condition]. */
    private fun eventually(what: String, condition: () -> Boolean) {
        val deadline = System.currentTimeMillis() + 3_000
        while (!condition()) {
            if (System.currentTimeMillis() > deadline) throw AssertionError(what)
            Thread.sleep(5)
        }
    }

    private fun client(): DatagramSocket =
        DatagramSocket(InetSocketAddress(InetAddress.getLoopbackAddress(), 0)).also {
            it.soTimeout = 3_000
            cleanup.add(it)
        }

    // --- the code -------------------------------------------------------------------

    @Test
    fun `the code is returned and never reaches a diagnostic or a string`() {
        val fake = FakeEngine()
        val host = Host("peer-a")
        val (l, socket) = open(fake, host)
        val code = l.code!!
        assertEquals(FRAGMENT, code.fragment)
        assertEquals(socket.localPort, code.port)
        assertEquals("open:127.0.0.1", fake.calls.first())
        // The code's own string leaves the fragment out.
        assertFalse(code.toString().contains(PASSWORD))
        assertFalse("$code".contains("op-direct"))

        fake.prove(1, "peer-m")
        l.pump()
        assertEquals("connected:peer-m", host.next())
        l.close()
        assertEquals("lost:peer-m", host.next())

        assertTrue("the open is reported", diagnostics.any { it.contains("WebRTC Direct listener open") })
        for (line in diagnostics) {
            assertFalse("a diagnostic carries the password: $line", line.contains(PASSWORD))
            assertFalse("a diagnostic carries the code: $line", line.contains("op-direct"))
        }
    }

    @Test
    fun `a socket bound to no one address is refused before the engine is asked`() {
        val fake = FakeEngine()
        val socket = DatagramSocket(0)
        cleanup.add(socket)
        val l = WebrtcDirectListener(fake.engine, object : WebrtcDirectListener.Sink {
            override fun proved(listener: WebrtcDirectListener, stream: Long, address: String, remoteIp: String) {}
            override fun body(stream: Long, data: ByteArray) {}
            override fun closed(stream: Long) {}
        }, ::record)
        try {
            l.open(socket)
            throw AssertionError("a wildcard socket was opened")
        } catch (_: IllegalArgumentException) {
        }
        assertTrue(fake.calls.isEmpty())
        assertTrue(socket.isClosed)
        // A listener that never opened closes nothing of the engine's.
        l.close()
        assertTrue(fake.calls.isEmpty())
    }

    // --- the socket and the pump -------------------------------------------------------

    @Test
    fun `every datagram reaches the engine and every queued datagram is sent`() {
        val fake = FakeEngine()
        fake.onDatagram = { engine, ip, port, data ->
            // An authenticated check is answered to where it came from.
            engine.datagrams.addLast(WebrtcDirectListener.Datagram(ip, port, data.reversedArray()))
        }
        val (_, socket) = open(fake, Host("peer-a"))
        val page = client()
        val check = byteArrayOf(1, 2, 3, 4)
        page.send(DatagramPacket(check, check.size, InetAddress.getLoopbackAddress(), socket.localPort))

        val (ip, port, data) = fake.received.poll(3, TimeUnit.SECONDS)!!
        assertEquals(InetAddress.getLoopbackAddress().hostAddress, ip)
        assertEquals(page.localPort, port)
        assertArrayEquals(check, data)

        val answer = DatagramPacket(ByteArray(16), 16)
        page.receive(answer)
        assertArrayEquals(byteArrayOf(4, 3, 2, 1), answer.data.copyOf(answer.length))
    }

    @Test
    fun `the clock is re-armed from each tick`() {
        val fake = FakeEngine()
        fake.ticks = ArrayDeque(listOf(20L, 20L, null))
        open(fake, Host("peer-a"))
        val deadline = System.currentTimeMillis() + 3_000
        while (fake.tickCount < 3 && System.currentTimeMillis() < deadline) Thread.sleep(10)
        assertEquals(3, fake.tickCount)
        // The last tick returned null: nothing is timed, and no tick follows.
        Thread.sleep(200)
        assertEquals(3, fake.tickCount)
    }

    @Test
    fun `a supersede inside the pump is handled after the event that caused it`() {
        val fake = FakeEngine()
        val host = Host("peer-a")
        val (l, _) = open(fake, host)
        fake.prove(1, "peer-m")
        fake.prove(2, "peer-m")
        l.pump()
        assertEquals("connected:peer-m", host.next())
        // The table closed the older through the listener, inside the pump;
        // the Closed that queued was handled in the same pump and reported nothing.
        assertTrue(fake.calls.contains("closeStream:1"))
        assertNull(host.quiet())
        assertNull(fake.engine.nextEvent())

        fake.events.addLast(WebrtcDirectListener.Event.Body(2, ByteArray(3)))
        l.pump()
        assertEquals("message:peer-m:3", host.next())
        assertEquals(PeerStreamSockets.SendResult.QUEUED, sinkSockets!!.send("peer-m", ByteArray(4)))
        eventually("the send reaches the engine") { fake.calls.contains("send:2:4") }
    }

    // --- the caller's thread ---------------------------------------------------------

    @Test
    fun `a send hands its pump to the listener and runs no delivery on the calling thread`() {
        val fake = FakeEngine()
        val host = Host("peer-a")
        val (l, _) = open(fake, host)
        fake.prove(1, "peer-m")
        l.pump()
        assertEquals("connected:peer-m", host.next())

        // Queued in the engine, as a body is between a datagram and its pump.
        fake.events.addLast(WebrtcDirectListener.Event.Body(1, ByteArray(3)))
        deliveredOn.clear()
        // The transport thread's send: the pump after it delivers the body,
        // and the body can mean an MLS decrypt under the core's lock.
        assertEquals(PeerStreamSockets.SendResult.QUEUED, sinkSockets!!.send("peer-m", ByteArray(4)))
        assertEquals("message:peer-m:3", host.next())
        eventually("the send reaches the engine") { fake.calls.contains("send:1:4") }
        assertFalse(
            "a delivery ran on the thread that sent",
            deliveredOn.any { it === Thread.currentThread() },
        )
    }

    @Test
    fun `a send and a stream close do not wait behind a delivery`() {
        val fake = FakeEngine()
        val host = Host("peer-a")
        val inDelivery = java.util.concurrent.CountDownLatch(1)
        val release = java.util.concurrent.CountDownLatch(1)
        fake.onDatagram = { engine, _, _, _ -> engine.events.addLast(WebrtcDirectListener.Event.Body(1, ByteArray(2))) }
        val (l, socket) = open(fake, host, beforeBody = {
            inDelivery.countDown()
            release.await(10, TimeUnit.SECONDS)
        })
        fake.prove(1, "peer-m")
        fake.prove(2, "peer-n")
        l.pump()
        assertEquals(setOf("connected:peer-m", "connected:peer-n"), setOf(host.next(), host.next()))

        // The receive thread is inside a delivery, holding the listener.
        val page = client()
        page.send(DatagramPacket(ByteArray(1), 1, InetAddress.getLoopbackAddress(), socket.localPort))
        assertTrue(inDelivery.await(3, TimeUnit.SECONDS))
        try {
            val started = System.nanoTime()
            assertEquals(PeerStreamSockets.SendResult.QUEUED, sinkSockets!!.send("peer-m", ByteArray(4)))
            l.close(2)
            val tookMs = TimeUnit.NANOSECONDS.toMillis(System.nanoTime() - started)
            assertTrue("the caller waited $tookMs ms behind a delivery", tookMs < 1_000)
            assertFalse("the send reached the engine inside the delivery", fake.calls.contains("send:1:4"))
        } finally {
            release.countDown()
        }
        // Then, in the order they were handed, after the delivery.
        assertEquals("message:peer-m:2", host.next())
        eventually("the send and the close reach the engine") { fake.calls.contains("closeStream:2") }
        assertTrue(fake.calls.indexOf("send:1:4") in 0 until fake.calls.indexOf("closeStream:2"))
        assertEquals("lost:peer-n", host.next())
    }

    @Test
    fun `closing reports each proved stream lost once, then closes the socket`() {
        val fake = FakeEngine()
        val host = Host("peer-a")
        val (l, socket) = open(fake, host)
        fake.prove(1, "peer-m")
        fake.prove(2, "peer-n")
        l.pump()
        assertEquals(setOf("connected:peer-m", "connected:peer-n"), setOf(host.next(), host.next()))

        l.close()
        assertEquals(setOf("lost:peer-m", "lost:peer-n"), setOf(host.next(), host.next()))
        assertTrue(socket.isClosed)
        assertFalse(l.isOpen)
        l.close()
        sinkSockets!!.closeDials()
        assertNull(host.quiet())
        assertEquals(1, fake.calls.count { it == "close" })
        // A closed listener refuses a send rather than reaching the engine.
        try {
            l.send(1, ByteArray(1))
            throw AssertionError("a closed listener took a send")
        } catch (_: IllegalStateException) {
        }
    }

    @Test
    fun `a close that names a reason reports it once, after the listener closed`() {
        // The application hears the bridge's own close (the network went,
        // the socket failed, the transport stopped) and stops showing a code
        // whose listener is gone. Once, so a second close cannot repeat it.
        val reported = Collections.synchronizedList(mutableListOf<String>())
        val fake = FakeEngine()
        val host = Host("peer-a")
        var l: WebrtcDirectListener? = null
        val (opened, _) = open(fake, host, onClosed = { reason ->
            assertFalse("reported while still open", l!!.isOpen)
            reported.add(reason)
        })
        l = opened
        fake.prove(1, "peer-m")
        opened.pump()
        assertEquals("connected:peer-m", host.next())

        opened.close(WebrtcDirectListener.CLOSED_NETWORK)
        opened.close(WebrtcDirectListener.CLOSED_SOCKET)
        assertEquals(listOf("network"), reported)
        assertEquals("lost:peer-m", host.next())
    }

    @Test
    fun `a close the application asked for reports nothing`() {
        // Its own closeWebrtcDirect, or an open replacing the listener,
        // names no reason: the application already knows.
        val reported = Collections.synchronizedList(mutableListOf<String>())
        val (l, _) = open(FakeEngine(), Host("peer-a"), onClosed = { reported.add(it) })
        l.close()
        l.close(WebrtcDirectListener.CLOSED_STOPPED)
        assertEquals(emptyList<String>(), reported)

        // Never opened: there is no listener the application was shown.
        val never = WebrtcDirectListener(FakeEngine().engine, object : WebrtcDirectListener.Sink {
            override fun proved(listener: WebrtcDirectListener, stream: Long, address: String, remoteIp: String) {}
            override fun body(stream: Long, data: ByteArray) {}
            override fun closed(stream: Long) {}
        }, ::record, onClosed = { reported.add(it) })
        never.close(WebrtcDirectListener.CLOSED_STOPPED)
        assertEquals(emptyList<String>(), reported)
    }

    @Test
    fun `the closed event carries the tag apps match on and its reason`() {
        // Pinned literally: apps switch on these (src/types.ts
        // WebrtcDirectClosedEvent and WebrtcDirectClosedReason).
        assertEquals("webrtc_direct_closed", WebrtcDirectListener.CLOSED_EVENT_TYPE)
        assertEquals(
            listOf("network", "hotspot", "socket", "stopped"),
            listOf(
                WebrtcDirectListener.CLOSED_NETWORK,
                WebrtcDirectListener.CLOSED_HOTSPOT,
                WebrtcDirectListener.CLOSED_SOCKET,
                WebrtcDirectListener.CLOSED_STOPPED,
            ),
        )
        val event = org.json.JSONObject(WebrtcDirectListener.closedEventJson("hotspot"))
        assertEquals("webrtc_direct_closed", event.getString("type"))
        assertEquals("hotspot", event.getString("reason"))
        assertEquals(2, event.length())
    }

    @Test
    fun `listenAddress takes a private IPv4 address, else link-local, never public or IPv6`() {
        fun a(s: String) = InetAddress.getByName(s)
        assertEquals(a("192.168.1.5"), WebrtcDirectListener.listenAddress(listOf(a("fe80::1"), a("169.254.3.4"), a("192.168.1.5"))))
        assertEquals(a("169.254.3.4"), WebrtcDirectListener.listenAddress(listOf(a("fe80::1"), a("169.254.3.4"))))
        assertNull(WebrtcDirectListener.listenAddress(listOf(a("8.8.8.8"), a("fe80::1"))))
        assertNotNull(WebrtcDirectListener.listenAddress(listOf(a("10.0.0.2"))))
    }
}
