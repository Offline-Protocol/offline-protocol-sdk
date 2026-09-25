package com.offlineprotocol

import org.json.JSONObject
import org.junit.Assert.assertArrayEquals
import org.junit.Assert.assertEquals
import org.junit.Assert.assertFalse
import org.junit.Assert.assertNull
import org.junit.Assert.assertSame
import org.junit.Assert.assertTrue
import org.junit.Assert.fail
import org.junit.Test
import java.io.ByteArrayInputStream
import java.io.DataInputStream
import java.io.EOFException
import java.io.File

/**
 * Pins the Wi-Fi Direct manager's half of docs/spec/stream-framing.md: the
 * length prefix and its bounds, the position rule for the preamble, and one
 * announced stream per address. Mirrors iOS's PeerStreamFramingTests, keep in
 * sync.
 *
 * The framing cases replay the chapter's conformance vectors
 * (crates/offline-protocol-transport/tests/data/stream-framing-v1.vectors.json),
 * so this reader and the Rust reference are checked against the same bytes.
 * The verifier is a stand-in here: the real one is Rust, needs the native
 * library, and is tested against the identity assertion vectors in Rust.
 */
class PeerStreamFramingTest {

    private val vectors: JSONObject by lazy { JSONObject(vectorFile().readText()) }

    private fun vectorFile(): File {
        val rel = "crates/offline-protocol-transport/tests/data/stream-framing-v1.vectors.json"
        var dir: File? = File("").absoluteFile
        while (dir != null) {
            val candidate = File(dir, rel)
            if (candidate.isFile) return candidate
            dir = dir.parentFile
        }
        error("cannot find $rel above ${File("").absolutePath}")
    }

    private fun hex(s: String): ByteArray =
        ByteArray(s.length / 2) { i -> s.substring(2 * i, 2 * i + 2).toInt(16).toByte() }

    private fun input(bytes: ByteArray) = DataInputStream(ByteArrayInputStream(bytes))

    private val preambleBody by lazy { hex(vectors.getJSONObject("preamble").getString("body_hex")) }
    private val preambleAddress by lazy { vectors.getJSONObject("preamble").getString("address") }

    /** Verifies exactly the vector preamble, as the real verifier would. */
    private val verifier: (ByteArray) -> String = { body ->
        if (body.contentEquals(preambleBody)) preambleAddress else throw IllegalStateException("signature")
    }

    // --- the hand-mirrored sizes (docs/bridges C5) ----------------------------

    @Test
    fun `the sizes are the chapter's`() {
        // Literals, not a read of the vector file: a test that read them from
        // the source it checks would agree with any edit made to both.
        assertEquals(4, PeerStreamFraming.PREFIX_BYTES)
        assertEquals(1_048_576, PeerStreamFraming.MAX_BODY_BYTES)
        assertEquals(96, PeerStreamFraming.PREAMBLE_MIN_BYTES)
        // And the file agrees with the literals, so a vector change shows up.
        assertEquals(1_048_576, vectors.getInt("ceiling"))
        assertEquals(96, vectors.getInt("preamble_floor"))
    }

    // --- the frame -----------------------------------------------------------

    @Test
    fun `frame writes the vector bytes`() {
        for (key in listOf("preamble", "message")) {
            val v = vectors.getJSONObject(key)
            assertArrayEquals(key, hex(v.getString("frame_hex")), PeerStreamFraming.frame(hex(v.getString("body_hex"))))
        }
    }

    @Test
    fun `frame refuses what a receiver would refuse`() {
        for (size in listOf(0, PeerStreamFraming.MAX_BODY_BYTES + 1)) {
            try {
                PeerStreamFraming.frame(ByteArray(size))
                fail("a $size-byte body must not be framed")
            } catch (_: IllegalArgumentException) {
            }
        }
        // The ceiling itself is a valid body.
        assertEquals(4 + 1_048_576, PeerStreamFraming.frame(ByteArray(1_048_576)).size)
    }

    // --- the reader ------------------------------------------------------------

    @Test
    fun `the exchange announces then delivers`() {
        val exchange = vectors.getJSONObject("exchange")
        val frames = exchange.getJSONArray("frames")
        val bytes = (0 until frames.length()).map { hex(frames.getString(it)) }
            .reduce { a, b -> a + b }
        val stream = input(bytes)
        val preamble = PeerStreamPreamble(verifier)

        val first = preamble.accept(PeerStreamFraming.readBody(stream, preamble = true))
        assertEquals(PeerStreamPreamble.Outcome.Announce(exchange.getString("announces")), first)

        val second = preamble.accept(PeerStreamFraming.readBody(stream, preamble = false))
        val delivered = exchange.getJSONArray("delivers").getJSONObject(0)
        assertTrue(second is PeerStreamPreamble.Outcome.Deliver)
        second as PeerStreamPreamble.Outcome.Deliver
        assertEquals(delivered.getString("from"), second.address)
        assertArrayEquals(hex(delivered.getString("body_hex")), second.body)
    }

    @Test
    fun `a prefix of exactly the ceiling is read`() {
        // Regression pin: the reader this replaced refused `length >= 1 MiB`.
        val accepted = vectors.getJSONArray("accepted_prefixes").getJSONObject(0)
        val prefix = hex(accepted.getString("prefix_hex"))
        val body = ByteArray(accepted.getInt("length")) { 7 }
        assertEquals(1_048_576, PeerStreamFraming.readBody(input(prefix + body), preamble = false).size)
    }

    @Test
    fun `every refusal vector closes the stream`() {
        val refusals = vectors.getJSONArray("refusals")
        assertEquals(4, refusals.length())
        for (i in 0 until refusals.length()) {
            val r = refusals.getJSONObject(i)
            val name = r.getString("name")
            val frames = r.getJSONArray("frames")
            val bytes = hex(frames.getString(0))
            // Every vector is the stream's first frame, so it is read as the
            // preamble. A refusal is a thrown Refused from the reader, or a
            // Refuse outcome from the position rule; never an announcement.
            val outcome = try {
                PeerStreamPreamble(verifier).accept(PeerStreamFraming.readBody(input(bytes), preamble = true))
            } catch (_: PeerStreamFraming.Refused) {
                null
            }
            assertTrue(name, outcome == null || outcome is PeerStreamPreamble.Outcome.Refuse)
        }
    }

    @Test
    fun `a refused length is refused before the body is read`() {
        // The length refusals carry no body. A reader that read (or allocated)
        // first would hit the end of input and throw EOF, not Refused.
        for ((prefix, preamble) in listOf(
            "00000000" to false,
            "00100001" to false,
            "80000000" to false, // 2^31 reads negative through readInt
            "ffffffff" to false,
            "0000005f" to true, // under the preamble floor, refused on the prefix
        )) {
            try {
                PeerStreamFraming.readBody(input(hex(prefix)), preamble)
                fail("$prefix must be refused")
            } catch (_: PeerStreamFraming.Refused) {
            } catch (e: EOFException) {
                fail("$prefix was read past: the refusal must come from the prefix alone")
            }
        }
    }

    @Test
    fun `a body shorter than its prefix is not delivered`() {
        val frame = hex(vectors.getJSONObject("message").getString("frame_hex"))
        try {
            PeerStreamFraming.readBody(input(frame.copyOf(frame.size - 1)), preamble = false)
            fail("a partial body must never be handed upward")
        } catch (_: EOFException) {
        }
    }

    // --- the position rule -------------------------------------------------------

    @Test
    fun `a preamble that does not verify is refused and announces nothing`() {
        val preamble = PeerStreamPreamble(verify = { throw IllegalStateException("bad signature") })
        assertTrue(preamble.accept(preambleBody) is PeerStreamPreamble.Outcome.Refuse)
        assertTrue(preamble.awaitingPreamble)
        assertNull(preamble.address)
    }

    @Test
    fun `a preamble under the floor never reaches the verifier`() {
        var called = false
        val preamble = PeerStreamPreamble({ called = true; preambleAddress })
        assertTrue(preamble.accept(ByteArray(95)) is PeerStreamPreamble.Outcome.Refuse)
        assertFalse(called)
    }

    @Test
    fun `a claim is compared exactly`() {
        assertEquals(
            PeerStreamPreamble.Outcome.Announce(preambleAddress),
            PeerStreamPreamble(verifier, expected = preambleAddress).accept(preambleBody),
        )
        assertTrue(
            PeerStreamPreamble(verifier, expected = preambleAddress.uppercase()).accept(preambleBody)
                is PeerStreamPreamble.Outcome.Refuse
        )
    }

    @Test
    fun `our own address played back is refused`() {
        assertTrue(
            PeerStreamPreamble(verifier, localAddress = preambleAddress).accept(preambleBody)
                is PeerStreamPreamble.Outcome.Refuse
        )
    }

    @Test
    fun `a second assertion after the first is a message`() {
        val preamble = PeerStreamPreamble(verifier)
        preamble.accept(preambleBody)
        val again = preamble.accept(preambleBody)
        assertTrue(again is PeerStreamPreamble.Outcome.Deliver)
    }

    // --- one announced stream per address -----------------------------------------

    private val a = "off1qysluvwl5922yctzd0u9gpr06gn3k7ldfvgtwgvn"
    private val b = "off1qyqqqqqqqqqqqqqqqqqqqqqqqqqqqqqqqqn8antf"

    @Test
    fun `the first stream for an address is announced`() {
        val links = PeerStreamLinks<String>()
        assertTrue(links.isEmpty())
        assertEquals(PeerStreamLinks.Announcement<String>(true, null), links.announce("s1", a))
        assertEquals("s1", links.handleFor(a))
        assertFalse(links.isEmpty())
    }

    @Test
    fun `a newer stream supersedes and the core sees one announcement and one loss`() {
        val links = PeerStreamLinks<String>()
        links.announce("old", a)

        val second = links.announce("new", a)
        assertFalse("a second announcement would tell the core nothing new", second.firstForAddress)
        assertSame("old", second.superseded)
        assertEquals("new", links.handleFor(a))

        // The superseded stream's end reports nothing: reporting it would
        // remove the live link the newer stream holds.
        assertNull(links.remove("old"))
        assertEquals("new", links.handleFor(a))

        // The live stream's end is the one loss.
        assertEquals(a, links.remove("new"))
        assertNull(links.handleFor(a))
        assertTrue(links.isEmpty())
    }

    @Test
    fun `a stream that never proved a peer reports nothing`() {
        val links = PeerStreamLinks<String>()
        assertNull(links.remove("never-announced"))
    }

    @Test
    fun `a stream is reported lost once`() {
        val links = PeerStreamLinks<String>()
        links.announce("s1", a)
        assertEquals(a, links.remove("s1"))
        assertNull(links.remove("s1"))
    }

    @Test
    fun `addresses are independent`() {
        val links = PeerStreamLinks<String>()
        links.announce("s1", a)
        assertEquals(PeerStreamLinks.Announcement<String>(true, null), links.announce("s2", b))
        assertEquals(a, links.remove("s1"))
        assertEquals("s2", links.handleFor(b))
    }

    @Test
    fun `removeAll returns only the live streams`() {
        val links = PeerStreamLinks<String>()
        links.announce("old", a)
        links.announce("new", a)
        links.announce("s2", b)
        assertEquals(setOf("new" to a, "s2" to b), links.removeAll().toSet())
        assertTrue(links.isEmpty())
        assertNull(links.remove("new"))
    }

    @Test
    fun `re-announcing a stream is a no-op`() {
        val links = PeerStreamLinks<String>()
        links.announce("s1", a)
        assertEquals(PeerStreamLinks.Announcement<String>(false, null), links.announce("s1", a))
        assertEquals("s1", links.handleFor(a))
    }
}
