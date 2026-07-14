package com.offlineprotocol

import org.json.JSONObject
import org.junit.Assert.assertArrayEquals
import org.junit.Assert.assertEquals
import org.junit.Assert.assertFalse
import org.junit.Assert.assertTrue
import org.junit.Test

/**
 * Pins lossless GroupInfo/UserGroups dual emission. Mirrors iOS
 * RelayGroupSnapshotBridgeTests -- keep in sync.
 */
class RelayGroupSnapshotBridgeTest {
    private data class TypedEmission(val prefix: String, val payload: JSONObject)

    @Test
    fun groupInfoEmitsTypedProjectionAndVerbatimRawFrame() {
        val raw = """
            {
              "future_top_level": {"ratio": 25.0},
              "pending_join_requests": [
                {"token":"invite-token","joiner_username":"dora","key_package":[1,2,3],"timestamp":"2026-07-14T10:00:00Z","future_request_field":{"v":1}}
              ],
              "avatar_url": "https://cdn.example/group.png",
              "members": [
                {"user_id":"alice","role":"admin","joined_at":"2026-07-01T00:00:00Z","profile":{"display_name":"Alice"}},
                "malformed-entry",
                {"user_id":"bob","role":"member","joined_at":"2026-07-02T00:00:00Z","future_member_field":true},
                {"user_id":"charlie","joined_at":"2026-07-03T00:00:00Z"},
                {"role":"member"}
              ],
              "description": "Planning group",
              "created_at": "2026-07-01T00:00:00Z",
              "created_by": "alice",
              "name": "Trip",
              "group_id": "g1",
              "type": "GroupInfo"
            }
        """.trimIndent()
        val typed = mutableListOf<TypedEmission>()
        val rawFrames = mutableListOf<String>()

        val handled = RelayGroupSnapshotBridge.dispatch(
            messageType = "GroupInfo",
            json = JSONObject(raw),
            rawText = raw,
            emitTyped = { prefix, payload -> typed += TypedEmission(prefix, payload) },
            emitRaw = { rawFrames += it }
        )

        assertTrue(handled)
        assertEquals(1, typed.size)
        assertEquals("__GROUP_INFO__", typed.single().prefix)
        val typedPayload = typed.single().payload
        assertEquals("g1", typedPayload.getString("group_id"))
        assertEquals("Trip", typedPayload.getString("name"))
        val members = typedPayload.getJSONArray("members")
        assertEquals(3, members.length())
        assertEquals("alice", members.getJSONObject(0).getString("user_id"))
        assertEquals("admin", members.getJSONObject(0).getString("role"))
        assertEquals("bob", members.getJSONObject(1).getString("user_id"))
        assertEquals("member", members.getJSONObject(1).getString("role"))
        assertEquals("charlie", members.getJSONObject(2).getString("user_id"))
        assertEquals("member", members.getJSONObject(2).getString("role"))

        assertEquals(1, rawFrames.size)
        assertArrayEquals(raw.toByteArray(Charsets.UTF_8), rawFrames.single().toByteArray(Charsets.UTF_8))
        val forwarded = JSONObject(rawFrames.single())
        assertEquals("Planning group", forwarded.getString("description"))
        assertEquals("https://cdn.example/group.png", forwarded.getString("avatar_url"))
        assertTrue(forwarded.has("future_top_level"))
        val pending = forwarded.getJSONArray("pending_join_requests").getJSONObject(0)
        assertEquals("invite-token", pending.getString("token"))
        assertEquals("dora", pending.getString("joiner_username"))
        assertEquals(3, pending.getJSONArray("key_package").length())
        assertEquals("2026-07-14T10:00:00Z", pending.getString("timestamp"))
        assertTrue(pending.has("future_request_field"))
        assertTrue(forwarded.getJSONArray("members").getJSONObject(0).has("profile"))
    }

    @Test
    fun userGroupsEmitsTypedProjectionAndPreservesProfileMembershipAndExtensions() {
        val raw = """
            {
              "type": "UserGroups",
              "profile": {"username":"alice","avatar_url":"https://cdn.example/alice.png","future_profile_field":25.0},
              "groups": [
                {"group_id":"g1","name":"Trip","created_at":"2026-07-01T00:00:00Z","membership":{"role":"admin","joined_at":"2026-07-01T00:00:00Z"},"future_group_field":{"enabled":true}},
                {"group_id":"g2","name":"Work","created_at":"2026-07-02T00:00:00Z","membership":{"role":"member","joined_at":"2026-07-03T00:00:00Z"}}
              ],
              "future_top_level": ["kept"]
            }
        """.trimIndent()
        val typed = mutableListOf<TypedEmission>()
        val rawFrames = mutableListOf<String>()

        val handled = RelayGroupSnapshotBridge.dispatch(
            messageType = "UserGroups",
            json = JSONObject(raw),
            rawText = raw,
            emitTyped = { prefix, payload -> typed += TypedEmission(prefix, payload) },
            emitRaw = { rawFrames += it }
        )

        assertTrue(handled)
        assertEquals(1, typed.size)
        assertEquals("__USER_GROUPS__", typed.single().prefix)
        val groups = typed.single().payload.getJSONArray("groups")
        assertEquals(2, groups.length())
        assertEquals("g1", groups.getJSONObject(0).getString("group_id"))
        assertEquals("Trip", groups.getJSONObject(0).getString("name"))
        assertEquals("g2", groups.getJSONObject(1).getString("group_id"))

        assertEquals(1, rawFrames.size)
        assertArrayEquals(raw.toByteArray(Charsets.UTF_8), rawFrames.single().toByteArray(Charsets.UTF_8))
        val forwarded = JSONObject(rawFrames.single())
        assertTrue(forwarded.has("profile"))
        assertTrue(forwarded.has("future_top_level"))
        val firstGroup = forwarded.getJSONArray("groups").getJSONObject(0)
        assertEquals("admin", firstGroup.getJSONObject("membership").getString("role"))
        assertTrue(firstGroup.has("future_group_field"))
    }

    @Test
    fun malformedRecognizedSnapshotsKeepExistingNoEventBehavior() {
        var typedCount = 0
        var rawCount = 0
        val emitTyped: (String, JSONObject) -> Unit = { _, _ -> typedCount++ }
        val emitRaw: (String) -> Unit = { rawCount++ }

        assertTrue(RelayGroupSnapshotBridge.dispatch(
            "GroupInfo",
            JSONObject("""{"type":"GroupInfo","group_id":"","description":"kept nowhere"}"""),
            "group-info-raw",
            emitTyped,
            emitRaw
        ))
        assertTrue(RelayGroupSnapshotBridge.dispatch(
            "UserGroups",
            JSONObject("""{"type":"UserGroups","groups":{"not":"an array"}}"""),
            "user-groups-raw",
            emitTyped,
            emitRaw
        ))

        assertEquals(0, typedCount)
        assertEquals(0, rawCount)
    }

    @Test
    fun unrelatedFramesAreNotClaimed() {
        val handled = RelayGroupSnapshotBridge.dispatch(
            "GroupError",
            JSONObject("""{"type":"GroupError","reason":"nope"}"""),
            "raw",
            { _, _ -> throw AssertionError("unexpected typed emission") },
            { throw AssertionError("unexpected raw emission") }
        )

        assertFalse(handled)
    }
}
