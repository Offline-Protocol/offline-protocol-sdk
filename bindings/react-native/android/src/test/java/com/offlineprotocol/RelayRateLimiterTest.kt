package com.offlineprotocol

import org.junit.Assert.assertFalse
import org.junit.Assert.assertTrue
import org.junit.Test

/**
 * Mirrors ios/tests/RelayRateLimiterTests.swift — keep in sync.
 */
class RelayRateLimiterTest {

    @Test
    fun allowsTheFullBurstThenDefers() {
        val limiter = RelayRateLimiter(capacity = 5, refillPerSecond = 1.0)
        val t0 = 1_000L
        repeat(5) { assertTrue(limiter.tryAcquire(t0)) }
        assertFalse(limiter.tryAcquire(t0))
    }

    @Test
    fun refillsAtTheConfiguredRate() {
        val limiter = RelayRateLimiter(capacity = 5, refillPerSecond = 2.0)
        val t0 = 1_000L
        repeat(5) { assertTrue(limiter.tryAcquire(t0)) }
        assertFalse(limiter.tryAcquire(t0))
        // 500ms at 2 tokens/s = exactly 1 token.
        assertTrue(limiter.tryAcquire(t0 + 500))
        assertFalse(limiter.tryAcquire(t0 + 500))
    }

    @Test
    fun refillNeverExceedsCapacity() {
        val limiter = RelayRateLimiter(capacity = 2, refillPerSecond = 1.0)
        val t0 = 1_000L
        assertTrue(limiter.tryAcquire(t0))
        // A long quiet period refills to capacity, not beyond.
        val later = t0 + 3_600_000
        repeat(2) { assertTrue(limiter.tryAcquire(later)) }
        assertFalse(limiter.tryAcquire(later))
    }

    @Test
    fun refundReturnsAnUnusedToken() {
        val limiter = RelayRateLimiter(capacity = 1, refillPerSecond = 1.0)
        val t0 = 1_000L
        assertTrue(limiter.tryAcquire(t0))
        assertFalse(limiter.tryAcquire(t0))
        limiter.refund()
        assertTrue(limiter.tryAcquire(t0))
    }

    @Test
    fun drainEmptiesTheBucketAndRefillResumes() {
        val limiter = RelayRateLimiter(capacity = 5, refillPerSecond = 1.0)
        val t0 = 1_000L
        limiter.drain(t0)
        assertFalse(limiter.tryAcquire(t0))
        assertTrue(limiter.tryAcquire(t0 + 1_000))
    }

    @Test
    fun clockGoingBackwardsDoesNotMintTokens() {
        val limiter = RelayRateLimiter(capacity = 1, refillPerSecond = 1000.0)
        val t0 = 10_000L
        assertTrue(limiter.tryAcquire(t0))
        assertFalse(limiter.tryAcquire(t0 - 5_000))
    }

    @Test
    fun defaultsSitUnderTheRelayBudget() {
        // The relay's documented bucket is 30 burst / 10 per second; the
        // client mirror must leave headroom for unmetered frames (auth).
        assertTrue(RelayRateLimiter.DEFAULT_CAPACITY < 30)
        assertTrue(RelayRateLimiter.DEFAULT_REFILL_PER_SECOND < 10.0)
    }
}
