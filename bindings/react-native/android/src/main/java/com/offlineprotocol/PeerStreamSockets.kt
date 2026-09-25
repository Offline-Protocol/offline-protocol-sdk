package com.offlineprotocol

import java.io.BufferedInputStream
import java.io.DataInputStream
import java.io.EOFException
import java.net.Socket
import java.util.concurrent.ConcurrentHashMap
import java.util.concurrent.ExecutorService
import java.util.concurrent.Executors
import java.util.concurrent.RejectedExecutionException
import java.util.concurrent.ScheduledExecutorService
import java.util.concurrent.TimeUnit
import java.util.concurrent.atomic.AtomicLong

/**
 * docs/spec/stream-framing.md over plain TCP sockets: what happens to a socket
 * from its first byte to its end, and how a body reaches the right one.
 *
 * [WifiDirectManager] owns the Wi-Fi Direct group, the listener and the
 * connect; it hands each socket to [run] and each outbound body to [send].
 * Everything between is here, behind [Host], so that
 * `PeerStreamSocketsTest` drives it on real loopback sockets without a group
 * or the native library. Mirrors the per-peer half of iOS's
 * PeerStreamSession.swift, keep in sync.
 *
 * The rules, each of which a test pins:
 * - Our preamble goes first, without waiting for the peer's, and the peer's
 *   must verify within [Limits.preambleTimeoutMs], one deadline over the
 *   prefix and the body together.
 * - The host is told only the address the preamble proved:
 *   [Host.peerConnected] once per address, each body attributed to it, and
 *   [Host.peerDisconnected] once, if and only if the stream still held the
 *   address when it ended.
 * - One announced stream per address, newer superseding older, through
 *   [PeerStreamLinks]. A superseded stream is closed and reports nothing.
 * - A stream's announcement, deliveries and loss report run under that
 *   stream's lock, so no body reaches the host after its loss report. The
 *   core re-adds a neighbour on any inbound body, so a delivery that raced
 *   past the report would resurrect a peer just reported gone.
 * - A refused frame closes the stream. Nothing reads on past a refusal.
 * - Each stream has its own ordered writer, so frames never interleave and a
 *   peer that stops reading stalls only itself. The write deadline is on
 *   progress, and the queue bound drops only while the writer is stalled.
 */
internal class PeerStreamSockets(
    private val host: Host,
    private val limits: Limits = Limits(),
) {
    /** The core, as far as a stream is concerned. */
    interface Host {
        /** This device's identity assertion, sent as our preamble. Throws without an identity. */
        fun identityAssertion(): ByteArray

        /** Our own address, so a copy of our preamble played back is refused. */
        fun localAddress(): String?

        /** `verifyIdentityAssertion`: the derived address, or a throw. */
        fun verify(assertion: ByteArray): String

        fun peerConnected(address: String)
        fun messageReceived(address: String, body: ByteArray)
        fun peerDisconnected(address: String)
        fun diagnostic(level: String, message: String, context: Map<String, Any?>)
    }

    /**
     * Local policy (the chapter leaves all of it to the implementation).
     * Tests shrink the times; production uses the defaults.
     */
    data class Limits(
        /** How long an open socket may take to prove its peer, prefix and body together. */
        val preambleTimeoutMs: Long = 10_000L,
        /**
         * How long a writer may make no progress before the socket is closed.
         * Progress, not a per-frame deadline: a per-frame deadline is a
         * throughput floor (1 MiB in thirty seconds), and a link slower than
         * it would be aborted, reconnected and handed the same frame forever.
         * A peer that stops reading fills both kernel buffers first, so this
         * notices it late; the core's acknowledgements are the real signal.
         */
        val writeTimeoutMs: Long = 30_000L,
        /** A frame is written in pieces of this size; each is one unit of progress. */
        val writeChunkBytes: Int = 64 * 1024,
        /** A writer blocked on one piece this long is stalled on the peer. */
        val writeStallMs: Long = 2_000L,
        /** Open sockets, proved or not. A group is a handful of devices. */
        val maxStreams: Int = 16,
        /**
         * Queued bytes toward one peer beyond which a body is dropped (the
         * core retries it), and only while that peer's writer is stalled. The
         * core hands a burst over in one tick, before the writer has run, so
         * a bound applied regardless drops frames for a peer that is reading.
         */
        val maxQueuedBytes: Long = 4L * PeerStreamFraming.MAX_BODY_BYTES,
    )

    enum class SendResult { QUEUED, NO_STREAM, OUT_OF_BOUNDS, QUEUE_FULL }

    private val openStreams: MutableSet<Stream> = ConcurrentHashMap.newKeySet()
    private val links = PeerStreamLinks<Stream>()

    // Daemon, like the stream threads, so an instance dropped without
    // closeAll() does not pin the process.
    private val deadlines: ScheduledExecutorService =
        Executors.newSingleThreadScheduledExecutor { r ->
            Thread(r, "offline-peerstream-deadline").apply { isDaemon = true }
        }

    /** Whether any stream has proved a peer: nothing is sendable otherwise. */
    fun isEmpty(): Boolean = links.isEmpty()

    /** Open sockets, proved or not. */
    val openCount: Int get() = openStreams.size

    /**
     * Runs [socket] to its end on the calling thread, and returns the address
     * it proved, or null if it proved none.
     *
     * [accepting] is the owner's "still running": a socket that arrives while
     * it is false, or past [Limits.maxStreams], is closed unread.
     */
    fun run(socket: Socket, outbound: Boolean, accepting: () -> Boolean): String? {
        val endpoint = socket.remoteSocketAddress?.toString() ?: "unknown"
        if (!accepting() || openStreams.size >= limits.maxStreams) {
            // Diagnostics may carry the endpoint; the host is never told it.
            host.diagnostic("warning", "Peer stream refused", mapOf(
                "socket" to endpoint,
                "reason" to (if (!accepting()) "not running" else "at stream limit"),
            ))
            closeQuietly(socket)
            return null
        }
        val stream = Stream(socket)
        openStreams.add(stream)
        // Re-checked after the add: a closeAll() that snapshotted the set just
        // before it cannot miss this stream, because the owner stops
        // accepting before it closes everything.
        if (!accepting()) {
            endStream(stream)
            return null
        }

        var proved: String? = null
        var why = "ended"
        try {
            try { socket.tcpNoDelay = true } catch (_: Exception) {}
            try { socket.keepAlive = true } catch (_: Exception) {}

            // Ours goes first and without waiting for theirs, so neither side
            // can hold the other half-open by staying silent.
            val assertion = try {
                host.identityAssertion()
            } catch (e: Exception) {
                why = "no identity to present: ${e.message ?: "unknown"}"
                return null
            }
            stream.enqueue(PeerStreamFraming.frame(assertion))

            val preamble = PeerStreamPreamble(
                verify = host::verify,
                localAddress = try { host.localAddress() } catch (_: Exception) { null },
            )
            val input = DataInputStream(BufferedInputStream(socket.getInputStream()))

            val deadline = deadlines.schedule(
                { stream.abort() }, limits.preambleTimeoutMs, TimeUnit.MILLISECONDS
            )
            val address = try {
                val body = PeerStreamFraming.readBody(input, preamble = true)
                when (val outcome = preamble.accept(body)) {
                    is PeerStreamPreamble.Outcome.Announce -> outcome.address
                    is PeerStreamPreamble.Outcome.Refuse -> {
                        why = "preamble refused: ${outcome.reason}"
                        return null
                    }
                    is PeerStreamPreamble.Outcome.Deliver -> {
                        why = "preamble state out of order"
                        return null
                    }
                }
            } finally {
                deadline.cancel(false)
            }
            if (!announce(stream, address)) {
                why = "ended before the announcement"
                return null
            }
            proved = address
            host.diagnostic("info", "Peer stream proved", mapOf(
                "address" to address,
                "outbound" to outbound,
            ))

            while (true) {
                val body = PeerStreamFraming.readBody(input, preamble = false)
                if (!deliver(stream, address, body)) {
                    why = "superseded or ended"
                    return proved
                }
            }
        } catch (e: PeerStreamFraming.Refused) {
            why = "frame refused: ${e.reason}"
        } catch (_: EOFException) {
            why = "peer closed"
        } catch (e: Exception) {
            why = if (socket.isClosed) "closed" else "read failed: ${e.message ?: "unknown"}"
        } finally {
            endStream(stream)
            host.diagnostic("info", "Peer stream closed", mapOf(
                "socket" to endpoint,
                "reason" to why,
            ))
        }
        return proved
    }

    /**
     * Frames [body] and queues it on the stream that proved [recipient].
     *
     * Never broadcasts. The core returns a body only for an address a stream
     * proved, so [SendResult.NO_STREAM] means that stream ended between the
     * core's answer and this lookup; the core's acknowledgement and retry
     * cover the body. The broadcast fallback the Wi-Fi Direct manager used to
     * have handed a message for one peer to every peer in the group.
     */
    fun send(recipient: String, body: ByteArray): SendResult {
        val stream = links.handleFor(recipient) ?: return SendResult.NO_STREAM
        val frame = try {
            PeerStreamFraming.frame(body)
        } catch (_: IllegalArgumentException) {
            return SendResult.OUT_OF_BOUNDS
        }
        return if (stream.enqueue(frame)) SendResult.QUEUED else SendResult.QUEUE_FULL
    }

    /** Ends every stream, reporting each announced one lost, on the calling thread. */
    fun closeAll() {
        for (stream in openStreams.toList()) {
            endStream(stream)
        }
    }

    /**
     * Enters [stream] as the one announced stream for [address], and tells
     * the host if no stream held it. False when the stream was ended first,
     * in which case nothing was announced.
     */
    private fun announce(stream: Stream, address: String): Boolean {
        synchronized(stream.lock) {
            if (stream.closed) return false
            val announcement = links.announce(stream, address)
            announcement.superseded?.let { older ->
                // Closed without a loss report: `links` already moved the
                // address here, so the older stream's end finds nothing.
                host.diagnostic("info", "Peer stream superseded", mapOf("address" to address))
                older.abort()
            }
            if (announcement.firstForAddress) {
                try {
                    host.peerConnected(address)
                } catch (e: Exception) {
                    host.diagnostic("error", "Error announcing peer", mapOf(
                        "error" to (e.message ?: "unknown"),
                    ))
                }
            }
            return true
        }
    }

    /**
     * Hands one body to the host attributed to [address]. False once the
     * stream no longer holds the address (ended, or superseded), and the
     * reader stops.
     */
    private fun deliver(stream: Stream, address: String, body: ByteArray): Boolean {
        synchronized(stream.lock) {
            if (stream.closed || links.addressOf(stream) == null) return false
            try {
                host.messageReceived(address, body)
            } catch (e: Exception) {
                // The core refused this body (malformed, unattributed, ...).
                // That is about the message, not the stream: keep reading.
                host.diagnostic("warning", "Peer stream body refused by the core", mapOf(
                    "address" to address,
                    "error" to (e.message ?: "unknown"),
                ))
            }
            return true
        }
    }

    /**
     * Ends [stream] once, from whichever side gets there first: its reader,
     * or [closeAll]. Reports the loss if the stream still held its address,
     * under the lock the deliveries take.
     */
    private fun endStream(stream: Stream) {
        synchronized(stream.lock) {
            if (stream.closed) return
            stream.closed = true
            links.remove(stream)?.let { address ->
                try {
                    host.peerDisconnected(address)
                } catch (e: Exception) {
                    host.diagnostic("error", "Error reporting peer lost", mapOf(
                        "error" to (e.message ?: "unknown"),
                    ))
                }
            }
        }
        openStreams.remove(stream)
        stream.abort()
        stream.shutdownWriter()
    }

    private fun closeQuietly(socket: Socket) {
        try { socket.close() } catch (_: Exception) {}
    }

    /**
     * One socket and the ordered writer that owns its output.
     *
     * Every write goes through [writer], one thread per stream, so frames
     * never interleave (a shared pool can run two writes to one socket at once
     * and splice a prefix into another frame's body) and a peer that stops
     * reading blocks only its own queue.
     */
    private inner class Stream(val socket: Socket) {
        /** Guards [closed] and the stream's calls into the host. */
        val lock = Any()
        var closed = false

        private val writer: ExecutorService = Executors.newSingleThreadExecutor { r ->
            Thread(r, "offline-peerstream-writer").apply { isDaemon = true }
        }
        private val queuedBytes = AtomicLong(0)
        // When the piece being written started, or 0 when the writer is idle.
        @Volatile private var pieceStartedAt = 0L

        private fun stalled(): Boolean {
            val started = pieceStartedAt
            return started != 0L && System.currentTimeMillis() - started > limits.writeStallMs
        }

        /** Queues one frame. False when the stream ended, or the peer is stalled and the budget spent. */
        fun enqueue(frame: ByteArray): Boolean {
            val size = frame.size.toLong()
            if (queuedBytes.addAndGet(size) > limits.maxQueuedBytes && stalled()) {
                queuedBytes.addAndGet(-size)
                return false
            }
            return try {
                writer.execute { write(frame) }
                true
            } catch (_: RejectedExecutionException) {
                queuedBytes.addAndGet(-size)
                false
            }
        }

        private fun write(frame: ByteArray) {
            try {
                if (socket.isClosed) return
                val out = socket.getOutputStream()
                var offset = 0
                while (offset < frame.size) {
                    val n = minOf(limits.writeChunkBytes, frame.size - offset)
                    pieceStartedAt = System.currentTimeMillis()
                    val deadline = deadlines.schedule(
                        { abort() }, limits.writeTimeoutMs, TimeUnit.MILLISECONDS
                    )
                    try {
                        out.write(frame, offset, n)
                    } finally {
                        deadline.cancel(false)
                    }
                    offset += n
                }
                out.flush()
            } catch (_: Exception) {
                // A failed write ends the stream: the reader wakes on the
                // closed socket and does the bookkeeping.
                abort()
            } finally {
                pieceStartedAt = 0L
                queuedBytes.addAndGet(-frame.size.toLong())
            }
        }

        /** Closes the socket. The reader's end does everything else. */
        fun abort() {
            closeQuietly(socket)
        }

        fun shutdownWriter() {
            writer.shutdownNow()
        }
    }
}
