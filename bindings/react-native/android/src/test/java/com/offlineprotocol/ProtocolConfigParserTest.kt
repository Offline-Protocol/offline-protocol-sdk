package com.offlineprotocol

import org.junit.Assert.assertFalse
import org.junit.Assert.assertTrue
import org.junit.Test

/**
 * Locks down the wire-format kill-switch parsing: a silent regression here
 * reverts a switch to its default with no error anywhere, which is exactly
 * how the legacy nested-only encryption fields drifted. Read order under
 * test: compactEnvelopeEnabled nested under `encryption` first then top
 * level, binaryWireEnabled top level only, both in camelCase or snake_case.
 */
class ProtocolConfigParserTest {

    private fun parse(json: String) = ProtocolConfigParser.parse(json).coreConfig

    @Test
    fun killSwitchesDefaultOnWhenOmitted() {
        val config = parse("""{"appId":"app","userId":"alice"}""")
        assertTrue(config.binaryWireEnabled)
        assertTrue(config.compactEnvelopeEnabled)
    }

    @Test
    fun killSwitchesReadTheFlatCamelCaseShapeTheJsWrapperSends() {
        val config = parse(
            """{"appId":"app","userId":"alice","binaryWireEnabled":false,"compactEnvelopeEnabled":false}"""
        )
        assertFalse(config.binaryWireEnabled)
        assertFalse(config.compactEnvelopeEnabled)
    }

    @Test
    fun compactEnvelopeReadsItsNestedEncryptionHome() {
        val config = parse(
            """{"appId":"app","userId":"alice","encryption":{"compactEnvelopeEnabled":false}}"""
        )
        assertFalse(config.compactEnvelopeEnabled)
        assertTrue(config.binaryWireEnabled)
    }

    @Test
    fun killSwitchesReadSnakeCase() {
        val config = parse(
            """{"appId":"app","userId":"alice","binary_wire_enabled":false,"encryption":{"compact_envelope_enabled":false}}"""
        )
        assertFalse(config.binaryWireEnabled)
        assertFalse(config.compactEnvelopeEnabled)
    }

    @Test
    fun nestedCompactEnvelopeWinsOverTopLevel() {
        val nestedTrue = parse(
            """{"appId":"app","userId":"alice","compactEnvelopeEnabled":false,"encryption":{"compactEnvelopeEnabled":true}}"""
        )
        assertTrue(nestedTrue.compactEnvelopeEnabled)

        val nestedFalse = parse(
            """{"appId":"app","userId":"alice","compactEnvelopeEnabled":true,"encryption":{"compact_envelope_enabled":false}}"""
        )
        assertFalse(nestedFalse.compactEnvelopeEnabled)
    }

    @Test
    fun encryptionSectionWithoutTheFlagFallsThroughToTopLevel() {
        val config = parse(
            """{"appId":"app","userId":"alice","compactEnvelopeEnabled":false,"encryption":{"enabled":true}}"""
        )
        assertFalse(config.compactEnvelopeEnabled)
    }
}
