package com.offlineprotocol

/** Kotlin 2.x treats Java `optString` as nullable — this avoids `?: ""` at every call site. */
internal fun org.json.JSONObject.safeOptString(key: String, fallback: String = ""): String =
    optString(key, fallback) ?: fallback
