package com.offlineprotocol

import org.junit.Assert.assertEquals
import org.junit.Assert.assertFalse
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
}
