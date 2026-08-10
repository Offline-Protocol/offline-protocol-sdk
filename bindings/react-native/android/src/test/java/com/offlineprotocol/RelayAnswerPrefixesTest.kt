package com.offlineprotocol

import org.junit.Assert.assertEquals
import org.junit.Assert.assertFalse
import org.junit.Assert.assertNull
import org.junit.Assert.assertTrue
import org.junit.Test

/**
 * Pins the "a relay answer reaches the core unattributed" rule against the
 * core's RELAY_ANSWER_PREFIXES. Mirrors iOS's RelayAnswerPrefixesTests — keep
 * in sync.
 *
 * Regression pin: `__GROUP_MEMBER_ADDED__` and `__GROUP_MEMBER_REMOVED__` were
 * injected with the relay-reported actor (`added_by` / `removed_by`) as a
 * reachability assertion. Once control traffic became unconditionally
 * signature-gated that attribution started failing the core's exemption — it
 * requires no transport peer identity — so every legitimate relay membership
 * notification was dropped as unsigned and raised `UNSIGNED_CONTROL_REJECTED`.
 */
class RelayAnswerPrefixesTest {

    /**
     * The list must match `RELAY_ANSWER_PREFIXES` in
     * `crates/offline-protocol/src/protocol/prefixes.rs` exactly. A prefix here
     * that the core does not exempt is dropped as unsigned; a prefix the core
     * exempts that is missing here gets attributed and dropped the same way.
     */
    @Test
    fun `list matches the core constant`() {
        assertEquals(
            setOf(
                "__GROUP_CREATED__",
                "__GROUP_MEMBER_ADDED__",
                "__GROUP_MEMBER_REMOVED__",
                "__GROUP_INFO__",
                "__USER_GROUPS__",
                "__GROUP_ERROR__"
            ),
            RelayAnswerPrefixes.ALL
        )
    }

    /** The regression itself: these two carried an actor and must not. */
    @Test
    fun `membership answers are never attributed`() {
        for (prefix in listOf("__GROUP_MEMBER_ADDED__", "__GROUP_MEMBER_REMOVED__")) {
            assertNull(
                "$prefix must reach the core unattributed or it is dropped as unsigned",
                RelayAnswerPrefixes.attributableActor(prefix, "alice")
            )
        }
    }

    @Test
    fun `every relay answer drops its actor`() {
        for (prefix in RelayAnswerPrefixes.ALL) {
            assertNull(RelayAnswerPrefixes.attributableActor(prefix, "alice"))
            assertNull(RelayAnswerPrefixes.attributableActor(prefix, null))
        }
    }

    /**
     * The rule is scoped, not blanket. `__GROUP_MSG__` is a data-plane prefix —
     * never signature-gated, because MLS authenticates it afterwards — so it
     * keeps its attribution and stays the reachability signal for a relayed
     * sender. Nulling it here would be a silent regression of that seam.
     */
    @Test
    fun `data plane and peer prefixes keep their actor`() {
        for (prefix in listOf("__GROUP_MSG__", "__CONN_REQ__", "__MLS_ENC__")) {
            assertEquals(
                "$prefix is not a relay answer and must keep its attribution",
                "alice",
                RelayAnswerPrefixes.attributableActor(prefix, "alice")
            )
        }
    }

    @Test
    fun `isRelayAnswer discriminates`() {
        assertTrue(RelayAnswerPrefixes.isRelayAnswer("__GROUP_CREATED__"))
        assertFalse(RelayAnswerPrefixes.isRelayAnswer("__GROUP_MSG__"))
        // Matched whole, not by prefix-of-prefix: a crafted content string
        // starting with an exempt prefix is a different question, decided in
        // the core against the frame's transport and attribution.
        assertFalse(RelayAnswerPrefixes.isRelayAnswer("__GROUP_CREATED__extra"))
    }
}
