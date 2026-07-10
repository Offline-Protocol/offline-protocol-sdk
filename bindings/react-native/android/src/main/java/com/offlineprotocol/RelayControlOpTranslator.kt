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
 *   `RemoveGroupMember` deltas against the last *committed* membership.
 *   Member deltas are admin-gated server-side ("Only admins can add
 *   members"): when the payload carries the core's `is_admin=false` hint the
 *   deltas are skipped up front (the admin's own device is the authoritative
 *   registrar), and without the hint a group's first admin-denied GroupError
 *   stops further deltas for it this connection — otherwise every reconnect
 *   would re-send N denied deltas whose group-scoped errors revoke the
 *   core's relay sync and surface as app-visible group_error events.
 * - `group_relay_broadcast` — [Translation.Replace] with `SendGroupMessage`
 *   carrying the MLS ciphertext; the relay fans out `GroupMessageReceived`
 *   per member (bridged back as `__GROUP_MSG__`, MLS-decrypted in core).
 * - `group_mls_leave` — [Translation.Tap]: the per-member leave notification
 *   must still be delivered verbatim, plus one relay-native `LeaveGroup`
 *   (deduped per group) so the relay registry doesn't go stale. The relay
 *   permits self-removal.
 *
 * State commits are deferred: a translation's [Translation.Replace.commit] /
 * [Translation.Tap.commit] must be invoked by the caller ONLY after every
 * frame was written to the socket. A dropped best-effort delta with an
 * optimistically committed snapshot would silently leave that member out of
 * relay fan-out for the rest of the connection; deferring the commit makes
 * the next register/leave re-send the missing frames instead.
 *
 * Known v1 limitation: a group rename re-registers, but the relay's
 * idempotent sync never updates the stored name (`ON CONFLICT DO NOTHING`).
 *
 * State is per-connection: call [reset] on disconnect.
 * Thread-safe: relay answers (`onGroupError`) arrive on OkHttp's reader
 * thread while `translate` runs on the main handler's poll loop, so all
 * state is guarded by an internal lock.
 */
class RelayControlOpTranslator(private val selfId: String) {

    companion object {
        /**
         * Cross-layer contract: substring of the relay server's admin-denial
         * GroupError reasons ("Only admins can add members" / "Only admins
         * can remove members"). If the relay rewords these, non-admin devices
         * without the core's `is_admin` hint fall back to re-learning the
         * denial each connection — noisy but safe. Keep in sync with the
         * relay source (see docs/relay-transport-parity-spec.md).
         */
        internal const val ADMIN_DENIED_REASON_MARKER = "Only admins"
    }

    sealed class Translation {
        /**
         * Send these relay-native frames instead of the original message.
         * Invoke [commit] only after ALL frames were written to the socket.
         */
        data class Replace(
            val frames: List<JSONObject>,
            val commit: (() -> Unit)? = null
        ) : Translation()

        /**
         * Send the original message verbatim, then these frames best-effort.
         * Invoke [commit] only after ALL extra frames were written.
         */
        data class Tap(
            val frames: List<JSONObject>,
            val commit: (() -> Unit)? = null
        ) : Translation()

        /** No translation — send the original message verbatim. */
        object PassThrough : Translation()
    }

    private val lock = Any()

    /** Last membership committed as registered with the relay, per group. */
    private val registeredMembers = HashMap<String, Set<String>>()

    /** Groups whose member deltas the relay denied (we are not the admin). */
    private val memberDeltasDenied = HashSet<String>()

    /** Groups for which a relay-native LeaveGroup was already committed. */
    private val leaveSent = HashSet<String>()

    fun translate(
        controlOp: String,
        controlPayload: String,
        recipientId: String
    ): Translation = synchronized(lock) {
        try {
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
                    if (groupId.isEmpty() || leavingMember != selfId || leaveSent.contains(groupId)) {
                        Translation.Tap(emptyList())
                    } else {
                        Translation.Tap(
                            listOf(JSONObject().apply {
                                put("type", "LeaveGroup")
                                put("group_id", groupId)
                            }),
                            commit = { commitLeave(groupId) }
                        )
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

        // A register proves membership again: a rejoin after a committed
        // leave must be allowed to send LeaveGroup again later.
        leaveSent.remove(groupId)

        // The core's admin hint: explicitly-not-admin devices never send
        // member deltas (the relay would deny each with a group-scoped
        // GroupError). Absent hint = unknown, fall back to send-and-learn.
        val notAdmin = payload.has("is_admin") && !payload.optBoolean("is_admin", true)

        val frames = ArrayList<JSONObject>()
        frames.add(JSONObject().apply {
            put("type", "CreateGroup")
            put("group_id", groupId)
            put("name", name)
        })

        // Member deltas: the relay adds the creator itself, and self-adds are
        // redundant, so the self id never appears in a delta. Sorted for a
        // deterministic wire order across platforms.
        var commit: (() -> Unit)? = null
        if (!notAdmin && !memberDeltasDenied.contains(groupId)) {
            val desired = members.filter { it != selfId }.toSet()
            val known = registeredMembers[groupId] ?: emptySet()
            for (added in (desired - known).sorted()) {
                frames.add(JSONObject().apply {
                    put("type", "AddGroupMember")
                    put("group_id", groupId)
                    put("username", added)
                })
            }
            for (removed in (known - desired).sorted()) {
                frames.add(JSONObject().apply {
                    put("type", "RemoveGroupMember")
                    put("group_id", groupId)
                    put("username", removed)
                })
            }
            commit = { commitRegisteredMembers(groupId, desired) }
        }
        return Translation.Replace(frames, commit)
    }

    private fun commitRegisteredMembers(groupId: String, members: Set<String>) {
        synchronized(lock) {
            // A GroupError may have marked the group admin-denied while the
            // frames were in flight; the denial wins.
            if (!memberDeltasDenied.contains(groupId)) {
                registeredMembers[groupId] = members
            }
        }
    }

    private fun commitLeave(groupId: String) {
        synchronized(lock) {
            leaveSent.add(groupId)
            registeredMembers.remove(groupId)
            memberDeltasDenied.remove(groupId)
        }
    }

    /** Feed relay GroupError answers so admin-denied groups stop producing member deltas. */
    fun onGroupError(groupId: String, reason: String) {
        if (groupId.isEmpty()) return
        if (reason.contains(ADMIN_DENIED_REASON_MARKER, ignoreCase = true)) {
            synchronized(lock) {
                memberDeltasDenied.add(groupId)
                // The membership snapshot was not applied server-side.
                registeredMembers.remove(groupId)
            }
        }
    }

    /** Per-connection state: call on disconnect. */
    fun reset() {
        synchronized(lock) {
            registeredMembers.clear()
            memberDeltasDenied.clear()
            leaveSent.clear()
        }
    }
}
