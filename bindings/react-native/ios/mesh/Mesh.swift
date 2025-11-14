//
//  Mesh.swift
//  OfflineProtocol
//

import Foundation

// MARK: - Advertisement Payload

/// Shared advertisement payload between iOS and Android mesh implementations.
/// Layout mirrors `MeshAdvertisementData` on Android.
struct MeshAdvertisementData {
    let version: UInt8
    let isLeader: Bool
    let memberCount: UInt8
    let availableSlots: UInt8
    let leaderScore: Double
    let clusterHash: UInt32
    let nodeHash: UInt32

    init(
        version: UInt8 = 1,
        isLeader: Bool,
        memberCount: UInt8,
        availableSlots: UInt8,
        leaderScore: Double,
        clusterHash: UInt32,
        nodeHash: UInt32
    ) {
        precondition(leaderScore.isNaN || (0.0...1.0).contains(leaderScore), "leaderScore must be within [0,1]")
        precondition(availableSlots <= 0x0F, "availableSlots exceeds 4-bit field")
        self.version = version
        self.isLeader = isLeader
        self.memberCount = memberCount
        self.availableSlots = availableSlots
        self.leaderScore = leaderScore
        self.clusterHash = clusterHash
        self.nodeHash = nodeHash
    }

    func encode() -> Data {
        var buffer = Data(capacity: 12)
        buffer.append(version)
        let flags: UInt8 = (availableSlots << 4) | (isLeader ? 0x01 : 0x00)
        buffer.append(flags)
        buffer.append(memberCount)
        let scoreByte: UInt8
        if leaderScore.isNaN || leaderScore <= 0 {
            scoreByte = 0
        } else if leaderScore >= 1.0 {
            scoreByte = 255
        } else {
            scoreByte = UInt8(min(255, max(0, Int((leaderScore * 255.0).rounded()))))
        }
        buffer.append(scoreByte)
        buffer.append(contentsOf: clusterHash.bigEndianBytes)
        buffer.append(contentsOf: nodeHash.bigEndianBytes)
        return buffer
    }

    static func decode(_ data: Data?) -> MeshAdvertisementData? {
        guard let data = data, data.count >= 12 else { return nil }
        var iterator = data.makeIterator()
        guard let version = iterator.next() else { return nil }
        guard version == 1 else { return nil }
        guard let flags = iterator.next(),
              let memberCount = iterator.next(),
              let score = iterator.next()
        else { return nil }

        let availableSlots = (flags >> 4) & 0x0F
        let isLeader = (flags & 0x01) == 0x01

        let clusterHash = UInt32(bigEndianBytes: Array(data[4...7]))
        let nodeHash = UInt32(bigEndianBytes: Array(data[8...11]))
        let leaderScore = Double(score) / 255.0

        return MeshAdvertisementData(
            version: version,
            isLeader: isLeader,
            memberCount: memberCount,
            availableSlots: availableSlots,
            leaderScore: leaderScore,
            clusterHash: clusterHash,
            nodeHash: nodeHash
        )
    }
}

private extension UInt32 {
    init(bigEndianBytes bytes: [UInt8]) {
        precondition(bytes.count == 4, "UInt32 requires 4 bytes")
        self = bytes.reduce(UInt32(0)) { acc, byte in
            (acc << 8) | UInt32(byte)
        }
    }

    var bigEndianBytes: [UInt8] {
        [
            UInt8((self >> 24) & 0xFF),
            UInt8((self >> 16) & 0xFF),
            UInt8((self >> 8) & 0xFF),
            UInt8(self & 0xFF)
        ]
    }
}

// MARK: - Mesh Controller

/// Mirrors the Android mesh controller to ensure deterministic cluster behaviour.
final class MeshController {
    struct MeshConfig {
        let maxConnectionsPerDevice: Int
        let maxClusterSize: Int
        let leaderReselectionInterval: TimeInterval
        let leaderDropScoreThreshold: Double
        let metadataTTL: TimeInterval
        let activePeerGrace: TimeInterval

        init(
            maxConnectionsPerDevice: Int = 3,
            maxClusterSize: Int = 4,
            leaderReselectionInterval: TimeInterval = 30.0,
            leaderDropScoreThreshold: Double = 0.25,
            metadataTTL: TimeInterval = 60.0,
            activePeerGrace: TimeInterval = 90.0
        ) {
            self.maxConnectionsPerDevice = maxConnectionsPerDevice
            self.maxClusterSize = maxClusterSize
            self.leaderReselectionInterval = leaderReselectionInterval
            self.leaderDropScoreThreshold = leaderDropScoreThreshold
            self.metadataTTL = metadataTTL
            self.activePeerGrace = activePeerGrace
        }
    }

    struct PeerMetrics {
        var rssi: Int?
        var batteryPercent: Int?
        var signalQuality: Int?
        var hopCount: Int?
        var stability: Double?
    }

    enum MeshRole {
        case leader
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

    struct PeerState {
        let deviceId: String
        var role: MeshRole
        var metrics: PeerMetrics
        var lastUpdated: Date
        var lastActivity: Date

        func score() -> Double {
            let rssiScore = metrics.rssi
                .map { Double(($0 + 100).clamped(to: -100...(-20)) + 100) / 80.0 }
                ?? 0.5
            let batteryScore = Double((metrics.batteryPercent?.clamped(to: 0...100) ?? 60)) / 100.0
            let signalScore = Double((metrics.signalQuality?.clamped(to: 0...100) ?? 50)) / 100.0
            let stabilityScore = (metrics.stability ?? 0.5).clamped(to: 0.0...1.0)
            let hopScore = metrics.hopCount.map { 1.0 / Double(1 + $0) } ?? 1.0
            return (rssiScore * 0.3) +
                (batteryScore * 0.25) +
                (signalScore * 0.2) +
                (stabilityScore * 0.15) +
                (hopScore * 0.1)
        }
    }

    struct ClusterSnapshot {
        let clusterId: String
        let leaderId: String
        let members: [String: PeerState]
        let availableSlots: Int
        let leaderScore: Double
    }

    struct RemoteCluster {
        let metadata: MeshAdvertisementData
        let observedAt: Date
        let rssi: Int?
    }

    typealias LeaderListener = (String) -> Void

    private let selfId: String
    private let config: MeshConfig
    private let queue = DispatchQueue(label: "com.offlineprotocol.mesh.controller", attributes: .concurrent)

    private var clusterId: String
    private var selfNodeHash: UInt32
    private var leaderId: String
    private var clusterVersion: Int64 = 1
    private var lastElection: Date
    private var members: [String: PeerState]
    private var activeConnections: [String: MeshRole] = [:]
    private var remoteClusters: [Int: RemoteCluster] = [:]
    private var leaderListeners: [UUID: LeaderListener] = [:]

    init(selfId: String, config: MeshConfig = MeshConfig()) {
        self.selfId = selfId
        self.config = config
        self.clusterId = MeshController.makeClusterId(selfId: selfId)
        self.selfNodeHash = MeshController.hash32(selfId)
        let now = Date()
        self.lastElection = now
        self.leaderId = selfId
        let initialState = PeerState(
            deviceId: selfId,
            role: .leader,
            metrics: PeerMetrics(),
            lastUpdated: now,
            lastActivity: now
        )
        self.members = [selfId: initialState]
    }

    @discardableResult
    func addLeaderListener(_ listener: @escaping LeaderListener) -> UUID {
        let token = UUID()
        queue.async(flags: .barrier) {
            self.leaderListeners[token] = listener
        }
        return token
    }

    func removeLeaderListener(_ token: UUID) {
        queue.async(flags: .barrier) {
            self.leaderListeners.removeValue(forKey: token)
        }
    }

    func snapshot() -> ClusterSnapshot {
        queue.sync {
            let availableSlots = max(config.maxClusterSize - members.count, 0)
            let leaderScore = members[leaderId]?.score() ?? 0.0
            return ClusterSnapshot(
                clusterId: clusterId,
                leaderId: leaderId,
                members: members,
                availableSlots: availableSlots,
                leaderScore: leaderScore
            )
        }
    }

    func advertisement() -> MeshAdvertisementData {
        let snap = snapshot()
        return MeshAdvertisementData(
            isLeader: snap.leaderId == selfId,
            memberCount: UInt8(truncatingIfNeeded: snap.members.count),
            availableSlots: UInt8(truncatingIfNeeded: snap.availableSlots),
            leaderScore: snap.leaderScore.clamped(to: 0.0...1.0),
            clusterHash: MeshController.hash32(snap.clusterId),
            nodeHash: selfNodeHash
        )
    }

    func updateSelfMetrics(_ metrics: PeerMetrics) {
        queue.async(flags: .barrier) {
            let now = Date()
            var state = self.members[self.selfId] ?? PeerState(
                deviceId: self.selfId,
                role: .leader,
                metrics: metrics,
                lastUpdated: now,
                lastActivity: now
            )
            state.metrics = metrics
            state.lastUpdated = now
            state.lastActivity = now
            self.members[self.selfId] = state
            self.maybeElectLeader(trigger: "self_metrics")
        }
    }

    func observeAdvertisement(_ data: MeshAdvertisementData?, rssi: Int?) {
        guard let data = data else { return }
        queue.async(flags: .barrier) {
            let now = Date()
            self.remoteClusters[Int(bitPattern: UInt(bitPattern: data.clusterHash))] = RemoteCluster(metadata: data, observedAt: now, rssi: rssi)
            self.pruneRemoteClusters(now: now)
        }
    }

    func shouldAcceptInboundConnection(remoteId: String?) -> MeshDecision {
        queue.sync {
            let now = Date()
            if activeConnections.count >= config.maxConnectionsPerDevice {
                return MeshDecision(intent: .rejected, reason: "connection_budget_exhausted")
            }
            if members.count >= config.maxClusterSize {
                return MeshDecision(intent: .interCluster, reason: "cluster_full")
            }
            if let remoteId = remoteId, members[remoteId] == nil {
                var state = PeerState(
                    deviceId: remoteId,
                    role: .member,
                    metrics: PeerMetrics(),
                    lastUpdated: now,
                    lastActivity: now
                )
                members[remoteId] = state
            } else if let remoteId = remoteId {
                var state = members[remoteId]
                state?.lastActivity = now
                if let state = state {
                    members[remoteId] = state
                }
            }
            return MeshDecision(intent: .intraCluster, reason: "slot_available")
        }
    }

    func registerConnection(peerId: String, role: MeshRole) {
        queue.async(flags: .barrier) {
            let now = Date()
            self.activeConnections[peerId] = role
            var state = self.members[peerId] ?? PeerState(
                deviceId: peerId,
                role: role,
                metrics: PeerMetrics(),
                lastUpdated: now,
                lastActivity: now
            )
            state.role = role
            state.lastUpdated = now
            state.lastActivity = now
            self.members[peerId] = state
            self.maybeElectLeader(trigger: "connection_registered")
        }
    }

    func updatePeerMetrics(peerId: String, metrics: PeerMetrics) {
        queue.async(flags: .barrier) {
            let now = Date()
            var state = self.members[peerId] ?? PeerState(
                deviceId: peerId,
                role: .member,
                metrics: metrics,
                lastUpdated: now,
                lastActivity: now
            )
            state.metrics = metrics
            state.lastUpdated = now
            state.lastActivity = now
            self.members[peerId] = state
            self.maybeElectLeader(trigger: "peer_metrics")
        }
    }

    func registerDisconnection(peerId: String) {
        queue.async(flags: .barrier) {
            self.activeConnections.removeValue(forKey: peerId)
            self.members.removeValue(forKey: peerId)
            if self.leaderId == peerId {
                self.leaderId = self.selfId
            }
            self.maybeElectLeader(trigger: "disconnect")
        }
    }

    func markPeerActive(_ peerId: String) {
        queue.async(flags: .barrier) {
            let now = Date()
            var state = self.members[peerId] ?? PeerState(
                deviceId: peerId,
                role: .member,
                metrics: PeerMetrics(),
                lastUpdated: now,
                lastActivity: now
            )
            state.lastActivity = now
            if state.deviceId == self.selfId {
                state.role = .leader
            }
            self.members[peerId] = state
        }
    }

    func connectionBudgetAvailable() -> Bool {
        queue.sync { activeConnections.count < config.maxConnectionsPerDevice }
    }

    func clusterHasCapacity() -> Bool {
        queue.sync { members.count < config.maxClusterSize }
    }

    func currentLeaderId() -> String {
        queue.sync { leaderId }
    }

    func isSelfLeader() -> Bool {
        currentLeaderId() == selfId
    }

    func shouldInitiateOutbound(metadata: MeshAdvertisementData?, rssi: Int?) -> MeshDecision {
        queue.sync {
            let now = Date()
            let connectionBudget = config.maxConnectionsPerDevice - activeConnections.count
            if let metadata = metadata {
                remoteClusters[Int(bitPattern: UInt(bitPattern: metadata.clusterHash))] = RemoteCluster(metadata: metadata, observedAt: now, rssi: rssi)
            }

            let selfLeader = leaderId == selfId
            let remoteHasCapacity = metadata?.availableSlots ?? 0 > 0

            let (intent, reason): (ConnectionIntent, String) = {
                guard let metadata = metadata else {
                    return (.intraCluster, "no_metadata")
                }

                if selfLeader && remoteHasCapacity {
                    return (.interCluster, "link_neighbor_cluster")
                } else if !selfLeader && remoteHasCapacity && members.count < config.maxClusterSize {
                    return (.intraCluster, "join_remote_cluster")
                } else if remoteHasCapacity {
                    return (.interCluster, "remote_capacity_only")
                } else {
                    return (.rejected, "remote_cluster_full")
                }
            }()

            if intent == .rejected {
                return MeshDecision(intent: intent, reason: reason)
            }

            if intent == .intraCluster && members.count >= config.maxClusterSize {
                return MeshDecision(intent: .rejected, reason: "cluster_full")
            }

            var evictionTarget: String?
            if connectionBudget <= 0 {
                if intent == .interCluster {
                    evictionTarget = lowestPriorityMemberForEviction(now: now)?.deviceId
                }

                if evictionTarget == nil {
                    return MeshDecision(intent: .rejected, reason: "local_capacity_full")
                }
            }

            return MeshDecision(intent: intent, reason: reason, evictPeerId: evictionTarget)
        }
    }

    private func lowestPriorityMemberForEviction(now: Date) -> PeerState? {
        members.values
            .filter { state in
                state.deviceId != selfId &&
                activeConnections[state.deviceId] != .bridge &&
                now.timeIntervalSince(state.lastActivity) > config.activePeerGrace
            }
            .min(by: { $0.score() < $1.score() })
    }

    private func maybeElectLeader(trigger: String) {
        queue.async(flags: .barrier) {
            let now = Date()
            if now.timeIntervalSince(self.lastElection) < self.config.leaderReselectionInterval &&
                trigger != "disconnect" && trigger != "peer_metrics" {
                return
            }

            var bestPeer: PeerState?
            var bestScore = -Double.infinity
            var mutableMembers = self.members

            for (deviceId, state) in mutableMembers {
                if now.timeIntervalSince(state.lastUpdated) > self.config.metadataTTL {
                    mutableMembers.removeValue(forKey: deviceId)
                    self.activeConnections.removeValue(forKey: deviceId)
                    continue
                }
                let score = state.score()
                if score > bestScore || (score == bestScore && deviceId < (bestPeer?.deviceId ?? deviceId)) {
                    bestPeer = state
                    bestScore = score
                }
            }

            self.members = mutableMembers

            guard let chosen = bestPeer else {
                self.leaderId = self.selfId
                self.members[self.selfId] = PeerState(
                    deviceId: self.selfId,
                    role: .leader,
                    metrics: self.members[self.selfId]?.metrics ?? PeerMetrics(),
                    lastUpdated: now,
                    lastActivity: now
                )
                self.clusterVersion += 1
                self.lastElection = now
                self.notifyLeaderChanged()
                return
            }

            let previousLeader = self.leaderId
            self.leaderId = chosen.deviceId
            for (deviceId, var state) in self.members {
                state.role = deviceId == self.leaderId ? .leader : .member
                self.members[deviceId] = state
            }

            if previousLeader != self.leaderId || chosen.score() < self.config.leaderDropScoreThreshold {
                self.clusterVersion += 1
                self.lastElection = now
                self.notifyLeaderChanged()
            }
        }
    }

    private func pruneRemoteClusters(now: Date) {
        remoteClusters = remoteClusters.filter { _, value in
            now.timeIntervalSince(value.observedAt) <= config.metadataTTL
        }
    }

    private func notifyLeaderChanged() {
        let listeners = queue.sync { leaderListeners }
        let leader = currentLeaderId()
        listeners.values.forEach { listener in
            listener(leader)
        }
    }

    private static func makeClusterId(selfId: String) -> String {
        let hash = hash32(selfId)
        return String(format: "cluster-%08x", hash)
    }

    private static func hash32(_ input: String) -> UInt32 {
        var hash: UInt32 = 0x811C9DC5
        for byte in input.utf8 {
            hash ^= UInt32(byte)
            hash = hash &* 0x01000193
        }
        return hash
    }
}

private extension Comparable {
    func clamped(to range: ClosedRange<Self>) -> Self {
        min(max(self, range.lowerBound), range.upperBound)
    }
}


