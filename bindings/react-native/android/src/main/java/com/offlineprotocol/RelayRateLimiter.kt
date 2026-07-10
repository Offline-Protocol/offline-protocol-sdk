package com.offlineprotocol

/**
 * Client-side mirror of the relay's per-connection token bucket (30 burst,
 * 10 refill/s). Every relay-bound frame takes a token before the socket
 * write: OkHttp's `send()` only proves a local buffer write, so without this
 * a poll batch or a large registration's member deltas can burst past the
 * server bucket — the relay drops the overflow *after* the local write
 * "succeeded", which would let the bridge confirm (and the translator
 * commit) state the relay never recorded.
 *
 * Capacity/refill sit slightly under the relay's documented budget so a
 * frame this limiter doesn't meter (authentication) never tips the server
 * bucket. A `false` from [tryAcquire] always means "defer the frame to a
 * later tick", never "drop it".
 *
 * Callers pass `nowMs` explicitly (testability). Thread-safe. Mirrors
 * ios/RelayRateLimiter.swift — keep in sync.
 */
class RelayRateLimiter(
    private val capacity: Int = DEFAULT_CAPACITY,
    private val refillPerSecond: Double = DEFAULT_REFILL_PER_SECOND
) {
    companion object {
        /** Relay burst budget is 30; keep headroom for unmetered frames. */
        const val DEFAULT_CAPACITY = 28

        /** Relay refill is 10/s; same headroom. */
        const val DEFAULT_REFILL_PER_SECOND = 9.0
    }

    private val lock = Any()
    private var tokens = capacity.toDouble()
    private var lastRefillAtMs = 0L

    /** Takes one token, refilling first. */
    fun tryAcquire(nowMs: Long): Boolean = synchronized(lock) {
        refill(nowMs)
        if (tokens >= 1.0) {
            tokens -= 1.0
            true
        } else {
            false
        }
    }

    /** Returns a token acquired for a frame that was never written. */
    fun refund() {
        synchronized(lock) {
            tokens = minOf(capacity.toDouble(), tokens + 1.0)
        }
    }

    /**
     * Empties the bucket. Called when the relay answers `RateLimited`: the
     * local mirror was too optimistic, so force a full refill interval of
     * quiet before the next frame.
     */
    fun drain(nowMs: Long) {
        synchronized(lock) {
            refill(nowMs)
            tokens = 0.0
        }
    }

    private fun refill(nowMs: Long) {
        if (lastRefillAtMs == 0L) {
            lastRefillAtMs = nowMs
            return
        }
        val elapsedMs = nowMs - lastRefillAtMs
        if (elapsedMs <= 0) return
        tokens = minOf(capacity.toDouble(), tokens + elapsedMs / 1000.0 * refillPerSecond)
        lastRefillAtMs = nowMs
    }
}
