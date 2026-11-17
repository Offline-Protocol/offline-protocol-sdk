package com.offlineprotocol

import org.junit.Assert.assertEquals
import org.junit.Test

/** `wifiDirect.fastLink`: a typo never widens what the application allowed. */
class FastLinkModeTest {
    @Test
    fun `each named mode parses`() {
        assertEquals(FastLinkMode.ANY, FastLinkMode.parse("any", FastLinkMode.OFF))
        assertEquals(FastLinkMode.REQUESTED_ONLY, FastLinkMode.parse("requestedOnly", FastLinkMode.ANY))
        assertEquals(FastLinkMode.OFF, FastLinkMode.parse("off", FastLinkMode.ANY))
    }

    @Test
    fun `an enable that names no mode keeps the current one`() {
        assertEquals(FastLinkMode.REQUESTED_ONLY, FastLinkMode.parse(null, FastLinkMode.REQUESTED_ONLY))
    }

    @Test
    fun `an unknown mode is off`() {
        assertEquals(FastLinkMode.OFF, FastLinkMode.parse("Any", FastLinkMode.ANY))
        assertEquals(FastLinkMode.OFF, FastLinkMode.parse("requested_only", FastLinkMode.ANY))
    }
}
