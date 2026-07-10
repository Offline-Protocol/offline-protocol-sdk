package com.offlineprotocol

import org.json.JSONObject
import org.junit.Assert.assertEquals
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

    @Test
    fun connectionOpsTranslateToRelayNativeFrames() {
        val translator = RelayControlOpTranslator("alice")

        val req = translator.translate(
            "conn_req",
            """{"sender_name":"Alice","timestamp_ms":1,"key_package":[1,2,3]}""",
            "bob"
        )
        assertTrue(req is RelayControlOpTranslator.Translation.Replace)
        val reqFrame = frames(req).single()
        assertEquals("SendConnectionRequest", reqFrame.getString("type"))
        assertEquals("bob", reqFrame.getString("recipient"))
        assertEquals("Alice", reqFrame.getString("sender_name"))
        assertEquals(3, reqFrame.getJSONArray("key_package").length())

        val acc = translator.translate(
            "conn_acc",
            """{"accepted_by_name":"Alice","timestamp_ms":1}""",
            "bob"
        )
        val accFrame = frames(acc).single()
        assertEquals("AcceptConnectionRequest", accFrame.getString("type"))
        assertEquals("bob", accFrame.getString("requester_id"))
        assertEquals("Alice", accFrame.getString("accepter_name"))
        assertTrue(!accFrame.has("key_package"))

        val rej = frames(translator.translate("conn_rej", "", "bob")).single()
        assertEquals("RejectConnectionRequest", rej.getString("type"))
        assertEquals("bob", rej.getString("requester_id"))

        val can = frames(translator.translate("conn_can", "", "bob")).single()
        assertEquals("CancelConnectionRequest", can.getString("type"))
        assertEquals("bob", can.getString("recipient"))
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

    @Test
    fun malformedPayloadAndUnknownOpFallBackToPassThrough() {
        val translator = RelayControlOpTranslator("alice")
        assertTrue(
            translator.translate("conn_req", "not-json", "bob")
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
}
