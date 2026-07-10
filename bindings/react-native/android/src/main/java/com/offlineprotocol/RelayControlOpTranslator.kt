package com.offlineprotocol

import org.json.JSONArray
import org.json.JSONObject

/**
 * Translates the SDK's server-plane control frames (tagged by the core via
 * `InternetMessage.control_op`) into the relay's native JSON protocol.
 *
 * The relay does not intercept content prefixes — a self-addressed
 * `__GRP_RELAY_REG__`/`__GRP_RELAY_BCAST__` frame sent as an opaque
 * `SendMessage` is just echoed back — so this translator is the "relay
 * adapter" the core's relay-optimized group path was designed against.
 *
 * Semantics per op:
 * - `conn_req`/`conn_acc`/`conn_rej`/`conn_can` — [Translation.Replace] with
 *   the relay-native connection-request op.
 * - `group_relay_register` — [Translation.Replace] with `CreateGroup` (the
 *   relay treats a member's re-sync as idempotent and answers `GroupCreated`,
 *   which is the core's sync acknowledgment) plus `AddGroupMember`/
 *   `RemoveGroupMember` deltas against the last registered membership.
 *   Member deltas are admin-gated server-side ("Only admins can add
 *   members"), so after a group's first admin-denied GroupError no further
 *   deltas are sent for it this connection — the admin's own device is the
 *   authoritative registrar.
 * - `group_relay_broadcast` — [Translation.Replace] with `SendGroupMessage`
 *   carrying the MLS ciphertext; the relay fans out `GroupMessageReceived`
 *   per member (bridged back as `__GROUP_MSG__`, MLS-decrypted in core).
 * - `group_mls_leave` — [Translation.Tap]: the per-member leave notification
 *   must still be delivered verbatim, plus one relay-native `LeaveGroup`
 *   (deduped per group) so the relay registry doesn't go stale. The relay
 *   permits self-removal.
 *
 * Known v1 limitation: a group rename re-registers, but the relay's
 * idempotent sync never updates the stored name (`ON CONFLICT DO NOTHING`).
 *
 * State is per-connection: call [reset] on disconnect.
 * Not thread-safe; InternetManager confines all calls to one thread.
 */
class RelayControlOpTranslator(private val selfId: String) {

    sealed class Translation {
        /** Send these relay-native frames instead of the original message. */
        data class Replace(val frames: List<JSONObject>) : Translation()

        /** Send the original message verbatim, then these frames best-effort. */
        data class Tap(val frames: List<JSONObject>) : Translation()

        /** No translation — send the original message verbatim. */
        object PassThrough : Translation()
    }

    /** Last membership registered with the relay, per group. */
    private val registeredMembers = HashMap<String, Set<String>>()

    /** Groups whose member deltas the relay denied (we are not the admin). */
    private val memberDeltasDenied = HashSet<String>()

    /** Groups for which a relay-native LeaveGroup was already sent. */
    private val leaveSent = HashSet<String>()

    fun translate(
        controlOp: String,
        controlPayload: String,
        recipientId: String
    ): Translation {
        return try {
            when (controlOp) {
                "conn_req" -> {
                    val payload = JSONObject(controlPayload)
                    Translation.Replace(listOf(JSONObject().apply {
                        put("type", "SendConnectionRequest")
                        put("recipient", recipientId)
                        put("sender_name", payload.optString("sender_name", selfId))
                        payload.optJSONArray("key_package")?.let { put("key_package", it) }
                    }))
                }

                "conn_acc" -> {
                    val payload = JSONObject(controlPayload)
                    Translation.Replace(listOf(JSONObject().apply {
                        put("type", "AcceptConnectionRequest")
                        put("requester_id", recipientId)
                        put("accepter_name", payload.optString("accepted_by_name", selfId))
                        payload.optJSONArray("key_package")?.let { put("key_package", it) }
                    }))
                }

                "conn_rej" -> Translation.Replace(listOf(JSONObject().apply {
                    put("type", "RejectConnectionRequest")
                    put("requester_id", recipientId)
                }))

                "conn_can" -> Translation.Replace(listOf(JSONObject().apply {
                    put("type", "CancelConnectionRequest")
                    put("recipient", recipientId)
                }))

                "group_relay_register" -> translateRegister(controlPayload)

                "group_relay_broadcast" -> {
                    val payload = JSONObject(controlPayload)
                    val groupId = payload.optString("group_id")
                    if (groupId.isEmpty()) return Translation.PassThrough
                    Translation.Replace(listOf(JSONObject().apply {
                        put("type", "SendGroupMessage")
                        put("group_id", groupId)
                        put("content", payload.optString("ciphertext"))
                        payload.optString("reply_to").takeIf { it.isNotEmpty() }
                            ?.let { put("reply_to_msg", it) }
                    }))
                }

                "group_mls_leave" -> {
                    val payload = JSONObject(controlPayload)
                    val groupId = payload.optString("group_id")
                    val leavingMember = payload.optString("leaving_member")
                    if (groupId.isEmpty() || leavingMember != selfId || !leaveSent.add(groupId)) {
                        Translation.Tap(emptyList())
                    } else {
                        forgetGroup(groupId)
                        Translation.Tap(listOf(JSONObject().apply {
                            put("type", "LeaveGroup")
                            put("group_id", groupId)
                        }))
                    }
                }

                else -> Translation.PassThrough
            }
        } catch (e: Exception) {
            // Malformed control payload: fall back to the verbatim send, the
            // relay-agnostic behavior that existed before translation.
            Translation.PassThrough
        }
    }

    private fun translateRegister(controlPayload: String): Translation {
        val payload = JSONObject(controlPayload)
        val groupId = payload.optString("group_id")
        if (groupId.isEmpty()) return Translation.PassThrough
        val name = payload.optString("group_name").ifEmpty { groupId }
        val members = payload.optJSONArray("members")?.let { array ->
            (0 until array.length()).mapNotNull { array.optString(it).takeIf { m -> m.isNotEmpty() } }
        } ?: emptyList()

        val frames = ArrayList<JSONObject>()
        frames.add(JSONObject().apply {
            put("type", "CreateGroup")
            put("group_id", groupId)
            put("name", name)
        })

        // Member deltas: the relay adds the creator itself, and self-adds are
        // redundant, so the self id never appears in a delta.
        if (!memberDeltasDenied.contains(groupId)) {
            val desired = members.filter { it != selfId }.toSet()
            val known = registeredMembers[groupId] ?: emptySet()
            for (added in desired - known) {
                frames.add(JSONObject().apply {
                    put("type", "AddGroupMember")
                    put("group_id", groupId)
                    put("username", added)
                })
            }
            for (removed in known - desired) {
                frames.add(JSONObject().apply {
                    put("type", "RemoveGroupMember")
                    put("group_id", groupId)
                    put("username", removed)
                })
            }
            registeredMembers[groupId] = desired
        }
        return Translation.Replace(frames)
    }

    /** Feed relay GroupError answers so admin-denied groups stop producing member deltas. */
    fun onGroupError(groupId: String, reason: String) {
        if (groupId.isEmpty()) return
        if (reason.contains("Only admins", ignoreCase = true)) {
            memberDeltasDenied.add(groupId)
            // The optimistic membership snapshot was not applied server-side.
            registeredMembers.remove(groupId)
        }
    }

    /** Drop per-group state (member left / removed / group deleted). */
    fun forgetGroup(groupId: String) {
        registeredMembers.remove(groupId)
        memberDeltasDenied.remove(groupId)
    }

    /** Per-connection state: call on disconnect. */
    fun reset() {
        registeredMembers.clear()
        memberDeltasDenied.clear()
        leaveSent.clear()
    }
}
