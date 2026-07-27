package com.offlineprotocol

/**
 * Decides whether a background→foreground transition should force-reconnect the
 * relay socket. Like iOS, Android can lose the relay TCP while the app is in the
 * background (Doze, OS process freeze, network handoff) without a clean
 * WebSocket close, leaving the cached ready flags stale-true against a dead or
 * relay-deregistered socket. `isReady()` cannot tell that apart from a healthy
 * socket, so we gate on how long the app actually stayed backgrounded: a stay
 * long enough that the socket is untrustworthy forces a reconnect; a quick
 * app-switch below the window keeps the live socket.
 *
 * Paired with iOS's [ForegroundReconnectPolicy].swift — both platforms drive the
 * same rule from their native lifecycle callbacks so the relay heals on
 * foreground automatically and identically, and the app no longer needs to call
 * `forceInternetReconnect()` on foreground itself.
 *
 * Times are monotonic, sleep-inclusive milliseconds supplied by the caller
 * (`SystemClock.elapsedRealtime()`), so the measured stay counts real time away
 * — including device sleep — while staying immune to wall-clock steps (NTP
 * correction, manual change).
 *
 * Not internally synchronized: the caller confines every call to the main
 * thread (React Native's host lifecycle callbacks are delivered there).
 */
class ForegroundReconnectPolicy(
    private val minBackgroundIntervalMs: Long = DEFAULT_MIN_BACKGROUND_INTERVAL_MS
) {
    companion object {
        /**
         * Minimum background stay before a foreground transition forces a
         * reconnect. Matches the iOS bridge's 4s window: below it a quick
         * app-switch keeps the live socket; at or above it the socket can no
         * longer be trusted.
         */
        const val DEFAULT_MIN_BACKGROUND_INTERVAL_MS = 4_000L
    }

    /**
     * The monotonic reading at the last background transition, or null when the
     * app has not been backgrounded since the last foreground check (or since
     * launch). null is the cold-launch/no-prior-background state, which never
     * triggers a reconnect.
     */
    private var backgroundEnteredAtMs: Long? = null

    /** Records the moment the app entered the background. */
    fun didEnterBackground(nowMs: Long) {
        backgroundEnteredAtMs = nowMs
    }

    /**
     * Returns whether the foreground transition should force-reconnect, and
     * consumes the recorded background timestamp so a second foreground with no
     * intervening background can never re-fire. Returns false when the app was
     * not backgrounded first (cold launch, or a spurious duplicate foreground).
     */
    fun shouldReconnectOnForeground(nowMs: Long): Boolean {
        val enteredAtMs = backgroundEnteredAtMs
        backgroundEnteredAtMs = null
        if (enteredAtMs == null) return false
        return nowMs - enteredAtMs >= minBackgroundIntervalMs
    }
}
