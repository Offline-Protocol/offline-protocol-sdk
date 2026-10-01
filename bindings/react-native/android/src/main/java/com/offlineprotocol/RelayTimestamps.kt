package com.offlineprotocol

/**
 * Parses the relay's last-seen timestamps to Unix ms.
 *
 * Accepts ISO-8601 or epoch milliseconds (the relay has sent both shapes);
 * returns null when absent or unparseable — a last-seen display must not
 * invent a timestamp. Mirrors ios/RelayTimestamps.swift — keep in sync.
 *
 * No `java.time`. `Instant` exists from API 26 and `minSdk` is 24, and on
 * API 24 or 25 the call throws `NoClassDefFoundError`, which is an `Error`
 * and passes through a `catch (e: Exception)`. Desugaring would have to be
 * switched on by every application, not by this library, so the parse is
 * written out here and behaves the same on every API level.
 *
 * It reads RFC 3339 internet date-time and answers what `Instant.parse`
 * answered for every timestamp the relay sends. It is not `Instant` at the
 * edges. It refuses hour 24 and a `.` with no digits after it, which RFC 3339
 * refuses too, and lowercase `t` or `z` and a leap second `:60`, which RFC
 * 3339 allows. `Instant.parse` accepted all four. No relay sends any of them.
 *
 * Stricter than the Swift twin: Foundation's formatter rolls 2023-02-29 over
 * into March and accepts 24:00 and a `+0530` offset, and this refuses all
 * three.
 */
object RelayTimestamps {
    /**
     * Below this an epoch can only be seconds (1e11 ms is March 1973, 1e11 s
     * is year ~5100); above it, milliseconds. Without the split, a
     * seconds-shaped `last_seen` renders as January 1970.
     */
    private const val EPOCH_SECONDS_CUTOFF = 100_000_000_000L

    /**
     * RFC 3339 internet date-time, what the Swift twin's
     * `ISO8601DateFormatter` accepts with `.withInternetDateTime`: a `T`, an
     * optional fraction, and `Z` or a numeric offset.
     *
     * `[0-9]`, never `\d`: on Android `\d` is ICU's and matches every Unicode
     * decimal digit, so `٢٠٢٤` would match here and then parse as 2024. The
     * JVM the unit tests run on reads `\d` as ASCII and cannot tell.
     */
    private val INTERNET_DATE_TIME = Regex(
        """([0-9]{4})-([0-9]{2})-([0-9]{2})T([0-9]{2}):([0-9]{2}):([0-9]{2})(?:\.([0-9]{1,9}))?(Z|([+-])([0-9]{2}):([0-9]{2}))"""
    )

    fun parseToMsOrNull(timestampStr: String): Long? {
        if (timestampStr.isEmpty()) return null
        timestampStr.toLongOrNull()?.let { return normalizeEpochToMs(it) }
        return parseIso8601ToMsOrNull(timestampStr)
    }

    /**
     * An RFC 3339 date-time as Unix ms, the fraction truncated to the
     * millisecond, or null when it is not one or names a date that does not
     * exist.
     */
    fun parseIso8601ToMsOrNull(timestampStr: String): Long? {
        val parts = INTERNET_DATE_TIME.matchEntire(timestampStr)?.groupValues ?: return null
        val year = parts[1].toInt()
        val month = parts[2].toInt()
        val day = parts[3].toInt()
        val hour = parts[4].toInt()
        val minute = parts[5].toInt()
        val second = parts[6].toInt()
        if (month !in 1..12 || day !in 1..daysInMonth(year, month)) return null
        if (hour > 23 || minute > 59 || second > 59) return null

        var offsetSeconds = 0L
        if (parts[8] != "Z") {
            val offsetHours = parts[10].toInt()
            val offsetMinutes = parts[11].toInt()
            if (offsetHours > 23 || offsetMinutes > 59) return null
            offsetSeconds = (offsetHours * 3600L + offsetMinutes * 60L) *
                (if (parts[9] == "-") -1 else 1)
        }

        val millis = parts[7].padEnd(3, '0').take(3).toLong()
        val seconds = daysSinceEpoch(year, month, day) * 86_400L +
            hour * 3600L + minute * 60L + second - offsetSeconds
        return seconds * 1000 + millis
    }

    /** Normalizes a numeric epoch (seconds or milliseconds) to milliseconds. */
    fun normalizeEpochToMs(value: Long): Long =
        if (value in 1 until EPOCH_SECONDS_CUTOFF) value * 1000 else value

    private fun isLeapYear(year: Int): Boolean =
        (year % 4 == 0 && year % 100 != 0) || year % 400 == 0

    private fun daysInMonth(year: Int, month: Int): Int = when (month) {
        2 -> if (isLeapYear(year)) 29 else 28
        4, 6, 9, 11 -> 30
        else -> 31
    }

    /**
     * Days from 1970-01-01 to a proleptic Gregorian date (Howard Hinnant's
     * `days_from_civil`).
     */
    private fun daysSinceEpoch(year: Int, month: Int, day: Int): Long {
        val y = (if (month <= 2) year - 1 else year).toLong()
        val era = Math.floorDiv(y, 400L)
        val yearOfEra = y - era * 400
        val shiftedMonth = if (month > 2) month - 3 else month + 9
        val dayOfYear = (153L * shiftedMonth + 2) / 5 + day - 1
        val dayOfEra = yearOfEra * 365 + yearOfEra / 4 - yearOfEra / 100 + dayOfYear
        return era * 146_097 + dayOfEra - 719_468
    }
}
