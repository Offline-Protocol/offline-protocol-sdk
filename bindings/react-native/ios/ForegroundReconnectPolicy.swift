//
// ForegroundReconnectPolicy.swift
// OfflineProtocol
//
// Decides whether a background→foreground transition should force-reconnect the
// relay socket. iOS suspends a backgrounded app within a few seconds, killing
// its TCP without a clean close (a "zombie" socket whose cached ready flags
// still report connected+authenticated). `isInternetReady()` cannot tell a
// healthy socket from a zombie, so we gate on how long the app actually stayed
// in the background: a stay long enough that the socket is untrustworthy forces
// a reconnect; a quick app-switch below the window keeps the live socket.
//
// Extracted as a standalone, unit-testable predicate (the same shape as the
// other bridge policy helpers) so this exact rule — the threshold, the
// nil-on-cold-launch behaviour, and the consume-on-read that stops a second
// foreground without an intervening background from re-firing — is pinned by
// `swift test` without the app toolchain, and so iOS and Android share one
// documented decision instead of two hand-copied gates that could drift.
//
// Times are monotonic, sleep-inclusive milliseconds supplied by the caller
// (iOS: `MonotonicClock.nowMs()`; Android: `SystemClock.elapsedRealtime()`),
// so the measured stay counts real time away — including device sleep — while
// staying immune to wall-clock steps (NTP correction, manual change).
//
// Not internally synchronized: the caller confines every call to the main
// thread (the lifecycle notifications that drive it are delivered there).
//

import Foundation

final class ForegroundReconnectPolicy {
    /// Minimum background stay before a foreground transition forces a
    /// reconnect. Aligned with the observed iOS ~4s background-disconnect window:
    /// below it a quick app-switch keeps the live socket; at or above it the
    /// socket can no longer be trusted.
    static let defaultMinBackgroundIntervalMs: Int64 = 4_000

    private let minBackgroundIntervalMs: Int64

    /// The monotonic reading at the last background transition, or nil when the
    /// app has not been backgrounded since the last foreground check (or since
    /// launch). nil is the cold-launch/no-prior-background state, which never
    /// triggers a reconnect.
    private var backgroundEnteredAtMs: Int64?

    init(minBackgroundIntervalMs: Int64 = ForegroundReconnectPolicy.defaultMinBackgroundIntervalMs) {
        self.minBackgroundIntervalMs = minBackgroundIntervalMs
    }

    /// Records the moment the app entered the background.
    func didEnterBackground(nowMs: Int64) {
        backgroundEnteredAtMs = nowMs
    }

    /// Returns whether the foreground transition should force-reconnect, and
    /// consumes the recorded background timestamp so a second foreground with no
    /// intervening background can never re-fire. Returns false when the app was
    /// not backgrounded first (cold launch, or a spurious duplicate foreground).
    func shouldReconnectOnForeground(nowMs: Int64) -> Bool {
        defer { backgroundEnteredAtMs = nil }
        guard let enteredAtMs = backgroundEnteredAtMs else { return false }
        return nowMs - enteredAtMs >= minBackgroundIntervalMs
    }
}
