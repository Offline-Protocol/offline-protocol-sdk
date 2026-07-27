//
// MonotonicClock.swift
// OfflineProtocol
//
// The single source of "monotonic, sleep-inclusive milliseconds" for the iOS
// bridge, extracted so InternetManager (rate limiter, in-flight tracker,
// presence watch, write-stall watchdog) and OfflineProtocolModule (the
// foreground-reconnect background-duration gate) measure the SAME real elapsed
// time from one implementation instead of two hand-copied copies that could
// silently drift apart — a drift that would look like a clock jump to every
// TTL that depends on them.
//
// mach_continuous_time is monotonic AND advances during device sleep (the true
// analogue of Android's SystemClock.elapsedRealtime), so a wall-clock step (an
// NTP correction, a manual change) can never freeze or over-mint token refill,
// mass-expire in-flight sends, or evict the whole watch set, and a device-sleep
// interval still counts toward every TTL instead of pausing it
// (mach_absolute_time-based clocks stop ticking during sleep).
//
// Foundation-only and stateless (only a process-constant timebase is cached),
// so it needs no threading discipline and unit tests can exercise callers
// without a live socket or the app toolchain.
//

import Foundation

enum MonotonicClock {
    /// Cached mach timebase (constant for the process).
    private static let timebase: mach_timebase_info_data_t = {
        var info = mach_timebase_info_data_t()
        mach_timebase_info(&info)
        return info
    }()

    /// Monotonic, sleep-inclusive milliseconds (mach_continuous_time).
    static func nowMs() -> Int64 {
        let ticks = mach_continuous_time()
        // Split multiply-then-divide so ticks * numer can't overflow UInt64;
        // the sub-tick truncation is nanoseconds-scale, irrelevant at ms.
        let numer = UInt64(timebase.numer)
        let denom = UInt64(timebase.denom)
        let nanos = (ticks / denom) * numer + (ticks % denom) * numer / denom
        return Int64(nanos / 1_000_000)
    }
}
