package com.offlineprotocol

import org.junit.Assert.assertEquals
import org.junit.Assert.assertFalse
import org.junit.Assert.assertNull
import org.junit.Assert.assertTrue
import org.junit.Test

/**
 * Locks down the create()-config parsing: a silent regression here reverts a
 * field to its default with no error anywhere — exactly what happened to the
 * four encryption flags, which were read nested-only while the JS wrapper
 * sent the flat shape, silently discarding every app-set value. Read order
 * under test: encryption flags and compactEnvelopeEnabled nested under
 * `encryption` first then top level, binaryWireEnabled top level only, all
 * in camelCase or snake_case.
 */
class ProtocolConfigParserTest {

    private fun parse(json: String) = ProtocolConfigParser.parse(json).coreConfig

    @Test
    fun killSwitchesDefaultOnWhenOmitted() {
        val config = parse("""{"appId":"app","userId":"alice"}""")
        assertTrue(config.binaryWireEnabled)
        assertTrue(config.compactEnvelopeEnabled)
        assertTrue(config.richPayloadEnabled)
        assertTrue(config.cryptoRecoveryEnabled)
    }

    @Test
    fun nostrSealingDefaultsOnWhenOmitted() {
        // Defaulting this to false would silently publish the whole protocol
        // envelope — both usernames included — in cleartext to every relay.
        assertTrue(parse("""{"appId":"app","userId":"alice"}""").nostrSealingEnabled)
    }

    @Test
    fun nostrSealingReadsItsNestedTransportHome() {
        val config = parse(
            """{"appId":"app","userId":"alice","transports":{"nostr":{"enabled":true,"sealingEnabled":false}}}"""
        )
        assertFalse(config.nostrSealingEnabled)
    }

    @Test
    fun nostrSealingReadsTheFlatShapeTheJsWrapperSends() {
        // The JS wrapper flattens `transports.nostr.sealingEnabled` to this
        // key; both shapes must parse or an app-set value is dropped.
        assertFalse(
            parse("""{"appId":"app","userId":"alice","nostrSealingEnabled":false}""")
                .nostrSealingEnabled
        )
        assertFalse(
            parse("""{"appId":"app","userId":"alice","nostr_sealing_enabled":false}""")
                .nostrSealingEnabled
        )
        assertFalse(
            parse(
                """{"appId":"app","userId":"alice","transports":{"nostr":{"sealing_enabled":false}}}"""
            ).nostrSealingEnabled
        )
    }

    @Test
    fun nostrColdContactDefaultsOnWhenOmitted() {
        assertTrue(parse("""{"appId":"app","userId":"alice"}""").nostrColdContactEnabled)
    }

    @Test
    fun nostrColdContactReadsBothShapes() {
        // Same nested-home-plus-flat-fallback contract as sealing: the JS
        // wrapper flattens the nested key, so both must parse or an app that
        // deliberately turned publication off keeps publishing.
        assertFalse(
            parse(
                """{"appId":"app","userId":"alice","transports":{"nostr":{"coldContactEnabled":false}}}"""
            ).nostrColdContactEnabled
        )
        assertFalse(
            parse("""{"appId":"app","userId":"alice","nostrColdContactEnabled":false}""")
                .nostrColdContactEnabled
        )
        assertFalse(
            parse("""{"appId":"app","userId":"alice","nostr_cold_contact_enabled":false}""")
                .nostrColdContactEnabled
        )
    }

    @Test
    fun nostrUsernameDiscoveryDefaultsOffWhenOmitted() {
        // OFF by default, unlike the two switches above. Publishing a claim
        // binds a human-readable name to an address in a public place, so an
        // app must opt in rather than inherit it.
        assertFalse(parse("""{"appId":"app","userId":"alice"}""").nostrUsernameDiscoveryEnabled)
    }

    @Test
    fun nostrUsernameDiscoveryReadsBothShapes() {
        // Same nested-home-plus-flat-fallback contract. A parser that missed a
        // shape here would silently reset the flag to its default, which for
        // this one means an app that asked to publish silently does not.
        assertTrue(
            parse(
                """{"appId":"app","userId":"alice","transports":{"nostr":{"usernameDiscoveryEnabled":true}}}"""
            ).nostrUsernameDiscoveryEnabled
        )
        assertTrue(
            parse(
                """{"appId":"app","userId":"alice","transports":{"nostr":{"username_discovery_enabled":true}}}"""
            ).nostrUsernameDiscoveryEnabled
        )
        assertTrue(
            parse("""{"appId":"app","userId":"alice","nostrUsernameDiscoveryEnabled":true}""")
                .nostrUsernameDiscoveryEnabled
        )
        assertTrue(
            parse("""{"appId":"app","userId":"alice","nostr_username_discovery_enabled":true}""")
                .nostrUsernameDiscoveryEnabled
        )
    }

    @Test
    fun nestedNostrUsernameDiscoveryWinsOverTopLevel() {
        val config = parse(
            """{"appId":"app","userId":"alice","nostrUsernameDiscoveryEnabled":true,"transports":{"nostr":{"usernameDiscoveryEnabled":false}}}"""
        )
        assertFalse(config.nostrUsernameDiscoveryEnabled)
    }

    @Test
    fun nestedNostrColdContactWinsOverTopLevel() {
        val config = parse(
            """{"appId":"app","userId":"alice","nostrColdContactEnabled":false,"transports":{"nostr":{"coldContactEnabled":true}}}"""
        )
        assertTrue(config.nostrColdContactEnabled)
    }

    @Test
    fun nestedNostrSealingWinsOverTopLevel() {
        val config = parse(
            """{"appId":"app","userId":"alice","nostrSealingEnabled":false,"transports":{"nostr":{"sealingEnabled":true}}}"""
        )
        assertTrue(config.nostrSealingEnabled)
    }

    @Test
    fun pendingTtlFallsBackToTheRustDefault() {
        // Mirrors DEFAULT_PENDING_TTL_MS (30 min); the iOS reader asserts the
        // same, and `rn_bridge_pending_ttl_fallbacks_match_rust_default` pins
        // all three bridge literals to the Rust constant.
        val config = parse("""{"appId":"app","userId":"alice"}""")
        assertEquals(1_800_000L, config.pendingTtlMs.toLong())
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

    @Test
    fun richPayloadReadsItsNestedEncryptionHomeThenTopLevel() {
        val nested = parse(
            """{"appId":"app","userId":"alice","richPayloadEnabled":true,"encryption":{"richPayloadEnabled":false}}"""
        )
        assertFalse(nested.richPayloadEnabled)

        val flat = parse(
            """{"appId":"app","userId":"alice","richPayloadEnabled":false,"encryption":{"enabled":true}}"""
        )
        assertFalse(flat.richPayloadEnabled)

        val snake = parse(
            """{"appId":"app","userId":"alice","encryption":{"rich_payload_enabled":false}}"""
        )
        assertFalse(snake.richPayloadEnabled)
    }

    @Test
    fun cryptoRecoveryReadsItsNestedEncryptionHomeThenTopLevel() {
        val nested = parse(
            """{"appId":"app","userId":"alice","cryptoRecoveryEnabled":true,"encryption":{"cryptoRecoveryEnabled":false}}"""
        )
        assertFalse(nested.cryptoRecoveryEnabled)

        val flat = parse(
            """{"appId":"app","userId":"alice","cryptoRecoveryEnabled":false,"encryption":{"enabled":true}}"""
        )
        assertFalse(flat.cryptoRecoveryEnabled)

        val snake = parse(
            """{"appId":"app","userId":"alice","encryption":{"crypto_recovery_enabled":false}}"""
        )
        assertFalse(snake.cryptoRecoveryEnabled)
    }

    @Test
    fun encryptionFlagsDefaultOnWhenOmitted() {
        val config = parse("""{"appId":"app","userId":"alice"}""")
        assertTrue(config.encryptionEnabled)
        assertTrue(config.autoKeyExchange)
        assertTrue(config.storePending)
        assertTrue(config.requireEncryption)
    }

    @Test
    fun encryptionFlagsReadTheFlatCamelCaseShapeTheJsWrapperSends() {
        val config = parse(
            """{"appId":"app","userId":"alice","encryptionEnabled":false,"autoKeyExchange":false,"storePending":false,"requireEncryption":false}"""
        )
        assertFalse(config.encryptionEnabled)
        assertFalse(config.autoKeyExchange)
        assertFalse(config.storePending)
        assertFalse(config.requireEncryption)
    }

    @Test
    fun encryptionFlagsReadTheirNestedHome() {
        val config = parse(
            """{"appId":"app","userId":"alice","encryption":{"enabled":false,"autoKeyExchange":false,"storePending":false,"requireEncryption":false}}"""
        )
        assertFalse(config.encryptionEnabled)
        assertFalse(config.autoKeyExchange)
        assertFalse(config.storePending)
        assertFalse(config.requireEncryption)
    }

    @Test
    fun encryptionFlagsReadFlatSnakeCase() {
        val config = parse(
            """{"appId":"app","userId":"alice","encryption_enabled":false,"auto_key_exchange":false,"store_pending":false,"require_encryption":false}"""
        )
        assertFalse(config.encryptionEnabled)
        assertFalse(config.autoKeyExchange)
        assertFalse(config.storePending)
        assertFalse(config.requireEncryption)
    }

    @Test
    fun nestedEncryptionFlagsWinOverFlat() {
        val config = parse(
            """{"appId":"app","userId":"alice","encryptionEnabled":true,"autoKeyExchange":true,"storePending":true,"requireEncryption":true,"encryption":{"enabled":false,"autoKeyExchange":false,"storePending":false,"requireEncryption":false}}"""
        )
        assertFalse(config.encryptionEnabled)
        assertFalse(config.autoKeyExchange)
        assertFalse(config.storePending)
        assertFalse(config.requireEncryption)
    }

    @Test
    fun encryptionSectionWithoutTheFlagsFallsThroughToFlatKeys() {
        val config = parse(
            """{"appId":"app","userId":"alice","encryptionEnabled":false,"requireEncryption":false,"encryption":{"compactEnvelopeEnabled":true}}"""
        )
        assertFalse(config.encryptionEnabled)
        assertFalse(config.requireEncryption)
        assertTrue(config.compactEnvelopeEnabled)
        assertTrue(config.autoKeyExchange)
        assertTrue(config.storePending)
    }

    @Test
    fun groupSectionDefaultsWhenOmitted() {
        val config = parse("""{"appId":"app","userId":"alice"}""")
        assertEquals(256, config.maxGroupMembers.toInt())
        assertTrue(config.groupRelayEnabled)
        // Broadcast defaults on; the core's capability gate is what keeps it
        // off against a v1 relay.
        assertTrue(config.groupRelayBroadcastEnabled)
        // Enforcement defaults OFF. This bridge fallback is what an app that
        // sends no group section actually gets, so it must not drift from the
        // Rust default — on, a lagging admin overlay forks the device out of
        // its own group.
        assertFalse(config.groupEnforceAdminCommits)
    }

    @Test
    fun groupSectionReadsItsNestedHome() {
        val config = parse(
            """{"appId":"app","userId":"alice","group":{"maxGroupMembers":32,"relayEnabled":false,"relayBroadcastEnabled":false,"enforceAdminCommits":true}}"""
        )
        assertEquals(32, config.maxGroupMembers.toInt())
        assertFalse(config.groupRelayEnabled)
        assertFalse(config.groupRelayBroadcastEnabled)
        assertTrue(config.groupEnforceAdminCommits)
    }

    @Test
    fun groupSectionReadsNestedSnakeCase() {
        val config = parse(
            """{"appId":"app","userId":"alice","group":{"max_group_members":48,"relay_enabled":false,"relay_broadcast_enabled":false,"enforce_admin_commits":true}}"""
        )
        assertEquals(48, config.maxGroupMembers.toInt())
        assertFalse(config.groupRelayEnabled)
        assertFalse(config.groupRelayBroadcastEnabled)
        assertTrue(config.groupEnforceAdminCommits)
    }

    @Test
    fun groupFlagsReadTheFlatShape() {
        val config = parse(
            """{"appId":"app","userId":"alice","maxGroupMembers":64,"groupRelayEnabled":false,"groupRelayBroadcastEnabled":false,"groupEnforceAdminCommits":true}"""
        )
        assertEquals(64, config.maxGroupMembers.toInt())
        assertFalse(config.groupRelayEnabled)
        assertFalse(config.groupRelayBroadcastEnabled)
        assertTrue(config.groupEnforceAdminCommits)
    }

    @Test
    fun nestedGroupSectionWinsOverFlat() {
        val config = parse(
            """{"appId":"app","userId":"alice","groupRelayBroadcastEnabled":true,"groupEnforceAdminCommits":true,"group":{"relayBroadcastEnabled":false,"enforceAdminCommits":false}}"""
        )
        assertFalse(config.groupRelayBroadcastEnabled)
        // Nested wins for the new flag too, in the safer direction: a nested
        // opt-out must beat a flat opt-in.
        assertFalse(config.groupEnforceAdminCommits)
        assertTrue(config.groupRelayEnabled)
        assertEquals(256, config.maxGroupMembers.toInt())
    }

    // ------------------------------------------------------------------
    // Mesh forwarding section
    // ------------------------------------------------------------------

    @Test
    fun meshRelaySectionIsAbsentWhenOmitted() {
        // Absent must stay absent, not become an object of nulls: the core
        // owns every default, and a section materialised here would be this
        // parser quietly deciding them instead.
        val config = parse("""{"appId":"app","userId":"alice"}""")
        assertNull(config.meshRelay)
    }

    @Test
    fun meshRelaySectionReadsItsNestedHome() {
        val config = parse(
            """{"appId":"app","userId":"alice","meshRelay":{"maxTtl":6,"denseMaxTtl":4,"denseDegree":9,"fanout":2,"jitterMinMs":35,"jitterMaxMs":175,"ratePerSec":12.5,"burst":41,"peerRatePerSec":6.5,"peerBurst":17,"queueCapacity":321,"biasMinScale":0.4,"biasMaxHandicapMs":275,"activityWindowMs":45000,"activityMinForwards":5,"activityIdleWindows":3}}"""
        )
        val mesh = config.meshRelay!!
        assertEquals(6, mesh.maxTtl!!.toInt())
        assertEquals(4, mesh.denseMaxTtl!!.toInt())
        assertEquals(9L, mesh.denseDegree!!.toLong())
        assertEquals(2L, mesh.fanout!!.toLong())
        assertEquals(35L, mesh.jitterMinMs!!.toLong())
        assertEquals(175L, mesh.jitterMaxMs!!.toLong())
        assertEquals(12.5f, mesh.ratePerSec!!, 0.0001f)
        assertEquals(41f, mesh.burst!!, 0.0001f)
        assertEquals(6.5f, mesh.peerRatePerSec!!, 0.0001f)
        assertEquals(17f, mesh.peerBurst!!, 0.0001f)
        assertEquals(321L, mesh.queueCapacity!!.toLong())
        assertEquals(0.4f, mesh.biasMinScale!!, 0.0001f)
        assertEquals(275L, mesh.biasMaxHandicapMs!!.toLong())
        assertEquals(45000L, mesh.activityWindowMs!!.toLong())
        assertEquals(5L, mesh.activityMinForwards!!.toLong())
        assertEquals(3, mesh.activityIdleWindows!!.toInt())
    }

    @Test
    fun meshRelaySectionReadsNestedSnakeCase() {
        val config = parse(
            """{"appId":"app","userId":"alice","mesh_relay":{"max_ttl":7,"dense_max_ttl":3,"dense_degree":8,"jitter_min_ms":10,"jitter_max_ms":150,"rate_per_sec":9.5,"peer_rate_per_sec":4.5,"peer_burst":12,"queue_capacity":128,"bias_min_scale":0.5,"bias_max_handicap_ms":300,"activity_window_ms":30000,"activity_min_forwards":4,"activity_idle_windows":5}}"""
        )
        val mesh = config.meshRelay!!
        assertEquals(7, mesh.maxTtl!!.toInt())
        assertEquals(3, mesh.denseMaxTtl!!.toInt())
        assertEquals(8L, mesh.denseDegree!!.toLong())
        assertEquals(10L, mesh.jitterMinMs!!.toLong())
        assertEquals(150L, mesh.jitterMaxMs!!.toLong())
        assertEquals(9.5f, mesh.ratePerSec!!, 0.0001f)
        assertEquals(4.5f, mesh.peerRatePerSec!!, 0.0001f)
        assertEquals(12f, mesh.peerBurst!!, 0.0001f)
        assertEquals(128L, mesh.queueCapacity!!.toLong())
        assertEquals(0.5f, mesh.biasMinScale!!, 0.0001f)
        assertEquals(300L, mesh.biasMaxHandicapMs!!.toLong())
        assertEquals(30000L, mesh.activityWindowMs!!.toLong())
        assertEquals(4L, mesh.activityMinForwards!!.toLong())
        assertEquals(5, mesh.activityIdleWindows!!.toInt())
    }

    @Test
    fun meshRelayLeavesUnnamedFieldsNull() {
        // A partial section is the ordinary case: an app sets the one dial it
        // cares about. Every field it did not name must arrive null so the
        // core keeps its own value — this is the DORS silent-reset bug in the
        // shape it would take here.
        val config = parse(
            """{"appId":"app","userId":"alice","meshRelay":{"fanout":7}}"""
        )
        val mesh = config.meshRelay!!
        assertEquals(7L, mesh.fanout!!.toLong())
        assertNull(mesh.maxTtl)
        assertNull(mesh.denseMaxTtl)
        assertNull(mesh.denseDegree)
        assertNull(mesh.jitterMinMs)
        assertNull(mesh.jitterMaxMs)
        assertNull(mesh.ratePerSec)
        assertNull(mesh.burst)
        assertNull(mesh.peerRatePerSec)
        assertNull(mesh.peerBurst)
        assertNull(mesh.queueCapacity)
        assertNull(mesh.biasMinScale)
        assertNull(mesh.biasMaxHandicapMs)
        assertNull(mesh.activityWindowMs)
        assertNull(mesh.activityMinForwards)
        assertNull(mesh.activityIdleWindows)
    }

    @Test
    fun meshRelayNegativesAreClampedRatherThanWrapped() {
        // These fields are unsigned across the FFI. A bare conversion would
        // turn -1 into ~18 quintillion, which the core cannot recognise as
        // wrong; clamped to 0 it hits the core's validation instead.
        val config = parse(
            """{"appId":"app","userId":"alice","meshRelay":{"fanout":-1,"maxTtl":-5,"activityIdleWindows":-2}}"""
        )
        val mesh = config.meshRelay!!
        assertEquals(0L, mesh.fanout!!.toLong())
        assertEquals(0, mesh.maxTtl!!.toInt())
        assertEquals(0, mesh.activityIdleWindows!!.toInt())
    }

    // ------------------------------------------------------------------
    // Data layer section
    // ------------------------------------------------------------------

    @Test
    fun dataLayerIsOffWhenTheSectionIsAbsent() {
        // The layer ships before its replication half, so off is the shipped
        // default and an app that says nothing must get it.
        val config = parse("""{"appId":"app","userId":"alice"}""")
        assertFalse(config.dataEnabled)
    }

    @Test
    fun dataLayerReadsItsNestedHome() {
        val config = parse("""{"appId":"app","userId":"alice","data":{"enabled":true}}""")
        assertTrue(config.dataEnabled)
    }

    @Test
    fun dataLayerCanBeExplicitlyDisabled() {
        // Explicitly off is applied; unset is left alone so the generated
        // default governs. Both end up false today, which is exactly why the
        // difference has to be pinned here rather than assumed: the day the
        // default flips, only one of them is supposed to change.
        val config = parse("""{"appId":"app","userId":"alice","data":{"enabled":false}}""")
        assertFalse(config.dataEnabled)
    }

    @Test
    fun aDataSectionWithNothingInItLeavesTheLayerOff() {
        val config = parse("""{"appId":"app","userId":"alice","data":{}}""")
        assertFalse(config.dataEnabled)
    }

    @Test
    fun anUnrelatedSectionDoesNotTurnTheDataLayerOn() {
        // Guards the shape of the lookup: reading the flag from the wrong
        // place (a top-level `enabled`, say) would make unrelated config
        // switch the layer on.
        val config = parse("""{"appId":"app","userId":"alice","enabled":true}""")
        assertFalse(config.dataEnabled)
    }
}
