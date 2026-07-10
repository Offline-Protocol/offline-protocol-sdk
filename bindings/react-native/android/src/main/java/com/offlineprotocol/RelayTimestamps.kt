package com.offlineprotocol

/**
 * Parses the relay's last-seen timestamps to Unix ms.
 *
 * Accepts ISO-8601 or epoch milliseconds (the relay has sent both shapes);
 * returns null when absent or unparseable — a last-seen display must not
 * invent a timestamp. Mirrors ios/RelayTimestamps.swift — keep in sync.
 */
object RelayTimestamps {
    fun parseToMsOrNull(timestampStr: String): Long? {
        if (timestampStr.isEmpty()) return null
        timestampStr.toLongOrNull()?.let { return it }
        return try {
            java.time.Instant.parse(timestampStr).toEpochMilli()
        } catch (e: Exception) {
            null
        }
    }
}
