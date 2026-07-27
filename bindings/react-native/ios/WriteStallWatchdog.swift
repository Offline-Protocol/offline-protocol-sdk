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
// each watched poll-path `task.send` and disarms it (retiring the oldest entry)
// from that send's completion; the poll checks `stalledAgeMs` every tick and
// tears the socket down when the oldest outstanding write has aged past the
// timeout. teardownSocket's cancel then fires the hung completions promptly, and
// autoReconnect + flush_outbox re-drive the backlog.
//
// It tracks only send-START timestamps, never message identity: it answers one
// question — "has the oldest still-outstanding write exceeded the timeout?" —
// which is all socket-liveness needs. Popping the front on any completion is
// correct even if completions arrive slightly out of send order: each completion
// retires exactly one write, so the count stays honest and the head stays
// monotonic. This is not internally synchronized; the caller confines it to a
// single serial queue (the bridge's messageQueue), exactly as it does for
// `pendingControlFrames`.
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
    /// Send-start timestamps of writes whose completion has not yet fired,
    /// oldest first. Monotonic, sleep-inclusive time supplied by the caller.
    private var startsMs: [Int64] = []

    init(timeoutMs: Int64 = WriteStallWatchdog.defaultTimeoutMs) {
        self.timeoutMs = timeoutMs
    }

    /// Records that a watched write was just handed to the socket. Call
    /// immediately before `task.send`, after every early-return guard, so the
    /// watchdog never holds a slot for a write that was not actually issued.
    func arm(nowMs: Int64) {
        startsMs.append(nowMs)
    }

    /// Retires the oldest outstanding write. Call from the send completion,
    /// before any stale-task guard, so a cancelled (post-teardown) completion
    /// still frees its slot. Empty is a deliberate no-op: `reset` may have
    /// cleared the queue before a late cancelled completion runs.
    func disarm() {
        if !startsMs.isEmpty {
            startsMs.removeFirst()
        }
    }

    /// Drops all tracking. Call on socket teardown/stop so the next connection
    /// starts fresh and abandoned writes never age into a false positive.
    func reset() {
        startsMs.removeAll()
    }

    /// Number of writes currently outstanding (diagnostics only).
    var outstandingCount: Int {
        startsMs.count
    }

    /// The age of the oldest outstanding write if it has exceeded the timeout,
    /// otherwise nil. `nil` when nothing is outstanding or the oldest is still
    /// within budget — i.e. nil means "do not tear down".
    func stalledAgeMs(nowMs: Int64) -> Int64? {
        guard let oldest = startsMs.first else { return nil }
        let age = nowMs - oldest
        return age > timeoutMs ? age : nil
    }
}
