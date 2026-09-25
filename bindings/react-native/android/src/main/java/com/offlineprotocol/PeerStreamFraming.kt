package com.offlineprotocol

import java.io.DataInputStream
import java.io.IOException

/**
 * The parts of `docs/spec/stream-framing.md` a platform manager owns: the
 * length prefix, its bounds, and the position rule that makes the first body
 * on a stream the peer's identity assertion.
 *
 * Framework-free on purpose, so the rules are unit-tested without a Wi-Fi
 * Direct group ([PeerStreamFramingTest] replays the chapter's conformance
 * vectors). [WifiDirectManager] owns the sockets and calls into this.
 * Mirrors iOS's PeerStreamFraming.swift, keep in sync.
 *
 * The three sizes are hand-mirrored from the chapter and from the transport
 * crate's `DEFAULT_MAX_MESSAGE_SIZE` and `IDENTITY_ASSERTION_MIN_LEN`, and
 * pinned as literals by the unit test (docs/bridges C5).
 */
object PeerStreamFraming {
    /** A `u32` big-endian length precedes every body. */
    const val PREFIX_BYTES = 4

    /**
     * The inclusive ceiling on a body, 1 MiB. A body of exactly this many
     * bytes is a valid frame: the reader this replaced refused it with `<`,
     * which dropped the largest message a conforming sender may send.
     */
    const val MAX_BODY_BYTES = 1_048_576

    /** The identity assertion's own floor: a key and a signature. */
    const val PREAMBLE_MIN_BYTES = 96

    /** Why a length or a preamble was refused. The stream closes in every case. */
    class Refused(val reason: String) : IOException(reason)

    /**
     * The one frame [body] travels as. Refuses a body the far side would
     * refuse, so a local bug fails here rather than as a peer that closes.
     */
    fun frame(body: ByteArray): ByteArray {
        require(body.isNotEmpty() && body.size <= MAX_BODY_BYTES) {
            "a frame body is 1..$MAX_BODY_BYTES bytes, not ${body.size}"
        }
        val out = ByteArray(PREFIX_BYTES + body.size)
        val n = body.size
        out[0] = (n ushr 24).toByte()
        out[1] = (n ushr 16).toByte()
        out[2] = (n ushr 8).toByte()
        out[3] = n.toByte()
        System.arraycopy(body, 0, out, PREFIX_BYTES, n)
        return out
    }

    /**
     * The refusal for a prefix, or null if a body of [length] may be read.
     *
     * [length] is the prefix as `DataInputStream.readInt` returns it, which is
     * signed: a prefix of 2^31 or more arrives negative, and is refused with
     * the zero case rather than read as a huge allocation.
     */
    fun refusalFor(length: Int, preamble: Boolean): String? = when {
        length <= 0 -> "zero or negative length"
        length > MAX_BODY_BYTES -> "length over the ceiling"
        preamble && length < PREAMBLE_MIN_BYTES -> "preamble under the floor"
        else -> null
    }

    /**
     * Reads one whole frame's body.
     *
     * Throws [Refused] on a bad prefix before a byte of the body is read or
     * allocated, and `EOFException` when the stream ends mid-frame. Either way
     * the caller closes the stream: a reader that refuses a length and keeps
     * reading takes the skipped body's first four bytes as the next length,
     * which is the second fault the chapter records in the reader this
     * replaced.
     */
    fun readBody(input: DataInputStream, preamble: Boolean): ByteArray {
        val length = input.readInt()
        refusalFor(length, preamble)?.let { throw Refused(it) }
        val body = ByteArray(length)
        input.readFully(body)
        return body
    }
}

/**
 * The position rule for one stream: the first body is the peer's identity
 * assertion, and every later body is a message from the address it proved.
 *
 * [verify] is `verifyIdentityAssertion` in production (steps one to three of
 * the Bluetooth LE chapter: parse, check the signature, derive) and returns
 * the derived address or throws. [expected] is a claim the stream was opened
 * toward, if any (step four); [localAddress] is ours, because a preamble that
 * proves our own address is a copy of ours played back to us.
 *
 * Not thread-safe. One stream's reader owns its instance.
 */
class PeerStreamPreamble(
    private val verify: (ByteArray) -> String,
    private val expected: String? = null,
    private val localAddress: String? = null,
) {
    sealed class Outcome {
        /** The preamble verified: announce [address]. */
        data class Announce(val address: String) : Outcome()

        /** A message from the proved address: hand [body] upward. */
        class Deliver(val address: String, val body: ByteArray) : Outcome()

        /** Close the stream and announce nothing. */
        data class Refuse(val reason: String) : Outcome()
    }

    /** The proved address, once the preamble verified. */
    var address: String? = null
        private set

    val awaitingPreamble: Boolean get() = address == null

    fun accept(body: ByteArray): Outcome {
        address?.let { return Outcome.Deliver(it, body) }

        if (body.size < PeerStreamFraming.PREAMBLE_MIN_BYTES) {
            return Outcome.Refuse("preamble under the floor")
        }
        val derived = try {
            verify(body)
        } catch (e: Exception) {
            return Outcome.Refuse("preamble did not verify: ${e.message ?: e.javaClass.simpleName}")
        }
        // Exact, never case-folded: the Bluetooth LE chapter's reason holds.
        if (expected != null && derived != expected) {
            return Outcome.Refuse("preamble proved an address other than the one expected")
        }
        if (localAddress != null && derived == localAddress) {
            return Outcome.Refuse("preamble proved our own address")
        }
        address = derived
        return Outcome.Announce(derived)
    }
}

/**
 * One announced stream per address (the chapter's "What a receiver owes").
 *
 * The core keys its peer-stream links by address, so a second announcement is
 * not a second link, and the first loss report removes the only one. A copied
 * preamble is enough to open a second stream for a live address, so the count
 * has to be kept here, where the streams are.
 *
 * Policy: the newer stream supersedes the older. On a phone the duplicate is
 * almost always the same peer reconnecting past a stream that went half-open
 * (a group client that re-joined, a Multipeer peer that restarted), and
 * refusing the newer one would leave that peer unreachable until the stale
 * stream's socket noticed. The cost, recorded in R16, is that a replayer can
 * choose when a real stream ends; it cannot use the stream it gets.
 *
 * Thread-safe, and deliberately knows nothing of the protocol: the send path
 * reads it from a thread the core may be calling from while holding its global
 * mutex, so nothing may ever call the core while holding this lock.
 */
class PeerStreamLinks<H : Any> {
    /** What [announce] decided. */
    data class Announcement<H>(
        /** True when no stream held this address: tell the core. */
        val firstForAddress: Boolean,
        /** The older stream for the same address, to close without a loss report. */
        val superseded: H?,
    )

    private val byAddress = HashMap<String, H>()
    private val byHandle = HashMap<H, String>()

    @Synchronized
    fun announce(handle: H, address: String): Announcement<H> {
        byHandle[handle]?.let { held ->
            // A stream proves one address, once. Re-announcing the same one is
            // a caller bug answered as a no-op, which keeps the count right.
            check(held == address) { "a stream cannot prove two addresses" }
            return Announcement(firstForAddress = false, superseded = null)
        }
        val older = byAddress.put(address, handle)
        byHandle[handle] = address
        if (older != null) byHandle.remove(older)
        return Announcement(firstForAddress = older == null, superseded = older)
    }

    /**
     * Forgets [handle] and returns the address to report lost, or null when
     * there is nothing to report: the stream never proved a peer, or a newer
     * stream for the same address superseded it and still holds the link.
     */
    @Synchronized
    fun remove(handle: H): String? {
        val address = byHandle.remove(handle) ?: return null
        if (byAddress[address] == handle) byAddress.remove(address)
        return address
    }

    /** Forgets every stream, returning each announced one with its address. */
    @Synchronized
    fun removeAll(): List<Pair<H, String>> {
        val live = byAddress.map { (address, handle) -> handle to address }
        byAddress.clear()
        byHandle.clear()
        return live
    }

    @Synchronized
    fun handleFor(address: String): H? = byAddress[address]

    @Synchronized
    fun addressOf(handle: H): String? = byHandle[handle]

    @Synchronized
    fun isEmpty(): Boolean = byAddress.isEmpty()
}
