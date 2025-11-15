import Foundation
import CryptoKit

/// Shared advertisement payload between iOS and Android mesh implementations.
/// Layout mirrors `MeshAdvertisementData` on Android.
struct MeshAdvertisementData {
    static let currentVersion: UInt8 = 2
    static let featureScoreBased: UInt8 = 0x01

    let version: UInt8
    let degree: UInt8
    let freeSlotEstimate: UInt8
    let nodeScore: Double
    let uptimeSeconds: UInt32
    let batteryPercent: UInt8?
    let loadPercent: UInt8?
    let rssiToYou: Int8?
    let nodeIdHash: UInt64
    let featureFlags: UInt8

    init(
        version: UInt8 = MeshAdvertisementData.currentVersion,
        degree: UInt8,
        freeSlotEstimate: UInt8,
        nodeScore: Double,
        uptimeSeconds: UInt32,
        batteryPercent: UInt8?,
        loadPercent: UInt8?,
        rssiToYou: Int8?,
        nodeIdHash: UInt64,
        featureFlags: UInt8 = MeshAdvertisementData.featureScoreBased
    ) {
        precondition(version == MeshAdvertisementData.currentVersion, "Unsupported advertisement version")
        precondition(degree <= 0x0F, "degree exceeds 4-bit field")
        precondition(freeSlotEstimate <= 0x0F, "freeSlotEstimate exceeds 4-bit field")
        precondition(nodeScore.isNaN || (0.0...1.0).contains(nodeScore), "nodeScore must be within [0,1]")
        precondition(batteryPercent == nil || batteryPercent! <= 100, "batteryPercent out of range")
        precondition(loadPercent == nil || loadPercent! <= 100, "loadPercent out of range")
        precondition(rssiToYou == nil || (-127...127).contains(rssiToYou!), "rssiToYou out of range")
        self.version = version
        self.degree = degree
        self.freeSlotEstimate = freeSlotEstimate
        self.nodeScore = nodeScore
        self.uptimeSeconds = uptimeSeconds
        self.batteryPercent = batteryPercent
        self.loadPercent = loadPercent
        self.rssiToYou = rssiToYou
        self.nodeIdHash = nodeIdHash
        self.featureFlags = featureFlags
    }

    func encode() -> Data {
        var buffer = Data(capacity: 19)
        buffer.append(version)
        let slotByte = ((degree & 0x0F) << 4) | (freeSlotEstimate & 0x0F)
        buffer.append(slotByte)

        let scoreByte: UInt8
        if nodeScore.isNaN || nodeScore <= 0 {
            scoreByte = 0
        } else if nodeScore >= 1.0 {
            scoreByte = 255
        } else {
            scoreByte = UInt8((nodeScore * 255.0).rounded().clamped(to: 0...255))
        }
        buffer.append(scoreByte)

        buffer.append(encodeOptionalPercent(batteryPercent))
        buffer.append(encodeOptionalPercent(loadPercent))
        buffer.append(encodeOptionalRssi(rssiToYou))

        var uptime = uptimeSeconds.bigEndian
        withUnsafeBytes(of: &uptime) { buffer.append(contentsOf: $0) }

        var hash = nodeIdHash.bigEndian
        withUnsafeBytes(of: &hash) { buffer.append(contentsOf: $0) }

        buffer.append(featureFlags)
        return buffer
    }

    static func decode(_ data: Data?) -> MeshAdvertisementData? {
        guard let data = data, data.count >= 19 else { return nil }
        var index = 0
        let version = data[index]
        index += 1
        guard version == MeshAdvertisementData.currentVersion else { return nil }

        let slotByte = data[index]
        index += 1
        let degree = (slotByte >> 4) & 0x0F
        let freeSlotEstimate = slotByte & 0x0F

        let scoreByte = data[index]
        index += 1
        let nodeScore = Double(scoreByte) / 255.0

        let battery = decodeOptionalPercent(data[index])
        index += 1
        let load = decodeOptionalPercent(data[index])
        index += 1
        let rssi = decodeOptionalRssi(data[index])
        index += 1

        let uptimeBytes = data[index..<(index + 4)]
        index += 4
        let uptimeSeconds = uptimeBytes.withUnsafeBytes { $0.load(as: UInt32.self).bigEndian }

        let hashBytes = data[index..<(index + 8)]
        index += 8
        let nodeIdHash = hashBytes.withUnsafeBytes { $0.load(as: UInt64.self).bigEndian }

        let featureFlags = data[index]

        return MeshAdvertisementData(
            degree: degree,
            freeSlotEstimate: freeSlotEstimate,
            nodeScore: nodeScore,
            uptimeSeconds: uptimeSeconds,
            batteryPercent: battery,
            loadPercent: load,
            rssiToYou: rssi,
            nodeIdHash: nodeIdHash,
            featureFlags: featureFlags
        )
    }

    private func encodeOptionalPercent(_ value: UInt8?) -> UInt8 {
        value ?? 0xFF
    }

    private func encodeOptionalRssi(_ value: Int8?) -> UInt8 {
        guard let value = value else { return 0xFF }
        let shifted = (Int(value) + 127).clamped(to: 0...254)
        return UInt8(shifted)
    }

    private static func decodeOptionalPercent(_ byte: UInt8) -> UInt8? {
        byte <= 100 ? byte : nil
    }

    private static func decodeOptionalRssi(_ byte: UInt8) -> Int8? {
        guard byte <= 254 else { return nil }
        let shifted = Int(byte) - 127
        return Int8(shifted.clamped(to: -127...127))
    }
}

// MARK: - Mesh Controller

final class MeshController {
    struct MeshConfig {
        let minConnections: Int
        let maxConnections: Int
        let metadataTTL: TimeInterval
        let rebalanceInterval: TimeInterval
        let connectionCooldown: TimeInterval
        let scoreWeights: ScoreWeights
        let uptimeSaturation: TimeInterval
        let scoreHysteresis: Double
        let bridgeFavor: Double
        let scoreEquivalenceEpsilon: Double

        init(
            minConnections: Int = 1,
            maxConnections: Int = 4,
            metadataTTL: TimeInterval = 120.0,
            rebalanceInterval: TimeInterval = 15.0,
            connectionCooldown: TimeInterval = 7.5,
            scoreWeights: ScoreWeights = ScoreWeights(),
            uptimeSaturation: TimeInterval = 3600.0,
            scoreHysteresis: Double = 0.05,
            bridgeFavor: Double = 0.1,
            scoreEquivalenceEpsilon: Double = 0.02
        ) {
            self.minConnections = minConnections
            self.maxConnections = maxConnections
            self.metadataTTL = metadataTTL
            self.rebalanceInterval = rebalanceInterval
            self.connectionCooldown = connectionCooldown
            self.scoreWeights = scoreWeights
            self.uptimeSaturation = uptimeSaturation
            self.scoreHysteresis = scoreHysteresis
            self.bridgeFavor = bridgeFavor
            self.scoreEquivalenceEpsilon = scoreEquivalenceEpsilon
        }
    }

    struct ScoreWeights {
        let rssi: Double
        let availability: Double
        let uptime: Double
        let battery: Double
        let stability: Double
        let load: Double

        init(
            rssi: Double = 0.35,
            availability: Double = 0.2,
            uptime: Double = 0.15,
            battery: Double = 0.15,
            stability: Double = 0.1,
            load: Double = 0.05
        ) {
            self.rssi = rssi
            self.availability = availability
            self.uptime = uptime
            self.battery = battery
            self.stability = stability
            self.load = load
        }

        func normalized() -> ScoreWeights {
            let total = max(rssi + availability + uptime + battery + stability + load, 0.00001)
            return ScoreWeights(
                rssi: rssi / total,
                availability: availability / total,
                uptime: uptime / total,
                battery: battery / total,
                stability: stability / total,
                load: load / total
            )
        }
    }

    struct PeerMetrics {
        var rssi: Int?
        var batteryPercent: Int?
        var signalQuality: Int?
        var stability: Double?
        var uptimeSeconds: TimeInterval?
        var loadPercent: Int?

        init(
            rssi: Int? = nil,
            batteryPercent: Int? = nil,
            signalQuality: Int? = nil,
            stability: Double? = nil,
            uptimeSeconds: TimeInterval? = nil,
            loadPercent: Int? = nil
        ) {
            self.rssi = rssi
            self.batteryPercent = batteryPercent
            self.signalQuality = signalQuality
            self.stability = stability
            self.uptimeSeconds = uptimeSeconds
            self.loadPercent = loadPercent
        }
    }

    enum MeshRole {
        case member
        case bridge
    }

    enum ConnectionIntent {
        case intraCluster
        case interCluster
        case rejected
    }

    struct MeshDecision {
        let intent: ConnectionIntent
        let reason: String
        let evictPeerId: String?

        init(intent: ConnectionIntent, reason: String, evictPeerId: String? = nil) {
            self.intent = intent
            self.reason = reason
            self.evictPeerId = evictPeerId
        }
    }

    struct RebalanceDirective {
        let decision: MeshDecision
        let candidate: MeshAdvertisementData
    }

    private final class PeerState {
        var deviceId: String
        var nodeHash: UInt64?
        var role: MeshRole
        var metrics: PeerMetrics
        var lastUpdated: Date
        var lastActivity: Date
        var advertisedDegree: Int
        var advertisedFreeSlots: Int
        var advertisedScore: Double
        var advertisedUptimeSeconds: TimeInterval
        var advertisedBatteryPercent: Int?
        var advertisedLoadPercent: Int?
        var advertisedRssiToUs: Int?
        var observedRssi: Int?

        init(
            deviceId: String,
            nodeHash: UInt64?,
            role: MeshRole = .member,
            metrics: PeerMetrics = PeerMetrics(),
            lastUpdated: Date,
            lastActivity: Date
        ) {
            self.deviceId = deviceId
            self.nodeHash = nodeHash
            self.role = role
            self.metrics = metrics
            self.lastUpdated = lastUpdated
            self.lastActivity = lastActivity
            self.advertisedDegree = 0
            self.advertisedFreeSlots = 0
            self.advertisedScore = 0
            self.advertisedUptimeSeconds = 0
            self.advertisedBatteryPercent = nil
            self.advertisedLoadPercent = nil
            self.advertisedRssiToUs = nil
            self.observedRssi = nil
        }
    }

    private struct RemoteCandidate {
        var metadata: MeshAdvertisementData
        var observedAt: Date
        var rssi: Int?
    }

    private let selfId: String
    private let config: MeshConfig
    private let timeProvider: () -> Date
    private let startTimestamp: Date
    private let weights: ScoreWeights
    private let queue = DispatchQueue(label: "com.offlineprotocol.mesh.controller", attributes: .concurrent)

    private var peersById: [String: PeerState] = [:]
    private var peersByHash: [UInt64: PeerState] = [:]
    private var activeConnections: [String: MeshRole] = [:]
    private var candidatesByHash: [UInt64: RemoteCandidate] = [:]
    private var lastRebalanceAt: Date
    private var selfMetrics = PeerMetrics()

    private static let defaultRemoteScore: Double = 0.55

    init(
        selfId: String,
        config: MeshConfig = MeshConfig(),
        timeProvider: @escaping () -> Date = Date.init
    ) {
        self.selfId = selfId
        self.config = config
        self.timeProvider = timeProvider
        self.startTimestamp = timeProvider()
        self.weights = config.scoreWeights.normalized()
        self.lastRebalanceAt = startTimestamp

        let initialState = PeerState(
            deviceId: selfId,
            nodeHash: MeshController.hash64(selfId),
            role: .member,
            metrics: PeerMetrics(),
            lastUpdated: startTimestamp,
            lastActivity: startTimestamp
        )

        queue.async(flags: .barrier) {
            self.peersById[selfId] = initialState
            if let hash = initialState.nodeHash {
                self.peersByHash[hash] = initialState
            }
        }
    }

    func updateSelfMetrics(_ metrics: PeerMetrics) {
        queue.async(flags: .barrier) {
            let now = self.timeProvider()
            self.selfMetrics = metrics
            let state = self.peersById[self.selfId] ?? PeerState(
                deviceId: self.selfId,
                nodeHash: MeshController.hash64(self.selfId),
                role: .member,
                metrics: metrics,
                lastUpdated: now,
                lastActivity: now
            )
            state.metrics = metrics
            state.lastUpdated = now
            state.lastActivity = now
            if state.nodeHash == nil {
                state.nodeHash = MeshController.hash64(self.selfId)
            }
            self.peersById[self.selfId] = state
            if let hash = state.nodeHash {
                self.peersByHash[hash] = state
            }
        }
    }

    func noteSelfMetrics(_ metrics: PeerMetrics) {
        updateSelfMetrics(metrics)
    }

    func advertisement() -> MeshAdvertisementData {
        queue.sync {
            let now = timeProvider()
            let elapsed = now.timeIntervalSince(startTimestamp)
            let uptimeSeconds = UInt32(max(0, min(elapsed, Double(UInt32.max))))
            let degree = activeConnections.count
            let freeSlots = max(0, config.maxConnections - degree)
            let availability = availabilityFactor(degree: degree, freeSlots: freeSlots)
            let score = computeNodeScore(
                metrics: selfMetrics,
                availability: availability,
                uptimeSeconds: selfMetrics.uptimeSeconds ?? elapsed
            ).clamped(to: 0.0...1.0)
            let nodeHash = peersById[selfId]?.nodeHash ?? MeshController.hash64(selfId)
            let degreeByte = UInt8(min(degree, 15))
            let freeSlotByte = UInt8(min(freeSlots, 15))
            let battery = selfMetrics.batteryPercent.map { UInt8(min(max($0, 0), 100)) }
            let load = selfMetrics.loadPercent.map { UInt8(min(max($0, 0), 100)) }
            let rssi = selfMetrics.rssi.map { Int8(min(max($0, -127), 127)) }

            return MeshAdvertisementData(
                degree: degreeByte,
                freeSlotEstimate: freeSlotByte,
                nodeScore: score,
                uptimeSeconds: uptimeSeconds,
                batteryPercent: battery,
                loadPercent: load,
                rssiToYou: rssi,
                nodeIdHash: nodeHash
            )
        }
    }

    func observeAdvertisement(_ data: MeshAdvertisementData?, rssi: Int?) {
        guard let data = data else { return }
        queue.async(flags: .barrier) {
            let now = self.timeProvider()
            let nodeHash = data.nodeIdHash
            let state = self.peersByHash[nodeHash] ?? PeerState(
                deviceId: "hash-\(String(nodeHash, radix: 16))",
                nodeHash: nodeHash,
                    role: .member,
                    metrics: PeerMetrics(),
                    lastUpdated: now,
                    lastActivity: now
                )

            state.advertisedDegree = Int(data.degree)
            state.advertisedFreeSlots = Int(data.freeSlotEstimate)
            state.advertisedScore = data.nodeScore.isNaN ? 0.0 : data.nodeScore
            state.advertisedUptimeSeconds = TimeInterval(data.uptimeSeconds)
            state.advertisedBatteryPercent = data.batteryPercent.map { Int($0) }
            state.advertisedLoadPercent = data.loadPercent.map { Int($0) }
            state.advertisedRssiToUs = data.rssiToYou.map { Int($0) }
            state.observedRssi = rssi
            state.lastUpdated = now
            state.lastActivity = now

            self.peersByHash[nodeHash] = state
            if self.peersById[state.deviceId] == nil {
                self.peersById[state.deviceId] = state
            }
            self.candidatesByHash[nodeHash] = RemoteCandidate(metadata: data, observedAt: now, rssi: rssi)
            self.pruneExpiredCandidates(now: now)
        }
    }

    func shouldAcceptInboundConnection(
        remoteId: String?,
        metadata: MeshAdvertisementData?,
        rssi: Int?
    ) -> MeshDecision {
        if let metadata = metadata {
            observeAdvertisement(metadata, rssi: rssi)
        }

        return queue.sync {
            let degree = activeConnections.count
            let freeSlots = config.maxConnections - degree
            if freeSlots > 0 {
                return MeshDecision(intent: .intraCluster, reason: "capacity_available")
            }

            if let metadata = metadata, let worstPeer = findWorstActivePeer(), degree - 1 >= config.minConnections {
                let candidateScore = computeCandidateScore(metadata: metadata, rssi: rssi)
                let worstScore = computePeerScore(peer: worstPeer)
                if let reason = evaluateSwapCandidate(candidate: metadata, candidateScore: candidateScore, worstPeer: worstPeer, worstScore: worstScore) {
                    return MeshDecision(intent: .intraCluster, reason: reason, evictPeerId: worstPeer.deviceId)
                }
            }

            if findWorstActivePeer() == nil {
                return MeshDecision(intent: .rejected, reason: "no_active_links")
            }

            if degree < config.minConnections {
                return MeshDecision(intent: .intraCluster, reason: "protect_min_degree")
            }

            return MeshDecision(intent: .rejected, reason: "local_links_preferred")
        }
    }

    func shouldAcceptInboundConnection(_ remoteId: String?) -> MeshDecision {
        shouldAcceptInboundConnection(remoteId: remoteId, metadata: nil, rssi: nil)
    }

    func shouldInitiateOutbound(metadata: MeshAdvertisementData?, rssi: Int?) -> MeshDecision {
        guard let metadata = metadata else {
            return MeshDecision(intent: .rejected, reason: "no_metadata")
        }
        observeAdvertisement(metadata, rssi: rssi)

        return queue.sync {
            let degree = activeConnections.count
            let freeSlots = config.maxConnections - degree
            if freeSlots > 0 {
                let intent: ConnectionIntent = metadata.freeSlotEstimate == 0 ? .interCluster : .intraCluster
                return MeshDecision(intent: intent, reason: "capacity_available")
            }

            guard let worstPeer = findWorstActivePeer() else {
                return MeshDecision(intent: .rejected, reason: "no_active_links")
            }

            let candidateScore = computeCandidateScore(metadata: metadata, rssi: rssi)
            let worstScore = computePeerScore(peer: worstPeer)

            if degree - 1 >= config.minConnections,
               let reason = evaluateSwapCandidate(candidate: metadata, candidateScore: candidateScore, worstPeer: worstPeer, worstScore: worstScore) {
                let intent: ConnectionIntent = metadata.freeSlotEstimate == 0 ? .interCluster : .intraCluster
                return MeshDecision(intent: intent, reason: reason, evictPeerId: worstPeer.deviceId)
            }

            return MeshDecision(intent: .rejected, reason: "local_links_preferred")
        }
    }

    func registerConnection(peerId: String, role: MeshRole) {
        queue.async(flags: .barrier) {
            let now = self.timeProvider()
            let state = self.peersById[peerId] ?? PeerState(
                deviceId: peerId,
                nodeHash: nil,
                role: role,
                metrics: PeerMetrics(),
                lastUpdated: now,
                lastActivity: now
            )
            state.role = role
            state.lastUpdated = now
            state.lastActivity = now
            self.peersById[peerId] = state
            self.activeConnections[peerId] = role
        }
    }

    func updatePeerMetrics(peerId: String, metrics: PeerMetrics) {
        queue.async(flags: .barrier) {
            let now = self.timeProvider()
            let state = self.peersById[peerId] ?? PeerState(
                deviceId: peerId,
                nodeHash: nil,
                role: .member,
                metrics: metrics,
                lastUpdated: now,
                lastActivity: now
            )
            state.metrics = metrics
            state.lastUpdated = now
            state.lastActivity = now
            self.peersById[peerId] = state
        }
    }

    func registerDisconnection(peerId: String) {
        queue.async(flags: .barrier) {
            self.activeConnections.removeValue(forKey: peerId)
            self.peersById[peerId]?.lastActivity = self.timeProvider()
        }
    }

    func markPeerActive(_ peerId: String) {
        queue.async(flags: .barrier) {
            let now = self.timeProvider()
            let state = self.peersById[peerId] ?? PeerState(
                deviceId: peerId,
                nodeHash: nil,
                role: .member,
                metrics: PeerMetrics(),
                lastUpdated: now,
                lastActivity: now
            )
            state.lastActivity = now
            self.peersById[peerId] = state
        }
    }

    func connectionBudgetAvailable() -> Bool {
        queue.sync { activeConnections.count < config.maxConnections }
    }

    func clusterHasCapacity() -> Bool {
        true
    }

    func evaluateRebalance() -> RebalanceDirective? {
        queue.sync {
            let now = timeProvider()
            guard now.timeIntervalSince(lastRebalanceAt) >= config.rebalanceInterval else { return nil }
            pruneExpiredCandidates(now: now)

            guard let bestCandidate = candidatesByHash.values.max(by: {
                computeCandidateScore(metadata: $0.metadata, rssi: $0.rssi) <
                computeCandidateScore(metadata: $1.metadata, rssi: $1.rssi)
            }) else {
                lastRebalanceAt = now
                return nil
            }

            let degree = activeConnections.count
            let freeSlots = config.maxConnections - degree

            let candidateScore = computeCandidateScore(metadata: bestCandidate.metadata, rssi: bestCandidate.rssi)

            if freeSlots > 0 {
                lastRebalanceAt = now
                let intent: ConnectionIntent = bestCandidate.metadata.freeSlotEstimate == 0 ? .interCluster : .intraCluster
                return RebalanceDirective(
                    decision: MeshDecision(intent: intent, reason: "rebalance_connect", evictPeerId: nil),
                    candidate: bestCandidate.metadata
                )
            }

            guard let worstPeer = findWorstActivePeer(), degree - 1 >= config.minConnections else {
                lastRebalanceAt = now
                return nil
            }

            let candidateScore = computeCandidateScore(metadata: bestCandidate.metadata, rssi: bestCandidate.rssi)
            let worstScore = computePeerScore(peer: worstPeer)

            guard let reason = evaluateSwapCandidate(
                candidate: bestCandidate.metadata,
                candidateScore: candidateScore,
                worstPeer: worstPeer,
                worstScore: worstScore
            ) else {
                lastRebalanceAt = now
                return nil
            }

            lastRebalanceAt = now
            let intent: ConnectionIntent = bestCandidate.metadata.freeSlotEstimate == 0 ? .interCluster : .intraCluster
            let mappedReason = reason == "swap_low_score_peer" ? "rebalance_swap" : "rebalance_bridge"
            return RebalanceDirective(
                decision: MeshDecision(intent: intent, reason: mappedReason, evictPeerId: worstPeer.deviceId),
                candidate: bestCandidate.metadata
            )
        }
    }

    // Compatibility no-ops for previous API
    @discardableResult
    func addLeaderListener(_ listener: @escaping (String) -> Void) -> UUID {
        listener(selfId)
        return UUID()
    }

    func removeLeaderListener(_ token: UUID) { }

    func currentLeaderId() -> String { selfId }

    func isSelfLeader() -> Bool { true }

    // MARK: - Helpers

    private func availabilityFactor(degree: Int, freeSlots: Int) -> Double {
        guard config.maxConnections > 0 else { return 0.0 }
        let maxConnections = Double(config.maxConnections)
        let normalizedDegree = Double(degree.clamped(to: 0...config.maxConnections)) / maxConnections
        let normalizedFree = Double(freeSlots.clamped(to: 0...config.maxConnections)) / maxConnections
        return ((1.0 - normalizedDegree) + normalizedFree) / 2.0
    }

    private func computeCandidateScore(metadata: MeshAdvertisementData, rssi: Int?) -> Double {
        let availability = availabilityFactor(
            degree: Int(metadata.degree),
            freeSlots: Int(metadata.freeSlotEstimate)
        )
        let metrics = PeerMetrics(
            rssi: rssi ?? metadata.rssiToYou.map { Int($0) },
            batteryPercent: metadata.batteryPercent.map { Int($0) },
            uptimeSeconds: TimeInterval(metadata.uptimeSeconds),
            loadPercent: metadata.loadPercent.map { Int($0) }
        )
        let computed = computeNodeScore(
            metrics: metrics,
            availability: availability,
            uptimeSeconds: metrics.uptimeSeconds ?? TimeInterval(metadata.uptimeSeconds)
        )
        return max(computed, metadata.nodeScore.isNaN ? 0.0 : metadata.nodeScore)
    }

    private func evaluateSwapCandidate(
        candidate: MeshAdvertisementData,
        candidateScore: Double,
        worstPeer: PeerState,
        worstScore: Double
    ) -> String? {
        let candidateRssiScore = normalizeRssi(candidate.rssiToYou.map { Int($0) })
        let worstRssiScore = peerRssiScore(worstPeer)
        let proximityAdvantage = candidateRssiScore + config.bridgeFavor >= worstRssiScore

        if candidateScore > worstScore + config.scoreHysteresis {
            return "swap_low_score_peer"
        }
        let peerFreeSlots = worstPeer.advertisedFreeSlots
        let availabilityGain = Int(candidate.freeSlotEstimate) - peerFreeSlots
        let candidateHasCapacity = candidate.freeSlotEstimate > 0
        let peerSaturated = peerFreeSlots <= 0
        let scoreWithinBridgeFavor = candidateScore + config.bridgeFavor >= worstScore
        if scoreWithinBridgeFavor && proximityAdvantage && (availabilityGain > 0 || (candidateHasCapacity && peerSaturated)) {
            return "swap_bridge_capacity"
        }

        let candidateUnderserved = Int(candidate.degree) < config.minConnections
        let selfHasSurplus = activeConnections.count > config.minConnections

        if candidateUnderserved && selfHasSurplus && proximityAdvantage {
            return "swap_bridge_capacity"
        }

        if abs(candidateScore - worstScore) <= config.scoreEquivalenceEpsilon && selfHasSurplus {
            return "swap_equivalent_peer"
        }

        return nil
    }

    private func peerRssiScore(_ peer: PeerState) -> Double {
        let rssi = peer.metrics.rssi ?? peer.observedRssi ?? peer.advertisedRssiToUs
        return normalizeRssi(rssi)
    }

    private func computePeerScore(peer: PeerState) -> Double {
        var metrics = peer.metrics
        if metrics.rssi == nil {
            metrics.rssi = peer.observedRssi ?? peer.advertisedRssiToUs
        }
        if metrics.batteryPercent == nil {
            metrics.batteryPercent = peer.advertisedBatteryPercent
        }
        if metrics.loadPercent == nil {
            metrics.loadPercent = peer.advertisedLoadPercent
        }
        if metrics.uptimeSeconds == nil {
            metrics.uptimeSeconds = peer.advertisedUptimeSeconds
        }
        let availability = availabilityFactor(
            degree: peer.advertisedDegree,
            freeSlots: peer.advertisedFreeSlots
        )
        let computed = computeNodeScore(
            metrics: metrics,
            availability: availability,
            uptimeSeconds: metrics.uptimeSeconds ?? peer.advertisedUptimeSeconds
        )
        return max(computed, peer.advertisedScore)
    }

    private func computeNodeScore(
        metrics: PeerMetrics,
        availability: Double,
        uptimeSeconds: TimeInterval
    ) -> Double {
        let rssiScore = normalizeRssi(metrics.rssi ?? metrics.signalQuality)
        let availabilityScore = availability.clamped(to: 0.0...1.0)
        let uptimeScore = normalizeUptime(metrics.uptimeSeconds ?? uptimeSeconds)
        let batteryScore = Double((metrics.batteryPercent ?? 60).clamped(to: 0...100)) / 100.0
        let stabilityScore = (metrics.stability ?? 0.6).clamped(to: 0.0...1.0)
        let loadScore = normalizeLoad(metrics.loadPercent)

        return (rssiScore * weights.rssi) +
            (availabilityScore * weights.availability) +
            (uptimeScore * weights.uptime) +
            (batteryScore * weights.battery) +
            (stabilityScore * weights.stability) +
            (loadScore * weights.load)
    }

    private func normalizeRssi(_ value: Int?) -> Double {
        guard let value = value else { return 0.5 }
        let clamped = value.clamped(to: -100...(-20))
        return Double(clamped + 100) / 80.0
    }

    private func normalizeLoad(_ value: Int?) -> Double {
        guard let value = value else { return 0.5 }
        return 1.0 - Double(value.clamped(to: 0...100)) / 100.0
    }

    private func normalizeUptime(_ uptimeSeconds: TimeInterval) -> Double {
        guard uptimeSeconds > 0 else { return 0.0 }
        let capped = min(uptimeSeconds, config.uptimeSaturation)
        return capped / max(config.uptimeSaturation, 1.0)
    }

    private func findWorstActivePeer() -> PeerState? {
        activeConnections.keys
            .compactMap { peersById[$0] }
            .filter { $0.deviceId != selfId }
            .min(by: { computePeerScore(peer: $0) < computePeerScore(peer: $1) })
    }

    private func pruneExpiredCandidates(now: Date) {
        candidatesByHash = candidatesByHash.filter { now.timeIntervalSince($0.value.observedAt) <= config.metadataTTL }
    }

    static func hash64(_ input: String) -> UInt64 {
        let digest = SHA256.hash(data: Data(input.utf8))
        return digest.prefix(8).reduce(0) { ($0 << 8) | UInt64($1) }
    }
}

private extension Comparable {
    func clamped(to range: ClosedRange<Self>) -> Self {
        min(max(self, range.lowerBound), range.upperBound)
    }
}

