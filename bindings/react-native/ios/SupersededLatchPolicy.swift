//
// SupersededLatchPolicy.swift
// OfflineProtocol
//
// The relay-displacement ("session superseded") latch, extracted as a pure,
// Foundation-only state machine so it can be unit-tested without a live
// WebSocket (see tests/SupersededLatchPolicyTests.swift). InternetManager
// owns the threading (every call is made on main, the single writer); this
// type owns only the decision and the boolean.
//
// The relay closes a displaced connection with WebSocket close code 4000 (a
// newer registration for the same identity took the slot) and, redundantly,
// sends an application-level SessionSuperseded notice on the live socket
// first. Either signal must latch the transport stopped: a blind reconnect
// would just re-displace the peer socket in a ~1s loop — the fleet-wide
// eviction storm the relay-displacement rollout guards against. The latch is
// cleared only by an explicit start() (the app resolved "connected
// elsewhere" and re-enabled the transport).
//
// Mirrors android/src/main/java/com/offlineprotocol/SupersededLatchPolicy.kt
// — keep in sync.
//

import Foundation

final class SupersededLatchPolicy {
    /// WebSocket close code the relay uses to signal displacement.
    static let SUPERSEDED_CLOSE_CODE = 4000

    // Main-owned like the InternetManager flags it replaces (autoReconnect,
    // the connection bools): written on main, read best-effort off-main
    // (getMetrics). A plain Bool matches that established pattern.
    private var latched = false

    var isSuperseded: Bool { latched }

    /// Pure decision: does a close on some socket displace the transport?
    ///
    /// - Parameter closeCode: the WebSocket close code, or nil for a local /
    ///   error terminal (ping/auth/send-failure teardown) that carries none.
    /// - Parameter hasNewerSuccessor: a *different*, live socket has already
    ///   replaced the one this close belongs to. A late 4000 for a bygone
    ///   generation (old socket displaced → app re-enabled via start() → new
    ///   socket B up) must NOT re-latch and stop B. This is the sibling-race
    ///   the cd9fa39 fix targets. Callers whose socket-identity guard already
    ///   ran before reaching the decision (the Android close funnel) pass
    ///   false — a successor can never reach that point.
    ///
    /// An already-latched connection stays latched regardless of close code
    /// (a non-4000 close after a SessionSuperseded notice still stops).
    func shouldMark(closeCode: Int?, hasNewerSuccessor: Bool) -> Bool {
        if hasNewerSuccessor { return false }
        return latched || closeCode == Self.SUPERSEDED_CLOSE_CODE
    }

    /// Latches superseded. Returns true only on the false→true transition, so
    /// the caller fires the one-shot event exactly once even though the relay
    /// emits both a notice and a close (each of which fans into 2-3 terminal
    /// signals) for a single displacement.
    @discardableResult
    func mark() -> Bool {
        if latched { return false }
        latched = true
        return true
    }

    /// Cleared by an explicit start() — the deliberate re-enable.
    func clear() {
        latched = false
    }
}
