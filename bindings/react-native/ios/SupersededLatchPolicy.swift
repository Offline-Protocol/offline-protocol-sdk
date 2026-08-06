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
// **This type is also the source of truth for the event that reports the
// latch**, not just the boolean behind it. `internet_session_superseded` is
// strictly one-shot — nothing ever restates it, and every reconnect route
// refuses while latched — so an emit the bridge drops leaves the app showing
// a relay connection that is never coming back. The fix is to re-derive the
// report from this state at a later trigger rather than to buffer a copy of
// the emit (see OfflineProtocolModule.applicationWillEnterForeground), which
// is why the event's type tag, its payload shape and its reason live here:
//
//   - Being here puts them in a file the SwiftPM harness compiles, the CI
//     `swiftc -typecheck` probe covers and a unit test pins. The bridge module
//     that would otherwise own the tag literal is compiled by neither, so a
//     typo there would pass every check in CI and silently mis-tag the event.
//   - Restatement needs the *reason* to survive as long as the latch does.
//     Held only by the emit's argument, it dies with the dropped emit and a
//     restatement could report the displacement but not why.
//
// Mirrors android/src/main/java/com/offlineprotocol/SupersededLatchPolicy.kt
// — keep in sync.
//

import Foundation

final class SupersededLatchPolicy {
    /// WebSocket close code the relay uses to signal displacement.
    static let SUPERSEDED_CLOSE_CODE = 4000

    /// The bridge event tag reporting a relay displacement.
    ///
    /// Declared here, and asserted literally in the tests, because the two
    /// call sites that emit it live in OfflineProtocolModule.swift — which
    /// nothing in CI compiles (it is on Package.swift's `exclude:` list and
    /// absent from the ci.yml typecheck probe). Apps match on this string
    /// (`src/types.ts` InternetSessionSupersededEvent), so a drift is an event
    /// nobody receives, with nothing to restate it.
    static let EVENT_TYPE = "internet_session_superseded"

    // Main-owned like the InternetManager flags it replaces (autoReconnect,
    // the connection bools): written on main, read best-effort off-main
    // (getMetrics). A plain Bool matches that established pattern.
    private var latched = false

    // The reason carried by the displacement that latched, kept for as long as
    // the latch itself so a restatement can report it. First-wins with
    // `latched` (set only on the false→true transition in `mark`), because the
    // relay sends a SessionSuperseded notice *and* a close 4000 for one
    // displacement and only the first carries the relay's own explanation.
    private var latchedReason: String?

    var isSuperseded: Bool { latched }

    /// The reason the current latch was taken, if the displacement carried one.
    var supersedeReason: String? { latchedReason }

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

    /// Latches superseded, retaining `reason` for later restatement. Returns
    /// true only on the false→true transition, so the caller fires the
    /// one-shot event exactly once even though the relay emits both a notice
    /// and a close (each of which fans into 2-3 terminal signals) for a single
    /// displacement.
    ///
    /// The reason is stored on that same transition and not after it: the
    /// later signals for one displacement are the close-code paths, which
    /// carry no relay explanation, so a last-wins store would overwrite the
    /// notice's reason with nothing.
    @discardableResult
    func mark(reason: String? = nil) -> Bool {
        if latched { return false }
        latched = true
        latchedReason = reason
        return true
    }

    /// Cleared by an explicit start() — the deliberate re-enable.
    ///
    /// Drops the reason with the latch. They are one fact: a reason outliving
    /// its latch could only ever be attached to a *different* displacement,
    /// and `mark` would refuse to overwrite it.
    func clear() {
        latched = false
        latchedReason = nil
    }

    /// The `internet_session_superseded` payload for `reason`, as event JSON.
    ///
    /// Non-optional by construction so this can never become a silent-loss
    /// path for a one-shot event: serialization of a dictionary holding two
    /// `String`s cannot fail, and the unreachable branch still returns a
    /// well-formed event built from the constant alone — losing at worst the
    /// reason, never the report. The `reason` key is omitted rather than
    /// null when absent, matching what the bridge has emitted since 0.16.2.
    static func eventJson(reason: String?) -> String {
        var payload: [String: Any] = ["type": EVENT_TYPE]
        if let reason = reason { payload["reason"] = reason }
        if let data = try? JSONSerialization.data(withJSONObject: payload, options: []),
           let json = String(data: data, encoding: .utf8) {
            return json
        }
        return "{\"type\":\"\(EVENT_TYPE)\"}"
    }

    /// The event restating a *currently* superseded transport, or nil if it is
    /// not superseded.
    ///
    /// This is what makes the report recoverable without buffering it. The
    /// bridge calls it on app foreground: nil means there is nothing to say,
    /// and a non-nil result is true at the moment it is delivered rather than
    /// being a replay of a past edge — so there is no staleness window to
    /// bound, no session generation to stamp, and no discard site to remember
    /// at the two places that clear the latch. Both of those (`start()` and
    /// `enableTransport('internet')` reaching `InternetManager.start()`) are
    /// covered for free, because after either one this returns nil.
    func restatementEventJson() -> String? {
        guard latched else { return nil }
        return Self.eventJson(reason: latchedReason)
    }
}
