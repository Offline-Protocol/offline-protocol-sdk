package com.offlineprotocol

import org.junit.Assert.assertNotNull
import org.junit.Assert.assertNull
import org.junit.Test
import org.junit.runner.RunWith
import org.robolectric.RobolectricTestRunner
import org.robolectric.annotation.Config

/**
 * [FastLinkGroupConfig]: a fast-link offer names any network the engine
 * accepts, and an Android joiner must turn one it cannot make into a refusal,
 * not a crash (ADR 0038).
 */
@RunWith(RobolectricTestRunner::class)
@Config(sdk = [34])
class FastLinkGroupConfigTest {
    private val passphrase = "0123456789abcdef0123456789abcdef"

    @Test
    fun `the engine's own name builds a group`() {
        assertNotNull(FastLinkGroupConfig.build("DIRECT-5d-6582af4ee017", passphrase))
    }

    @Test
    fun `a name android refuses is null, never a throw`() {
        assertNull(FastLinkGroupConfig.build("HomeNetwork", passphrase))
        assertNull(FastLinkGroupConfig.build("DIRECT-", passphrase))
    }

    @Test
    fun `a passphrase android refuses is null, never a throw`() {
        assertNull(FastLinkGroupConfig.build("DIRECT-5d-6582af4ee017", "short"))
    }

    @Test
    @Config(sdk = [28])
    fun `below android 10 there is no group by name`() {
        assertNull(FastLinkGroupConfig.build("DIRECT-5d-6582af4ee017", passphrase))
    }
}
