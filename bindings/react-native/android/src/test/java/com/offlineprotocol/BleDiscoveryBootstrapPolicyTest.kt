package com.offlineprotocol

import org.junit.Assert.assertFalse
import org.junit.Assert.assertTrue
import org.junit.Test

class BleDiscoveryBootstrapPolicyTest {

    @Test
    fun `allows connectable unknown candidate on cold start with strong signal`() {
        val allowed = BleDiscoveryBootstrapPolicy.shouldAllowCandidate(
            isConnectable = true,
            currentConnectionCount = 0,
            maxConnectionsPerDevice = 4,
            estimatedVisiblePeerCount = 3,
            densePeerThreshold = 50,
            rssi = -62,
            hasScanRecord = false,
            minRssiWithScanRecord = -75,
            minRssiWithoutScanRecord = -68,
            lastAttemptAt = null,
            now = 1_000L,
            perDeviceCooldownMs = 12_000L,
            recentBootstrapAttempts = 0,
            maxBootstrapAttemptsPerMinute = 4,
            recentConnectionAttempts = 0,
            maxConnectionAttemptsPerMinute = 6
        )

        assertTrue(allowed)
    }

    @Test
    fun `rejects unknown candidate when signal is weak and advertisement is partial`() {
        val allowed = BleDiscoveryBootstrapPolicy.shouldAllowCandidate(
            isConnectable = true,
            currentConnectionCount = 0,
            maxConnectionsPerDevice = 4,
            estimatedVisiblePeerCount = 4,
            densePeerThreshold = 50,
            rssi = -82,
            hasScanRecord = false,
            minRssiWithScanRecord = -75,
            minRssiWithoutScanRecord = -68,
            lastAttemptAt = null,
            now = 2_000L,
            perDeviceCooldownMs = 12_000L,
            recentBootstrapAttempts = 0,
            maxBootstrapAttemptsPerMinute = 4,
            recentConnectionAttempts = 0,
            maxConnectionAttemptsPerMinute = 6
        )

        assertFalse(allowed)
    }

    @Test
    fun `rejects candidate when per-device cooldown is active`() {
        val allowed = BleDiscoveryBootstrapPolicy.shouldAllowCandidate(
            isConnectable = true,
            currentConnectionCount = 0,
            maxConnectionsPerDevice = 4,
            estimatedVisiblePeerCount = 2,
            densePeerThreshold = 50,
            rssi = -60,
            hasScanRecord = true,
            minRssiWithScanRecord = -75,
            minRssiWithoutScanRecord = -68,
            lastAttemptAt = 10_000L,
            now = 15_000L,
            perDeviceCooldownMs = 12_000L,
            recentBootstrapAttempts = 1,
            maxBootstrapAttemptsPerMinute = 4,
            recentConnectionAttempts = 1,
            maxConnectionAttemptsPerMinute = 6
        )

        assertFalse(allowed)
    }
}
