package com.offlineprotocol

/**
 * Parses the relay's last-seen timestamps to Unix ms.
 *
 * Accepts ISO-8601 or epoch milliseconds (the relay has sent both shapes);
 * returns null when absent or unparseable — a last-seen display must not
 * invent a timestamp. Mirrors ios/RelayTimestamps.swift — keep in sync.
 */
object RelayTimestamps {
    /**
     * Below this an epoch can only be seconds (1e11 ms is March 1973, 1e11 s
     * is year ~5100); above it, milliseconds. Without the split, a
     * seconds-shaped `last_seen` renders as January 1970.
     */
    private const val EPOCH_SECONDS_CUTOFF = 100_000_000_000L

    fun parseToMsOrNull(timestampStr: String): Long? {
        if (timestampStr.isEmpty()) return null
        timestampStr.toLongOrNull()?.let { return normalizeEpochToMs(it) }
        return try {
            java.time.Instant.parse(timestampStr).toEpochMilli()
        } catch (e: Exception) {
            null
        }
    }

    /** Normalizes a numeric epoch (seconds or milliseconds) to milliseconds. */
    fun normalizeEpochToMs(value: Long): Long =
        if (value in 1 until EPOCH_SECONDS_CUTOFF) value * 1000 else value
}
