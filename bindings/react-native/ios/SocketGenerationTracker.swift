//
// SocketGenerationTracker.swift
// OfflineProtocol
//
// A monotonic socket-generation counter, extracted as a pure, Foundation-only
// helper so it can be unit-tested without a live WebSocket (see
// tests/SocketGenerationTrackerTests.swift). InternetManager owns the
// threading (every call is made on main, the single writer — the same
// discipline that governs webSocketTask); this type owns only the counter and
// the bygone-generation decision.
//
// Purpose: the relay-displacement latch (see SupersededLatchPolicy) has to
// answer, in didCloseWith, "does this close-4000 belong to the socket I still
// intend to run, or to a bygone one?". Object identity against the current
// webSocketTask cannot answer that during a reconnect backoff window, when
// webSocketTask is momentarily nil: nil means both "current generation,
// detached by a sibling terminal signal, reconnect pending" (must latch) and
// "an old superseded socket while a newer generation is already in flight"
// (must NOT latch). A per-socket generation disambiguates them: each socket is
// stamped with mint() at creation (carried on task.taskDescription), and a
// close whose generation is strictly older than latest is bygone regardless of
// whether webSocketTask is currently nil or a live successor.
//
// There is deliberately NO Kotlin mirror. The Android close funnel runs its
// socket-identity guard (terminateSocket) BEFORE the supersede decision, so a
// non-current socket's close — nil-window or successor — is dropped before it
// can ever latch; Android is immune to the bygone-generation false-latch by
// construction (see InternetManager.kt's ORDERING NOTE). Only the iOS funnel,
// which marks on the close code before its identity guard, needs this.
//

import Foundation

struct SocketGenerationTracker {
    /// The newest generation minted so far. Starts at 0 (no socket yet); the
    /// first mint() returns 1. Main-owned like webSocketTask.
    private(set) var latest: Int = 0

    /// Advances to and returns the next generation. Called once per socket
    /// creation (InternetManager.connect), which stamps the returned value on
    /// the task so didCloseWith can recover it.
    mutating func mint() -> Int {
        latest += 1
        return latest
    }

    /// True when `generation` belongs to a socket older than the newest one
    /// minted — a bygone generation whose late close must not re-latch a
    /// transport that has since moved on. The current generation (== latest)
    /// is not bygone, so a genuine displacement of the live socket still
    /// latches.
    func isBygone(_ generation: Int) -> Bool {
        generation < latest
    }
}
