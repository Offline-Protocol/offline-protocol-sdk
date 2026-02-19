import Foundation

internal enum BleDiscoveryBootstrapPolicy {
    static func shouldAllowCandidate(
        isConnectable: Bool,
        currentConnectionCount: Int,
        maxConnectionsPerDevice: Int,
        estimatedVisiblePeerCount: Int,
        densePeerThreshold: Int,
        rssi: Int16,
        hasAnyServiceKey: Bool,
        minRssiWithServiceKeys: Int16,
        minRssiWithoutServiceKeys: Int16,
        lastAttemptAt: Date?,
        now: Date,
        perDeviceCooldown: TimeInterval,
        recentBootstrapAttempts: Int,
        maxBootstrapAttemptsPerMinute: Int,
        recentConnectionAttempts: Int,
        maxConnectionAttemptsPerMinute: Int
    ) -> Bool {
        if !isConnectable { return false }
        if currentConnectionCount >= maxConnectionsPerDevice { return false }
        if estimatedVisiblePeerCount > densePeerThreshold { return false }

        let minRssi = hasAnyServiceKey ? minRssiWithServiceKeys : minRssiWithoutServiceKeys
        if rssi < minRssi { return false }

        if let lastAttemptAt, now.timeIntervalSince(lastAttemptAt) < perDeviceCooldown { return false }
        if recentBootstrapAttempts >= maxBootstrapAttemptsPerMinute { return false }
        if recentConnectionAttempts >= maxConnectionAttemptsPerMinute { return false }

        return true
    }
}
