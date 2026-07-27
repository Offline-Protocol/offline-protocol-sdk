//
// WriteStallWatchdog.swift
// OfflineProtocol
//
// The iOS analogue of the Kotlin bridge's OkHttp `writeTimeout`. URLSession
// provides no write timeout for a WebSocket: a `URLSessionWebSocketTask.send`
// completion can hang for the full OS TCP timeout (~1min+) on a socket the OS
// silently killed during app suspension — with no error and no delegate
// callback (confirmed on Apple's dev forums, thread 726676 / 698065). On the
// iOS relay bridge that stalled write both black-holes all egress AND, if it is
// a control-op primary, pins `inFlightControlPrimaries` so the poll gate
// freezes the entire data plane (DMs, ACKs, typing, read receipts) for that
// whole window. Android never sees this: OkHttp's `writeTimeout` fails a hung
// write in seconds.
//
// This watchdog gives iOS the same bound. The bridge arms it immediately before
// each watched poll-path `task.send` and disarms it from that send's completion
// — both inside `sendWatched`, the single funnel that owns the whole arm/send/
// disarm triple, so a new send site cannot silently escape coverage on either
// end. The poll checks `stalledAgeMs` every tick and tears the socket down when
// the oldest outstanding write has aged past the timeout. teardownSocket's
// cancel then fires the hung completions promptly, and autoReconnect +
// flush_outbox re-drive the backlog.
//
// It tracks only send-START timestamps, never message identity: it answers one
// question — "has the oldest still-outstanding write exceeded the timeout?" —
// which is all socket-liveness needs.
//
// Each armed write gets an opaque `WriteToken`, and its own completion hands
// that token back to `disarm`, which retires exactly that entry. Retiring the
// write's OWN slot (rather than the oldest one around) is what keeps the head
// honest when completions arrive out of send order: popping oldest-first would
// let a fast write's completion discard a still-hung older write's timestamp
// and re-key the stall off a younger one, delaying the teardown by the gap
// between their send times. The count stays honest either way — the timestamp
// does not.
//
// Tokens are minted from a monotonic counter and never reused, which also
// closes the cross-connection hazard without any separate generation tag: a
// cancelled completion from a torn-down socket, arriving after `reset` has
// cleared the FIFO and a fresh socket has already armed new writes, carries a
// token that names no live entry, so its disarm is a no-op and can never pop
// the live successor's freshly-armed slot. `reset` on teardown remains the
// primary guard (it stops abandoned writes from aging into a false stall that
// would tear down the *current* socket, since `stalledAgeMs` looks at the head
// regardless of origin); token identity is the belt to that braces.
//
// SCOPE — this watches only poll-path data and control writes (the ones that
// can pin `inFlightControlPrimaries` and freeze the data plane). Ping, presence
// checks, raw commands, and auth writes are deliberately NOT watched: their
// stalling does not freeze the data plane, so they need no gate-unfreeze bound.
// The consequence is that a zombie appearing while the app is foreground-active
// with an EMPTY outbox arms nothing here and is not detected by this watchdog
// until the next data/control send — but an idle socket with nothing queued has
// no user-visible stall, and the suspension case (the common one) is already
// healed proactively by OfflineProtocolModule's foreground forceReconnect. So
// the uncovered case is "idle zombie, foreground, no traffic", which self-heals
// the instant traffic resumes. Widen coverage to those other send sites only if
// that case ever proves to matter.
//
// This is not internally synchronized; the caller confines it to a single serial
// queue (the bridge's messageQueue), exactly as it does for `pendingControlFrames`.
//
// There is no Android counterpart to mirror — OkHttp owns this on that side —
// but it follows the same standalone-policy-class shape as the other bridge
// helpers so `swift test` can cover it without the app toolchain.
//

import Foundation

final class WriteStallWatchdog {
    /// Default stall timeout. Matches the Kotlin bridge's OkHttp
    /// `writeTimeout` / `CONNECTION_TIMEOUT_MS` (10s): a healthy write completes
    /// in milliseconds, so this only ever fires on a genuinely dead socket.
    static let defaultTimeoutMs: Int64 = 10_000

    /// Identity for one armed write: minted by `arm`, handed back by that
    /// write's own send completion to `disarm`. Opaque and never reused, so a
    /// completion can only ever retire the slot it created — a late completion
    /// whose slot `reset` already dropped names nothing and disarms nothing.
    struct WriteToken: Equatable {
        fileprivate let id: UInt64
    }

    private let timeoutMs: Int64

    /// One still-outstanding write: its identity and its send-start timestamp
    /// (monotonic, sleep-inclusive time supplied by the caller). Ordered oldest
    /// first — appends are in send order and removals never reorder.
    private struct Outstanding {
        let token: WriteToken
        let startMs: Int64
    }

    private var outstanding: [Outstanding] = []

    /// Monotonic token source. Never reset (not even by `reset`) — reuse across
    /// connections is exactly what token identity exists to prevent.
    private var nextTokenId: UInt64 = 0

    init(timeoutMs: Int64 = WriteStallWatchdog.defaultTimeoutMs) {
        self.timeoutMs = timeoutMs
    }

    /// Records that a watched write was just handed to the socket, returning
    /// the token its completion must disarm with. Call immediately before
    /// `task.send`, after every early-return guard, so the watchdog never holds
    /// a slot for a write that was not actually issued.
    func arm(nowMs: Int64) -> WriteToken {
        nextTokenId += 1
        let token = WriteToken(id: nextTokenId)
        outstanding.append(Outstanding(token: token, startMs: nowMs))
        return token
    }

    /// Retires the write `token` was minted for. Call from that write's send
    /// completion, before any stale-task guard, so a cancelled (post-teardown)
    /// completion still frees its slot. A token with no tracked entry is a
    /// deliberate no-op: `reset` may have cleared the queue before a late
    /// cancelled completion runs — either way this must never disturb another
    /// write's slot, least of all a live successor connection's.
    func disarm(_ token: WriteToken) {
        if let idx = outstanding.firstIndex(where: { $0.token == token }) {
            outstanding.remove(at: idx)
        }
    }

    /// Drops all tracking. Call on socket teardown/stop so the next connection
    /// starts fresh and abandoned writes never age into a false positive.
    func reset() {
        outstanding.removeAll()
    }

    /// Number of writes currently outstanding (diagnostics only).
    var outstandingCount: Int {
        outstanding.count
    }

    /// The age of the oldest outstanding write if it has exceeded the timeout,
    /// otherwise nil. `nil` when nothing is outstanding or the oldest is still
    /// within budget — i.e. nil means "do not tear down".
    func stalledAgeMs(nowMs: Int64) -> Int64? {
        guard let oldest = outstanding.first else { return nil }
        let age = nowMs - oldest.startMs
        return age > timeoutMs ? age : nil
    }
}
