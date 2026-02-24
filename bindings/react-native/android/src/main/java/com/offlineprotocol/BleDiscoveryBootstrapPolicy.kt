package com.offlineprotocol

internal object BleDiscoveryBootstrapPolicy {
    fun shouldAllowCandidate(
        isConnectable: Boolean,
        currentConnectionCount: Int,
        maxConnectionsPerDevice: Int,
        estimatedVisiblePeerCount: Int,
        densePeerThreshold: Int,
        rssi: Int,
        hasScanRecord: Boolean,
        minRssiWithScanRecord: Int,
        minRssiWithoutScanRecord: Int,
        lastAttemptAt: Long?,
        now: Long,
        perDeviceCooldownMs: Long,
        recentBootstrapAttempts: Int,
        maxBootstrapAttemptsPerMinute: Int,
        recentConnectionAttempts: Int,
        maxConnectionAttemptsPerMinute: Int
    ): Boolean {
        if (!isConnectable) return false
        if (currentConnectionCount >= maxConnectionsPerDevice) return false
        if (estimatedVisiblePeerCount > densePeerThreshold) return false

        val minRssi = if (hasScanRecord) minRssiWithScanRecord else minRssiWithoutScanRecord
        if (rssi < minRssi) return false

        if (lastAttemptAt != null && now - lastAttemptAt < perDeviceCooldownMs) return false
        if (recentBootstrapAttempts >= maxBootstrapAttemptsPerMinute) return false
        if (recentConnectionAttempts >= maxConnectionAttemptsPerMinute) return false

        return true
    }
}
