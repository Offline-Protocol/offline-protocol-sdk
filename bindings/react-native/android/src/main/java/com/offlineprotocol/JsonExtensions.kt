package com.offlineprotocol

import org.json.JSONObject

/** Kotlin 2.x treats Java `optString` as nullable — this avoids `?: ""` at every call site. */
internal fun JSONObject.safeOptString(key: String, fallback: String = ""): String =
    optString(key, fallback) ?: fallback

/**
 * Multi-key compat helpers: accept camelCase and snake_case variants,
 * returning the first non-null hit. Nullable return — caller decides the default.
 */
internal fun JSONObject.optBooleanCompat(vararg keys: String): Boolean? {
    keys.forEach { key ->
        if (has(key) && !isNull(key)) {
            return runCatching { getBoolean(key) }.getOrNull()
        }
    }
    return null
}

internal fun JSONObject.optIntCompat(vararg keys: String): Int? {
    keys.forEach { key ->
        if (has(key) && !isNull(key)) {
            return runCatching { getInt(key) }
                .getOrElse { runCatching { getDouble(key).toInt() }.getOrNull() }
        }
    }
    return null
}

internal fun JSONObject.optLongCompat(vararg keys: String): Long? {
    keys.forEach { key ->
        if (has(key) && !isNull(key)) {
            return runCatching { getLong(key) }
                .getOrElse { runCatching { getDouble(key).toLong() }.getOrNull() }
        }
    }
    return null
}

internal fun JSONObject.optDoubleCompat(vararg keys: String): Double? {
    keys.forEach { key ->
        if (has(key) && !isNull(key)) {
            return runCatching { getDouble(key) }.getOrNull()
        }
    }
    return null
}

internal fun JSONObject.optStringCompat(vararg keys: String): String? {
    keys.forEach { key ->
        if (has(key) && !isNull(key)) {
            return runCatching { getString(key) }.getOrNull()
        }
    }
    return null
}

/** Nullable string lookup that avoids the Kotlin 2.x `Nothing?` type-mismatch warning on `optString(key, null)`. */
internal fun JSONObject.optNullableString(key: String): String? =
    if (has(key) && !isNull(key)) getString(key) else null
