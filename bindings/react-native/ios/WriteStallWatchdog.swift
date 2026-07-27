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
// each watched poll-path `task.send` (via `sendWatched`, the single funnel that
// couples arm+send so a new send site cannot silently escape coverage) and
// disarms it from that send's completion; the poll checks `stalledAgeMs` every
// tick and tears the socket down when the oldest outstanding write has aged past
// the timeout. teardownSocket's cancel then fires the hung completions promptly,
// and autoReconnect + flush_outbox re-drive the backlog.
//
// It tracks only send-START timestamps, never message identity: it answers one
// question — "has the oldest still-outstanding write exceeded the timeout?" —
// which is all socket-liveness needs. Popping the oldest entry of the completing
// write's generation is correct even if completions arrive slightly out of send
// order: each completion retires exactly one write, so the count stays honest and
// the head stays monotonic.
//
// Each entry is tagged with the caller's socket generation (the same monotonic
// counter InternetManager stamps on `task.taskDescription`, see
// SocketGenerationTracker). `disarm` retires the oldest entry OF THAT GENERATION,
// and a disarm for a generation with no tracked entries is a deliberate no-op.
// That closes the one otherwise-possible cross-connection hazard: a cancelled
// completion from a torn-down socket, arriving after `reset` has cleared the FIFO
// and a fresh socket has already armed new writes, disarms its own (absent)
// generation and can never pop the live successor's freshly-armed slot. `reset`
// on teardown remains the primary guard (it stops abandoned writes from aging
// into a false stall that would tear down the *current* socket, since
// `stalledAgeMs` looks at the head regardless of generation); the generation tag
// is the belt to that braces.
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

    private let timeoutMs: Int64

    /// One still-outstanding write: its send-start timestamp (monotonic,
    /// sleep-inclusive time supplied by the caller) and the socket generation
    /// it was issued on. Ordered oldest first.
    private struct Outstanding {
        let startMs: Int64
        let generation: Int
    }

    private var outstanding: [Outstanding] = []

    init(timeoutMs: Int64 = WriteStallWatchdog.defaultTimeoutMs) {
        self.timeoutMs = timeoutMs
    }

    /// Records that a watched write was just handed to the socket on
    /// `generation`. Call immediately before `task.send`, after every
    /// early-return guard, so the watchdog never holds a slot for a write that
    /// was not actually issued.
    func arm(nowMs: Int64, generation: Int) {
        outstanding.append(Outstanding(startMs: nowMs, generation: generation))
    }

    /// Retires the oldest outstanding write belonging to `generation`. Call from
    /// the send completion, before any stale-task guard, so a cancelled
    /// (post-teardown) completion still frees its slot. A disarm for a
    /// generation with no tracked entries is a deliberate no-op: `reset` may have
    /// cleared the queue before a late cancelled completion runs, or a newer
    /// socket generation may already own every live entry — either way this must
    /// not pop a live successor's freshly-armed write.
    func disarm(generation: Int) {
        if let idx = outstanding.firstIndex(where: { $0.generation == generation }) {
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
