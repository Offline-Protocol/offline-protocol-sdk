package com.offlineprotocol

import org.json.JSONObject
import org.junit.Assert.assertEquals
import org.junit.Assert.assertFalse
import org.junit.Assert.assertTrue
import org.junit.Test

class RelayControlOpTranslatorTest {

    private fun frames(t: RelayControlOpTranslator.Translation): List<JSONObject> = when (t) {
        is RelayControlOpTranslator.Translation.Replace -> t.frames
        is RelayControlOpTranslator.Translation.Tap -> t.frames
        is RelayControlOpTranslator.Translation.PassThrough -> emptyList()
    }

    /** Simulates InternetManager writing every frame successfully. */
    private fun commit(t: RelayControlOpTranslator.Translation) {
        when (t) {
            is RelayControlOpTranslator.Translation.Replace -> t.commit?.invoke()
            is RelayControlOpTranslator.Translation.Tap -> t.commit?.invoke()
            is RelayControlOpTranslator.Translation.PassThrough -> Unit
        }
    }

    /**
     * Connection ops are no longer server-plane: they ship verbatim as
     * signed SendMessage frames, so the translator must pass them through
     * untouched (they should never even be tagged by the core).
     */
    @Test
    fun connectionOpsPassThroughVerbatim() {
        val translator = RelayControlOpTranslator("alice")
        val cases = listOf(
            "conn_req" to """{"sender_name":"Alice","timestamp_ms":1,"key_package":[1,2,3]}""",
            "conn_acc" to """{"accepted_by_name":"Alice","timestamp_ms":1}""",
            "conn_rej" to "",
            "conn_can" to ""
        )
        for ((op, payload) in cases) {
            assertTrue(
                "$op must pass through as a verbatim SendMessage",
                translator.translate(op, payload, "bob")
                    is RelayControlOpTranslator.Translation.PassThrough
            )
        }
    }

    @Test
    fun registerTranslatesToCreateGroupPlusMemberDeltas() {
        val translator = RelayControlOpTranslator("alice")

        val firstTranslation = translator.translate(
            "group_relay_register",
            """{"group_id":"g1","group_name":"Trip","members":["alice","bob","carol"]}""",
            "alice"
        )
        val first = frames(firstTranslation)
        assertEquals("CreateGroup", first[0].getString("type"))
        assertEquals("g1", first[0].getString("group_id"))
        assertEquals("Trip", first[0].getString("name"))
        // Self never appears in deltas (the relay adds the creator itself),
        // and deltas are sorted for a deterministic wire order.
        val added = first.drop(1).map { it.getString("type") to it.getString("username") }
        assertEquals(
            listOf("AddGroupMember" to "bob", "AddGroupMember" to "carol"),
            added
        )
        commit(firstTranslation)

        // Re-registration after a membership change sends only the deltas.
        val second = frames(
            translator.translate(
                "group_relay_register",
                """{"group_id":"g1","members":["alice","bob","dave"]}""",
                "alice"
            )
        )
        assertEquals("CreateGroup", second[0].getString("type"))
        // Name falls back to group_id when the payload omits it.
        assertEquals("g1", second[0].getString("name"))
        val deltas = second.drop(1).map { it.getString("type") to it.getString("username") }
        assertEquals(
            setOf("AddGroupMember" to "dave", "RemoveGroupMember" to "carol"),
            deltas.toSet()
        )
    }

    @Test
    fun uncommittedRegistrationResendsDeltas() {
        val translator = RelayControlOpTranslator("alice")

        // The frames were produced but never fully written (a best-effort
        // delta dropped): the commit must not run, so the next registration
        // re-sends the missing deltas instead of assuming them applied —
        // otherwise that member is silently missing from relay fan-out for
        // the rest of the connection.
        translator.translate(
            "group_relay_register",
            """{"group_id":"g1","members":["alice","bob"]}""",
            "alice"
        )

        val retry = frames(
            translator.translate(
                "group_relay_register",
                """{"group_id":"g1","members":["alice","bob"]}""",
                "alice"
            )
        )
        assertEquals(2, retry.size)
        assertEquals("AddGroupMember", retry[1].getString("type"))
        assertEquals("bob", retry[1].getString("username"))
    }

    @Test
    fun adminDeniedGroupsStopProducingMemberDeltas() {
        val translator = RelayControlOpTranslator("bob")

        commit(
            translator.translate(
                "group_relay_register",
                """{"group_id":"g1","members":["alice","bob","carol"]}""",
                "bob"
            )
        )
        translator.onGroupError("g1", "Only admins can add members")

        val after = frames(
            translator.translate(
                "group_relay_register",
                """{"group_id":"g1","members":["alice","bob","carol","dave"]}""",
                "bob"
            )
        )
        // CreateGroup still goes out (idempotent member sync → GroupCreated
        // ack keeps relay_synced fresh), but no more deltas from a non-admin.
        assertEquals(1, after.size)
        assertEquals("CreateGroup", after[0].getString("type"))
    }

    @Test
    fun notAdminHintSkipsMemberDeltasUpFront() {
        val translator = RelayControlOpTranslator("bob")

        // The core's is_admin=false hint: no deltas are ever attempted, so
        // the relay never answers the group-scoped denials that would revoke
        // relay_synced and surface as app-visible group_error on reconnect.
        val translation = translator.translate(
            "group_relay_register",
            """{"group_id":"g1","members":["alice","bob","carol"],"is_admin":false}""",
            "bob"
        )
        assertTrue(translation is RelayControlOpTranslator.Translation.Replace)
        val sent = frames(translation)
        assertEquals(1, sent.size)
        assertEquals("CreateGroup", sent[0].getString("type"))

        // is_admin=true keeps the normal delta behavior.
        val admin = frames(
            translator.translate(
                "group_relay_register",
                """{"group_id":"g2","members":["alice","bob"],"is_admin":true}""",
                "bob"
            )
        )
        assertEquals(2, admin.size)
        assertEquals("AddGroupMember", admin[1].getString("type"))
        assertEquals("alice", admin[1].getString("username"))
    }

    @Test
    fun numericAndStringIsAdminHintsMatchSwiftSemantics() {
        val translator = RelayControlOpTranslator("bob")

        // Numeric 0 is honored as not-admin (NSNumber.boolValue parity with
        // the Swift translator) even though serde emits real booleans.
        val numericZero = frames(
            translator.translate(
                "group_relay_register",
                """{"group_id":"g1","members":["alice","bob"],"is_admin":0}""",
                "bob"
            )
        )
        assertEquals(1, numericZero.size)
        assertEquals("CreateGroup", numericZero[0].getString("type"))

        // Numeric 1 behaves like true: deltas flow.
        val numericOne = frames(
            translator.translate(
                "group_relay_register",
                """{"group_id":"g2","members":["alice","bob"],"is_admin":1}""",
                "bob"
            )
        )
        assertEquals(2, numericOne.size)

        // A string encoding is not boolean-like on either platform: treated
        // as an absent hint (send-and-learn), not as a denial.
        val stringFalse = frames(
            translator.translate(
                "group_relay_register",
                """{"group_id":"g3","members":["alice","bob"],"is_admin":"false"}""",
                "bob"
            )
        )
        assertEquals(2, stringFalse.size)
    }

    @Test
    fun leaveAfterRejoinSendsLeaveGroupAgain() {
        val translator = RelayControlOpTranslator("alice")

        commit(
            translator.translate(
                "group_mls_leave",
                """{"group_id":"g1","leaving_member":"alice"}""",
                "bob"
            )
        )

        // Rejoining re-registers the group: that proves membership again and
        // must re-arm the leave dedup.
        commit(
            translator.translate(
                "group_relay_register",
                """{"group_id":"g1","members":["alice","bob"]}""",
                "alice"
            )
        )

        val secondLeave = translator.translate(
            "group_mls_leave",
            """{"group_id":"g1","leaving_member":"alice"}""",
            "bob"
        )
        assertTrue(secondLeave is RelayControlOpTranslator.Translation.Tap)
        val leave = frames(secondLeave).single()
        assertEquals("LeaveGroup", leave.getString("type"))
        assertEquals("g1", leave.getString("group_id"))
    }

    @Test
    fun adminDenialDuringFlightWinsOverCommit() {
        val translator = RelayControlOpTranslator("bob")

        // Frames produced, but the relay's denial lands before the caller
        // commits (reader thread races the send loop): the commit must not
        // record the membership snapshot the relay refused.
        val inFlight = translator.translate(
            "group_relay_register",
            """{"group_id":"g1","members":["alice","bob"]}""",
            "bob"
        )
        translator.onGroupError("g1", "Only admins can add members")
        commit(inFlight)

        val after = frames(
            translator.translate(
                "group_relay_register",
                """{"group_id":"g1","members":["alice","bob"]}""",
                "bob"
            )
        )
        // Denied: CreateGroup only, and no snapshot was committed.
        assertEquals(1, after.size)
        assertEquals("CreateGroup", after[0].getString("type"))
    }

    @Test
    fun broadcastTranslatesToSendGroupMessage() {
        val translator = RelayControlOpTranslator("alice")
        val bcast = frames(
            translator.translate(
                "group_relay_broadcast",
                """{"group_id":"g1","ciphertext":"AAECAw==","epoch":4,"reply_to":"m-9"}""",
                "alice"
            )
        ).single()
        assertEquals("SendGroupMessage", bcast.getString("type"))
        assertEquals("g1", bcast.getString("group_id"))
        assertEquals("AAECAw==", bcast.getString("content"))
        assertEquals("m-9", bcast.getString("reply_to_msg"))
    }

    @Test
    fun leaveIsATapSentOncePerGroupForSelfOnly() {
        val translator = RelayControlOpTranslator("alice")

        val tap = translator.translate(
            "group_mls_leave",
            """{"group_id":"g1","leaving_member":"alice"}""",
            "bob"
        )
        assertTrue(tap is RelayControlOpTranslator.Translation.Tap)
        val leave = frames(tap).single()
        assertEquals("LeaveGroup", leave.getString("type"))
        assertEquals("g1", leave.getString("group_id"))
        commit(tap)

        // Second per-member leave notification: no duplicate LeaveGroup.
        val second = translator.translate(
            "group_mls_leave",
            """{"group_id":"g1","leaving_member":"alice"}""",
            "carol"
        )
        assertTrue(second is RelayControlOpTranslator.Translation.Tap)
        assertTrue(frames(second).isEmpty())

        // Someone else leaving is not our LeaveGroup to send.
        val other = translator.translate(
            "group_mls_leave",
            """{"group_id":"g2","leaving_member":"bob"}""",
            "carol"
        )
        assertTrue(frames(other).isEmpty())
    }

    /**
     * Parity pin against the Rust op registry
     * (test_internet_control_op_registry_is_closed in
     * crates/offline-protocol/src/protocol/tests/mod.rs): every op the core
     * can emit must translate to a relay-native shape (Replace/Tap), never
     * PassThrough. A new Rust op must be handled here AND in the Swift
     * translator AND the spec table before it ships — an unhandled op
     * degrades to an opaque SendMessage the relay merely echoes/forwards.
     */
    @Test
    fun everyCoreControlOpTranslatesToRelayNative() {
        val translator = RelayControlOpTranslator("alice")

        // Ordered: g1 is registered before the ops that reference it.
        val cases = listOf(
            Triple(
                "group_relay_register",
                """{"group_id":"g1","group_name":"Trip","members":["alice","bob"]}""",
                "alice"
            ),
            Triple(
                "group_relay_broadcast",
                """{"group_id":"g1","ciphertext":"AAECAw==","epoch":1}""",
                "alice"
            ),
            Triple("group_mls_leave", """{"group_id":"g1","leaving_member":"alice"}""", "bob"),
        )

        for ((op, payload, recipient) in cases) {
            val translation = translator.translate(op, payload, recipient)
            commit(translation)
            assertFalse(
                "core op '$op' must translate to a relay-native shape, got PassThrough",
                translation is RelayControlOpTranslator.Translation.PassThrough
            )
        }
    }

    @Test
    fun malformedPayloadAndUnknownOpFallBackToPassThrough() {
        val translator = RelayControlOpTranslator("alice")
        assertTrue(
            translator.translate("group_relay_broadcast", "not-json", "alice")
                is RelayControlOpTranslator.Translation.PassThrough
        )
        assertTrue(
            translator.translate("some_future_op", "{}", "bob")
                is RelayControlOpTranslator.Translation.PassThrough
        )
        assertTrue(
            translator.translate("group_relay_register", """{"members":[]}""", "alice")
                is RelayControlOpTranslator.Translation.PassThrough
        )
    }

    @Test
    fun resetForgetsRegistrationDiffState() {
        val translator = RelayControlOpTranslator("alice")
        commit(
            translator.translate(
                "group_relay_register",
                """{"group_id":"g1","members":["alice","bob"]}""",
                "alice"
            )
        )
        translator.reset()

        // After reconnect the full membership registers again.
        val again = frames(
            translator.translate(
                "group_relay_register",
                """{"group_id":"g1","members":["alice","bob"]}""",
                "alice"
            )
        )
        assertEquals(2, again.size)
        assertEquals("AddGroupMember", again[1].getString("type"))
        assertEquals("bob", again[1].getString("username"))
    }

    @Test
    fun registerCommitAfterResetIsANoOp() {
        val translator = RelayControlOpTranslator("alice")

        // A chain that settles after a disconnect reset must not commit a
        // phantom snapshot into the NEXT connection's diff base — the relay
        // never received the buffered deltas, and a poisoned base would make
        // the reconnect's register skip them permanently.
        val staleTranslation = translator.translate(
            "group_relay_register",
            """{"group_id":"g1","members":["alice","bob"]}""",
            "alice"
        )
        translator.reset()
        commit(staleTranslation)

        val again = frames(
            translator.translate(
                "group_relay_register",
                """{"group_id":"g1","members":["alice","bob"]}""",
                "alice"
            )
        )
        assertEquals(2, again.size)
        assertEquals("AddGroupMember", again[1].getString("type"))
        assertEquals("bob", again[1].getString("username"))
    }

    @Test
    fun leaveCommitAfterResetIsANoOp() {
        val translator = RelayControlOpTranslator("alice")

        val staleTranslation = translator.translate(
            "group_mls_leave",
            """{"group_id":"g1","leaving_member":"alice"}""",
            "bob"
        )
        translator.reset()
        commit(staleTranslation)

        // The stale LeaveGroup never reached the relay's registry on the
        // new connection; the dedup must not swallow the retry.
        val retry = frames(
            translator.translate(
                "group_mls_leave",
                """{"group_id":"g1","leaving_member":"alice"}""",
                "bob"
            )
        )
        assertEquals(1, retry.size)
        assertEquals("LeaveGroup", retry[0].getString("type"))
    }

    @Test
    fun nonAdminReasonGroupErrorDoesNotSuppressDeltas() {
        val translator = RelayControlOpTranslator("alice")

        // Only the relay's admin-denial wording flips the suppression, even
        // when the error correlates to outstanding deltas; an unrelated
        // group error (bad member id, transient state) must not silently
        // stop membership sync.
        commit(
            translator.translate(
                "group_relay_register",
                """{"group_id":"g1","members":["alice","bob"]}""",
                "alice"
            )
        )
        translator.onGroupError("g1", "User not found")

        val registration = frames(
            translator.translate(
                "group_relay_register",
                """{"group_id":"g1","members":["alice","bob","carol"]}""",
                "alice"
            )
        )
        assertEquals(2, registration.size)
        assertEquals("AddGroupMember", registration[1].getString("type"))
        assertEquals("carol", registration[1].getString("username"))
    }

    @Test
    fun rolePromotionReenablesMemberDeltas() {
        val translator = RelayControlOpTranslator("bob")
        // The denial answers an outstanding member delta (the correlation
        // window — see onGroupError).
        translator.translate(
            "group_relay_register",
            """{"group_id":"g1","members":["alice","bob"]}""",
            "bob"
        )
        translator.onGroupError("g1", "Only admins can add members")

        // Denied: registration is CreateGroup only.
        val denied = frames(
            translator.translate(
                "group_relay_register",
                """{"group_id":"g1","members":["alice","bob"]}""",
                "bob"
            )
        )
        assertEquals(1, denied.size)

        // Someone else's promotion — or a demotion — changes nothing.
        translator.onRoleChanged("g1", "carol", "admin")
        translator.onRoleChanged("g1", "bob", "member")
        assertEquals(
            1,
            frames(
                translator.translate(
                    "group_relay_register",
                    """{"group_id":"g1","members":["alice","bob"]}""",
                    "bob"
                )
            ).size
        )

        // This device's promotion to admin re-enables the deltas.
        translator.onRoleChanged("g1", "bob", "admin")
        val promoted = frames(
            translator.translate(
                "group_relay_register",
                """{"group_id":"g1","members":["alice","bob"]}""",
                "bob"
            )
        )
        assertEquals(2, promoted.size)
        assertEquals("AddGroupMember", promoted[1].getString("type"))
        assertEquals("alice", promoted[1].getString("username"))
    }

    @Test
    fun uncorrelatedAdminDenialDoesNotSuppressDeltas() {
        val translator = RelayControlOpTranslator("bob")

        // An admin-denial GroupError with NO member deltas outstanding from
        // this translator answers someone else's operation (an app
        // raw-channel op, another admin's edit, an unrelated error quoting
        // a user-authored group name) — honoring it would permanently
        // silence membership sync for the group.
        translator.onGroupError("g1", "Only admins can add members")

        val registration = frames(
            translator.translate(
                "group_relay_register",
                """{"group_id":"g1","members":["alice","bob"]}""",
                "bob"
            )
        )
        assertEquals(2, registration.size)
        assertEquals("AddGroupMember", registration[1].getString("type"))
        assertEquals("alice", registration[1].getString("username"))
    }

    @Test
    fun requestIdCarryingDenialIsNeverOurs() {
        val translator = RelayControlOpTranslator("bob")

        // The translator never tags request_id, so a request_id-echoing
        // GroupError answers an app raw-channel frame — even mid-window it
        // must neither suppress deltas nor consume the window.
        translator.translate(
            "group_relay_register",
            """{"group_id":"g1","members":["alice","bob"]}""",
            "bob"
        )
        translator.onGroupError("g1", "Only admins can add members", "req-42")

        // Not suppressed: the uncommitted registration re-sends its delta.
        val retry = frames(
            translator.translate(
                "group_relay_register",
                """{"group_id":"g1","members":["alice","bob"]}""",
                "bob"
            )
        )
        assertEquals(2, retry.size)
        assertEquals("AddGroupMember", retry[1].getString("type"))

        // The window survived the disowned error: a real (request_id-less)
        // denial still lands.
        translator.onGroupError("g1", "Only admins can add members")
        assertEquals(
            1,
            frames(
                translator.translate(
                    "group_relay_register",
                    """{"group_id":"g1","members":["alice","bob"]}""",
                    "bob"
                )
            ).size
        )
    }

    @Test
    fun deltaWindowClosesOnFirstGroupScopedAnswer() {
        val translator = RelayControlOpTranslator("bob")

        // One answer per window: the first group-scoped GroupError closes
        // it, so a later admin-denial with nothing outstanding is
        // uncorrelated and falls back to send-and-learn (noisy but safe)
        // instead of suppressing on someone else's error.
        commit(
            translator.translate(
                "group_relay_register",
                """{"group_id":"g1","members":["alice","bob"]}""",
                "bob"
            )
        )
        translator.onGroupError("g1", "User not found")
        translator.onGroupError("g1", "Only admins can add members")

        val registration = frames(
            translator.translate(
                "group_relay_register",
                """{"group_id":"g1","members":["alice","bob","carol"]}""",
                "bob"
            )
        )
        assertEquals(2, registration.size)
        assertEquals("AddGroupMember", registration[1].getString("type"))
        assertEquals("carol", registration[1].getString("username"))
    }

    @Test
    fun successAnswerClosesTheDenialWindow() {
        val translator = RelayControlOpTranslator("bob")

        // The register succeeded (GroupCreated answered it): the success
        // answer must close the delta window too, or it stays armed for the
        // rest of the connection and a later unrelated error quoting the
        // denial phrase (e.g. a user-authored group name) would be honored
        // as OUR denial and suppress membership sync.
        commit(
            translator.translate(
                "group_relay_register",
                """{"group_id":"g1","members":["alice","bob"]}""",
                "bob"
            )
        )
        translator.onGroupAnswered("g1")
        translator.onGroupError("g1", "Only admins can add members")

        val registration = frames(
            translator.translate(
                "group_relay_register",
                """{"group_id":"g1","members":["alice","bob","carol"]}""",
                "bob"
            )
        )
        assertEquals(2, registration.size)
        assertEquals("AddGroupMember", registration[1].getString("type"))
        assertEquals("carol", registration[1].getString("username"))
    }

    @Test
    fun resetClosesTheDenialWindow() {
        val translator = RelayControlOpTranslator("bob")

        translator.translate(
            "group_relay_register",
            """{"group_id":"g1","members":["alice","bob"]}""",
            "bob"
        )
        translator.reset()

        // The outstanding delta died with the connection: a phrase-quoting
        // error on the next connection is not its answer.
        translator.onGroupError("g1", "Only admins can add members")

        val after = frames(
            translator.translate(
                "group_relay_register",
                """{"group_id":"g1","members":["alice","bob"]}""",
                "bob"
            )
        )
        assertEquals(2, after.size)
        assertEquals("AddGroupMember", after[1].getString("type"))
    }

    @Test
    fun serverPlaneFirewallBlocksRelayAnswerPrefixesOnly() {
        // Relay-answer frames a peer must never originate: the core trusts
        // these from the internet path (__GROUP_CREATED__ can mark a group
        // relay-synced), and the relay forwards peer content verbatim.
        for (forged in listOf(
            "__GROUP_CREATED__{\"group_id\":\"g1\",\"name\":\"x\"}",
            "__GROUP_MEMBER_ADDED__{\"group_id\":\"g1\",\"user_id\":\"mallory\"}",
            "__GROUP_MEMBER_REMOVED__{\"group_id\":\"g1\",\"user_id\":\"bob\"}",
            "__GROUP_INFO__{\"group_id\":\"g1\"}",
            "__USER_GROUPS__{\"groups\":[]}",
            "__GROUP_ERROR__{\"reason\":\"x\",\"group_id\":\"g1\"}"
        )) {
            assertTrue(
                "expected forged frame to be blocked: $forged",
                RelayControlOpTranslator.isForgedServerPlaneAnswer(forged)
            )
        }

        // Legitimate peer traffic must keep flowing — group fan-out,
        // typing, MLS control, plain text, and prefix-shaped user content
        // that is not an exact server-plane prefix.
        for (legit in listOf(
            "__GROUP_MSG__{\"group_id\":\"g1\",\"content\":\"c\"}",
            "__TYPING__{\"conversation_id\":\"c1\",\"is_typing\":true}",
            "__GRP_MLS_WELCOME__abc",
            "__CONN_REQ__{\"sender_name\":\"bob\"}",
            "hello __GROUP_CREATED__ mid-string",
            "__GROUP_CREATED_X__ not the prefix",
            ""
        )) {
            assertFalse(
                "expected legitimate frame to pass: $legit",
                RelayControlOpTranslator.isForgedServerPlaneAnswer(legit)
            )
        }
    }
}
