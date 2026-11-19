package com.offlineprotocol

import org.json.JSONObject
import java.net.DatagramPacket
import java.net.DatagramSocket
import java.net.Inet4Address
import java.net.InetAddress
import java.net.InetSocketAddress
import java.util.concurrent.RejectedExecutionException
import java.util.concurrent.ScheduledExecutorService
import java.util.concurrent.ScheduledFuture
import java.util.concurrent.ScheduledThreadPoolExecutor
import java.util.concurrent.TimeUnit
import java.util.concurrent.atomic.AtomicLong
import kotlin.concurrent.thread

/**
 * The platform's half of a WebRTC Direct listener (ADR 0039): one UDP socket,
 * the clock, and the pump between them and the engine's sans-I/O listener.
 *
 * Kept apart from [PeerStreamSockets] because none of it is a stream rule: it
 * moves datagrams and fires timers, and hands every event to [sink], whose one
 * table decides what the core hears. The FFI's contract (the `webrtc_direct_*`
 * block in the UDL) is what it holds:
 *
 * - every datagram the socket receives goes to the listener, unfiltered,
 *   because the listener answers only an authenticated check;
 * - after every call into the listener, every datagram it queued is sent and
 *   every event it queued is handled;
 * - `webrtc_direct_tick` runs again within the milliseconds it returned, and
 *   after any other call, since a datagram or a send can bring the next
 *   deadline forward.
 *
 * Every call into [engine] is made under [lock], so the listener is never
 * entered from two threads at once and its events are handled in the order it
 * queued them. A handler that calls back in (a refusal closing a stream, a
 * supersede) runs on the thread already holding the lock and only marks the
 * pump to go round again, so events are never handled inside one another.
 * The lock is taken before a stream's lock in [PeerStreamSockets], never
 * after it.
 *
 * Only the listener's own threads take [lock]: the receive thread, and
 * [clock]'s one thread, which runs the timer and every [send] and
 * [close] (of a stream) a caller hands it, in the order they were handed.
 * The pump hands the sink every event it finds, and the sink calls the core
 * (a body can mean an MLS decrypt under the core's lock), so a caller that
 * took the lock itself would run those deliveries, or wait out the receive
 * thread's. The caller that sends is the transport thread, which Wi-Fi P2P's
 * broadcasts share: a TCP stream's send only queues a frame for its writer,
 * and a dialled stream's must not cost more. The exception is [close] of the
 * whole listener, which reports every stream's loss before it returns.
 *
 * The dial code carries the listener's ICE password. It is returned to the
 * caller and never logged or put in a diagnostic, and [Code.toString] leaves
 * it out, so a stray string template cannot leak it either.
 */
internal class WebrtcDirectListener(
    private val engine: Engine,
    private val sink: Sink,
    private val diagnostic: (level: String, message: String, context: Map<String, Any?>) -> Unit,
    /**
     * The application's report of a [close] that names a reason (one of the
     * `CLOSED_*` tokens): at most once, after the close. A close with no
     * reason (the application's own, or an open replacing this listener)
     * reports nothing, since the application asked for it.
     */
    private val onClosed: (reason: String) -> Unit = {},
    /** The socket stopped without a [close] of ours (the interface went away). */
    private val onSocketLost: (WebrtcDirectListener) -> Unit = {},
) : PeerStreamSockets.Dials {

    /** The engine's `webrtc_direct_*` methods, in this module's types. */
    interface Engine {
        fun open(localIp: String, localPort: Int): Code
        fun close()
        fun datagramReceived(sourceIp: String, sourcePort: Int, data: ByteArray)
        fun nextDatagram(): Datagram?
        fun nextEvent(): Event?
        fun send(stream: Long, body: ByteArray)
        fun closeStream(stream: Long)
        /** Milliseconds until the next tick is due, or null when nothing is timed. */
        fun tick(): Long?
    }

    /** What the listener's events mean to the stream table. */
    interface Sink {
        fun proved(listener: WebrtcDirectListener, stream: Long, address: String, remoteIp: String)
        fun body(stream: Long, data: ByteArray)
        fun closed(stream: Long)
    }

    /**
     * A dial code. [fragment] is what a page dials: the application shows it
     * after `#` in a URL or as a QR code, so it never reaches a web server.
     */
    class Code(val fragment: String, val host: String, val port: Int, val address: String) {
        /** Leaves [fragment] out: it carries the ICE password. */
        override fun toString(): String = "WebrtcDirectCode(host=$host, port=$port, address=$address)"
    }

    class Datagram(val destinationIp: String, val destinationPort: Int, val data: ByteArray)

    sealed class Event {
        abstract val stream: Long

        class Proved(
            override val stream: Long,
            val address: String,
            val remoteIp: String,
            val remotePort: Int,
        ) : Event()

        class Body(override val stream: Long, val data: ByteArray) : Event()

        class Closed(override val stream: Long) : Event()
    }

    companion object {
        /**
         * The address a listener on a network with [addresses] binds: its
         * first private IPv4 address (RFC 1918), else its first IPv4
         * link-local one, else null. A code may name only a private or
         * link-local address, and a page reaches IPv4 on every network this
         * runs on; an IPv6 link-local address would also need its zone,
         * which no code carries.
         */
        fun listenAddress(addresses: List<InetAddress>): Inet4Address? {
            val v4 = addresses.filterIsInstance<Inet4Address>()
            return v4.firstOrNull { it.isSiteLocalAddress } ?: v4.firstOrNull { it.isLinkLocalAddress }
        }

        /**
         * The most bytes [send] holds waiting for [clock], the engine's own
         * bound on what a stream queues behind its channel: more than that
         * reaching the engine would close the stream when it landed, and
         * without a bound a stalled pump would queue the outbox in memory.
         */
        const val MAX_HANDED_BYTES = 8L * 1024 * 1024

        // Why the bridge closed a listener the application opened: the
        // `reason` of `webrtc_direct_closed` (`src/types.ts`
        // WebrtcDirectClosedReason, which iOS's `paused` and `background`
        // complete). Without the event an application kept showing a code
        // whose listener was gone, and every page that dialled it timed out
        // with nothing to say why.

        /** The Wi-Fi network the listener is bound to went, or lost the bound address. */
        const val CLOSED_NETWORK = "network"
        /** This device's own hotspot went down, or lost the bound address. */
        const val CLOSED_HOTSPOT = "hotspot"
        /** The socket failed without a close of ours. */
        const val CLOSED_SOCKET = "socket"
        /** The transport stopped. */
        const val CLOSED_STOPPED = "stopped"

        /** The bridge event tag apps match on (`src/types.ts` WebrtcDirectClosedEvent). */
        const val CLOSED_EVENT_TYPE = "webrtc_direct_closed"

        /** The `webrtc_direct_closed` payload for [reason], as event JSON. */
        fun closedEventJson(reason: String): String =
            JSONObject().put("type", CLOSED_EVENT_TYPE).put("reason", reason).toString()
    }

    private val lock = Any()
    private var pumping = false
    private var again = false
    @Volatile private var closed = false
    /** Bytes handed to [send] that [clock] has not yet given the engine. */
    private val handedBytes = AtomicLong(0)
    private var socket: DatagramSocket? = null
    private var timer: ScheduledFuture<*>? = null

    // The listener's own thread: the timer, and every send and stream close
    // a caller hands it. Daemon, so a listener dropped without close() does
    // not pin the process.
    private val clock: ScheduledExecutorService =
        ScheduledThreadPoolExecutor(1) { r ->
            Thread(r, "offline-webrtc-direct-timer").apply { isDaemon = true }
        }.apply { removeOnCancelPolicy = true }

    /** The code the engine returned, once open. */
    var code: Code? = null
        private set

    /** The address and port the socket is bound to, once open. */
    var boundAddress: InetAddress? = null
        private set

    /**
     * Opens the engine's listener on [socket], which the caller bound to one
     * private or link-local address (never a wildcard: a code names one
     * address, and a socket on every interface would admit checks from
     * networks the code was never shown on), and starts receiving. On a throw
     * the socket is closed and nothing is left open.
     */
    fun open(socket: DatagramSocket): Code {
        val local = socket.localSocketAddress as? InetSocketAddress
        val ip = local?.address
        if (ip == null || ip.isAnyLocalAddress || local.port == 0) {
            socket.close()
            throw IllegalArgumentException("a WebRTC Direct listener needs a socket bound to one address")
        }
        val opened = try {
            synchronized(lock) {
                check(!closed) { "this WebRTC Direct listener was closed" }
                // The address without a zone, which the engine refuses; the
                // real port, never 0, because the code names it.
                engine.open(ip.hostAddress?.substringBefore('%') ?: "", local.port).also {
                    this.socket = socket
                    code = it
                    boundAddress = ip
                }
            }
        } catch (e: Exception) {
            closed = true
            socket.close()
            clock.shutdownNow()
            throw e
        }
        // The address and port, never the fragment.
        diagnostic("info", "WebRTC Direct listener open", mapOf(
            "host" to opened.host,
            "port" to opened.port,
            "address" to opened.address,
        ))
        thread(isDaemon = true, name = "offline-webrtc-direct-receive") { receive(socket) }
        pump()
        return opened
    }

    /**
     * Closes the engine's listener, hands its last events (a `Closed` for
     * every proved stream) to the sink, then closes the socket. Idempotent:
     * only the first close of an open listener reports, and only when it
     * names a [reason] (see [onClosed]).
     */
    fun close(reason: String? = null) {
        val toClose = synchronized(lock) {
            if (closed) return
            if (code == null) {
                // Never opened: the engine's listener, if any, is another's.
                closed = true
                clock.shutdownNow()
                return
            }
            engine.close()
            // The Closed events, and anything the close queued to send, while
            // the socket is still open.
            pumpLocked(tick = false)
            closed = true
            timer?.cancel(false)
            timer = null
            socket.also { socket = null }
        }
        clock.shutdownNow()
        toClose?.close()
        diagnostic("info", "WebRTC Direct listener closed", emptyMap())
        if (reason != null) onClosed(reason)
    }

    val isOpen: Boolean get() = !closed && code != null

    // MARK: - PeerStreamSockets.Dials

    /**
     * Hands [body] to [clock] and returns: the caller never runs the pump
     * or waits on [lock]. Throws when the listener is closed, or when
     * [MAX_HANDED_BYTES] are already waiting. A send the engine refuses once
     * it is given one (the stream ended, or past its bound, which also
     * closes the stream and so reports the loss once) drops the body; the
     * core retries either way.
     */
    override fun send(stream: Long, body: ByteArray) {
        check(!closed) { "the WebRTC Direct listener is closed" }
        val size = body.size.toLong()
        if (handedBytes.addAndGet(size) > MAX_HANDED_BYTES) {
            handedBytes.addAndGet(-size)
            throw IllegalStateException("too much is waiting for the WebRTC Direct listener")
        }
        try {
            clock.execute {
                handedBytes.addAndGet(-size)
                synchronized(lock) {
                    if (closed) return@execute
                    try {
                        engine.send(stream, body)
                    } catch (_: Exception) {
                        diagnostic("warning", "WebRTC Direct body dropped: the stream refused it", mapOf(
                            "stream" to stream,
                            "size" to body.size,
                        ))
                    }
                    // The Closed for a stream the send ended, and the
                    // datagrams the send queued, go out now.
                    pumpLocked(tick = true)
                }
            }
        } catch (e: RejectedExecutionException) {
            // Closed since the check above.
            handedBytes.addAndGet(-size)
            throw IllegalStateException("the WebRTC Direct listener is closed", e)
        }
    }

    /**
     * Ends [stream]. Inside the pump (a handler's refusal or supersede) at
     * once, so its `Closed` is handled in the same pump; from any other
     * thread handed to [clock], after every send handed before it.
     */
    override fun close(stream: Long) {
        if (closed) return
        if (Thread.holdsLock(lock)) {
            closeStreamLocked(stream)
            return
        }
        try {
            clock.execute { synchronized(lock) { closeStreamLocked(stream) } }
        } catch (_: RejectedExecutionException) {
            // The listener closed, and every stream with it.
        }
    }

    private fun closeStreamLocked(stream: Long) {
        if (closed) return
        engine.closeStream(stream)
        pumpLocked(tick = true)
    }

    // MARK: - The socket

    private fun receive(socket: DatagramSocket) {
        // A WebRTC datagram fits a path MTU; the buffer is the UDP maximum
        // so that nothing larger is truncated into a different packet.
        val buffer = ByteArray(65_535)
        val packet = DatagramPacket(buffer, buffer.size)
        while (true) {
            try {
                packet.setLength(buffer.size)
                socket.receive(packet)
            } catch (e: Exception) {
                if (!closed) {
                    diagnostic("warning", "WebRTC Direct socket lost", mapOf(
                        "error" to e.javaClass.simpleName,
                    ))
                    onSocketLost(this)
                }
                return
            }
            val source = packet.address?.hostAddress?.substringBefore('%') ?: continue
            val data = packet.data.copyOfRange(packet.offset, packet.offset + packet.length)
            synchronized(lock) {
                if (closed) return
                try {
                    engine.datagramReceived(source, packet.port, data)
                } catch (_: Exception) {
                    // No listener (a close raced the socket): about one
                    // datagram, not the socket.
                }
                pumpLocked(tick = true)
            }
        }
    }

    // MARK: - The pump

    /** Sends what the listener queued, hands its events on, fires its timers, then arms the clock. */
    fun pump() {
        synchronized(lock) {
            if (closed) return
            pumpLocked(tick = true)
        }
    }

    private fun pumpLocked(tick: Boolean) {
        if (pumping) {
            again = true
            return
        }
        pumping = true
        var delay: Long?
        try {
            while (true) {
                again = false
                flush()
                delay = if (tick && !closed) engine.tick() else null
                if (!flush() && !again) break
            }
        } finally {
            pumping = false
        }
        if (tick) arm(delay)
    }

    private fun flush(): Boolean {
        var moved = false
        while (true) {
            val datagram = engine.nextDatagram() ?: break
            moved = true
            sendTo(datagram)
        }
        while (true) {
            val event = engine.nextEvent() ?: break
            moved = true
            try {
                when (event) {
                    is Event.Proved -> sink.proved(this, event.stream, event.address, event.remoteIp)
                    is Event.Body -> sink.body(event.stream, event.data)
                    is Event.Closed -> sink.closed(event.stream)
                }
            } catch (e: Exception) {
                diagnostic("error", "WebRTC Direct event handling failed", mapOf(
                    "error" to e.javaClass.simpleName,
                ))
            }
            // A handler's own calls (a close, a send) queue datagrams.
            while (true) {
                val datagram = engine.nextDatagram() ?: break
                sendTo(datagram)
            }
        }
        return moved
    }

    private fun sendTo(datagram: Datagram) {
        val socket = socket ?: return
        try {
            val to = InetAddress.getByName(datagram.destinationIp)
            socket.send(DatagramPacket(datagram.data, datagram.data.size, to, datagram.destinationPort))
        } catch (_: Exception) {
            // An unreachable page, a full buffer: about one datagram. The
            // page retransmits, and consent ends a stream that stays silent.
        }
    }

    private fun arm(delay: Long?) {
        timer?.cancel(false)
        timer = null
        if (delay == null || closed) return
        timer = try {
            clock.schedule({ pump() }, delay, TimeUnit.MILLISECONDS)
        } catch (_: Exception) {
            null
        }
    }
}
