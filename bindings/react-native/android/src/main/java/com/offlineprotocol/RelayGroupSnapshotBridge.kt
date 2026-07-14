package com.offlineprotocol

import org.json.JSONArray
import org.json.JSONObject

/**
 * Projects relay group snapshots into the SDK's stable typed payloads while
 * also forwarding the original frame for application-owned extensions.
 *
 * The raw callback must receive [rawText] unchanged: re-serializing [json]
 * can reorder keys and canonicalize numbers. Mirrors
 * ios/RelayGroupSnapshotBridge.swift -- keep in sync.
 */
internal object RelayGroupSnapshotBridge {
    fun dispatch(
        messageType: String,
        json: JSONObject,
        rawText: String,
        emitTyped: (prefix: String, payload: JSONObject) -> Unit,
        emitRaw: (String) -> Unit
    ): Boolean {
        return when (messageType) {
            "GroupInfo" -> {
                val groupId = json.safeOptString("group_id")
                if (groupId.isEmpty()) return true

                val membersJson = JSONArray()
                val membersArray = json.optJSONArray("members")
                if (membersArray != null) {
                    for (i in 0 until membersArray.length()) {
                        // Preserve the bridge's existing per-entry tolerance:
                        // malformed entries never discard the valid members.
                        val member = membersArray.optJSONObject(i) ?: continue
                        val memberId = member.safeOptString("user_id")
                        if (memberId.isEmpty()) continue
                        membersJson.put(JSONObject().apply {
                            put("user_id", memberId)
                            put("role", member.safeOptString("role", "member"))
                            put("joined_at", member.safeOptString("joined_at"))
                        })
                    }
                }

                val payload = JSONObject().apply {
                    put("group_id", groupId)
                    put("name", json.safeOptString("name"))
                    put("created_by", json.safeOptString("created_by"))
                    put("created_at", json.safeOptString("created_at"))
                    put("members", membersJson)
                }
                emitTyped("__GROUP_INFO__", payload)
                emitRaw(rawText)
                true
            }

            "UserGroups" -> {
                val groupsArray = json.optJSONArray("groups") ?: return true
                val groupsJson = JSONArray()
                for (i in 0 until groupsArray.length()) {
                    // Preserve the bridge's existing per-entry tolerance.
                    val group = groupsArray.optJSONObject(i) ?: continue
                    val groupId = group.safeOptString("group_id")
                    if (groupId.isEmpty()) continue
                    groupsJson.put(JSONObject().apply {
                        put("group_id", groupId)
                        put("name", group.safeOptString("name"))
                        put("created_at", group.safeOptString("created_at"))
                    })
                }

                emitTyped(
                    "__USER_GROUPS__",
                    JSONObject().apply { put("groups", groupsJson) }
                )
                emitRaw(rawText)
                true
            }

            else -> false
        }
    }
}
