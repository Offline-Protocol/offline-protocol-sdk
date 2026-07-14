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
 *   — recognized only while that group has member deltas awaiting an answer
 *   (see [onGroupError]) — stops further deltas for it this connection;
 *   otherwise every reconnect would re-send N denied deltas whose
 *   group-scoped errors revoke the core's relay sync and surface as
 *   app-visible group_error events.
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
 * the next register/leave re-send the missing frames instead. Commits are
 * additionally generation-guarded: a commit whose translation predates a
 * [reset] is a no-op, so a chain that settles after a disconnect (or
 * RateLimited) cannot write a phantom snapshot into the next connection's
 * diff base.
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
         * can remove members"). The marker alone is NOT enough to suppress
         * deltas — it only counts when the GroupError correlates to member
         * deltas this translator actually has outstanding (see
         * [onGroupError]), so an admin-denial answering some other actor's
         * operation can never permanently silence membership sync. If the
         * relay rewords these, non-admin devices without the core's
         * `is_admin` hint fall back to re-learning the denial each
         * connection — noisy but safe. These strings are an external wire
         * contract — keep them in sync with the denial reasons received over
         * the internet transport.
         */
        internal const val ADMIN_DENIED_REASON_MARKER = "Only admins"

        /**
         * SDK content prefixes only the relay server (bridged by this
         * layer's `injectGroupInternalMessage`) may originate — never a
         * peer. The relay forwards peer message content verbatim, so
         * without this firewall any peer could deliver a crafted
         * `__GROUP_CREATED__` over the authenticated internet path and (in
         * concert with a spoofed registration window) mark a group
         * relay-synced against a relay that never registered it —
         * black-holing broadcasts on a store-less relay. `__GROUP_MSG__` is
         * deliberately absent: it is legitimate peer/relay group traffic.
         */
        private val SERVER_PLANE_ANSWER_PREFIXES = arrayOf(
            "__GROUP_CREATED__",
            "__GROUP_MEMBER_ADDED__",
            "__GROUP_MEMBER_REMOVED__",
            "__GROUP_INFO__",
            "__USER_GROUPS__",
            "__GROUP_ERROR__"
        )

        /**
         * True when peer-delivered message content must be dropped because
         * it impersonates a relay server answer. Called by the
         * `MessageReceived` ingest path with the inner SDK content.
         */
        fun isForgedServerPlaneAnswer(content: String): Boolean =
            SERVER_PLANE_ANSWER_PREFIXES.any { content.startsWith(it) }
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

    /**
     * Bumped by [reset]. Commit closures capture the generation of their
     * translation and no-op if it moved: state written after a reset would
     * describe frames sent on a connection (or inside a rate budget) the
     * relay already discarded.
     */
    private var generation = 0L

    /** Last membership committed as registered with the relay, per group. */
    private val registeredMembers = HashMap<String, Set<String>>()

    /** Groups whose member deltas the relay denied (we are not the admin). */
    private val memberDeltasDenied = HashSet<String>()

    /**
     * Groups with member deltas outstanding: the window between a
     * translation that produced AddGroupMember/RemoveGroupMember frames and
     * the next group-scoped answer. Only a GroupError landing inside this
     * window may be read as OUR admin denial — the translator never tags
     * request_id, so a group-scoped error with no outstanding deltas belongs
     * to some other actor (an app raw-channel op, another admin's edit).
     * Mirrors ios/RelayControlOpTranslator.swift — keep the two in sync.
     */
    private val outstandingMemberDeltas = HashSet<String>()

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
                        val gen = generation
                        Translation.Tap(
                            listOf(JSONObject().apply {
                                put("type", "LeaveGroup")
                                put("group_id", groupId)
                            }),
                            commit = { commitLeave(gen, groupId) }
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
        // serde emits a real boolean, but numeric 0/1 encodings are honored
        // too, and anything else (strings included) is treated as absent —
        // exactly NSNumber.boolValue semantics, so both bridges parse the
        // frame identically. Keep in sync with the Swift translator.
        val notAdmin = when (val isAdmin = payload.opt("is_admin")) {
            is Boolean -> !isAdmin
            is Number -> isAdmin.toDouble() == 0.0
            else -> false
        }

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
            // Deltas produced: open the group's outstanding-delta window so
            // the next group-scoped GroupError may be read as our denial.
            if (frames.size > 1) {
                outstandingMemberDeltas.add(groupId)
            }
            val gen = generation
            commit = { commitRegisteredMembers(gen, groupId, desired) }
        }
        return Translation.Replace(frames, commit)
    }

    private fun commitRegisteredMembers(generationAtTranslate: Long, groupId: String, members: Set<String>) {
        synchronized(lock) {
            if (generationAtTranslate != generation) return
            // A GroupError may have marked the group admin-denied while the
            // frames were in flight; the denial wins.
            if (!memberDeltasDenied.contains(groupId)) {
                registeredMembers[groupId] = members
            }
        }
    }

    private fun commitLeave(generationAtTranslate: Long, groupId: String) {
        synchronized(lock) {
            if (generationAtTranslate != generation) return
            leaveSent.add(groupId)
            registeredMembers.remove(groupId)
            memberDeltasDenied.remove(groupId)
            // A left group's answers are no longer ours to correlate; a
            // stale window must not let a post-leave GroupError mark a
            // future rejoin as admin-denied.
            outstandingMemberDeltas.remove(groupId)
        }
    }

    /**
     * Feed relay GroupError answers so admin-denied groups stop producing
     * member deltas. The denial is honored ONLY when it correlates to
     * member deltas this translator has outstanding: a `request_id`-carrying
     * error answers an app raw-channel frame (this translator never tags
     * request_id, so it cannot be ours), and without an open per-group
     * delta window the error belongs to some other actor's operation —
     * treating it as ours would permanently suppress membership sync for
     * the group (e.g. an unrelated error quoting a user-authored group
     * name that contains the phrase).
     */
    fun onGroupError(groupId: String, reason: String, requestId: String? = null) {
        if (groupId.isEmpty()) return
        if (!requestId.isNullOrEmpty()) return
        synchronized(lock) {
            if (!outstandingMemberDeltas.contains(groupId)) return
            // The next group-scoped answer closes the window, denial or not.
            outstandingMemberDeltas.remove(groupId)
            if (reason.contains(ADMIN_DENIED_REASON_MARKER, ignoreCase = true)) {
                memberDeltasDenied.add(groupId)
                // The membership snapshot was not applied server-side.
                registeredMembers.remove(groupId)
            }
        }
    }

    /**
     * Feed relay *success* answers on the group channel (GroupCreated,
     * GroupMemberAdded, GroupMemberRemoved): any group-scoped answer closes
     * the admin-denial correlation window, success included — [onGroupError]
     * already closes it for errors. Without this, a successful
     * register-with-deltas leaves the window open for the whole connection,
     * and a later request_id-less GroupError merely *quoting* the denial
     * phrase (e.g. a user-authored group name) would be honored as OUR
     * denial and suppress membership sync until reconnect.
     */
    fun onGroupAnswered(groupId: String) {
        if (groupId.isEmpty()) return
        synchronized(lock) {
            outstandingMemberDeltas.remove(groupId)
        }
    }

    /**
     * Feed relay `GroupRoleChanged` frames: a promotion of this device to
     * admin re-enables member deltas an earlier denial suppressed —
     * otherwise a mid-connection promotion keeps membership edits away from
     * the relay until the next reconnect. (The denial already dropped the
     * group's committed snapshot, so the next register recomputes the full
     * delta set.)
     */
    fun onRoleChanged(groupId: String, userId: String, newRole: String) {
        if (groupId.isEmpty() || userId != selfId) return
        if (!newRole.equals("admin", ignoreCase = true)) return
        synchronized(lock) {
            memberDeltasDenied.remove(groupId)
        }
    }

    /** Per-connection state: call on disconnect. */
    fun reset() {
        synchronized(lock) {
            generation++
            registeredMembers.clear()
            memberDeltasDenied.clear()
            leaveSent.clear()
            outstandingMemberDeltas.clear()
        }
    }
}
