package com.offlineprotocol.mesh

import java.nio.ByteBuffer
import java.nio.ByteOrder
import java.security.MessageDigest
import java.util.UUID
import java.util.concurrent.ConcurrentHashMap
import kotlin.math.max
import kotlin.math.roundToInt

/**
 * Compact representation of cluster metadata encoded into BLE advertisement service data.
 *
 * Byte layout (big endian):
 * 0   : version (currently 1)
 * 1   : flags (bit0 = leader, bits1-3 reserved, bits4-7 = available slots)
 * 2   : member count (0-255)
 * 3   : leader score (0-255 scaled)
 * 4-7 : cluster hash (32-bit)
 * 8-11: node hash (32-bit)
 */
data class MeshAdvertisementData(
    val version: Int = 1,
    val isLeader: Boolean,
    val memberCount: Int,
    val availableSlots: Int,
    val leaderScore: Double,
    val clusterHash: Int,
    val nodeHash: Int
) {
    init {
        require(version in 0..255) { "version out of range" }
        require(memberCount in 0..255) { "memberCount out of range" }
        require(availableSlots in 0..15) { "availableSlots out of range" }
        require(leaderScore.isNaN() || (leaderScore in 0.0..1.0)) { "leaderScore must be within [0,1]" }
    }

    fun encode(): ByteArray {
        val buffer = ByteBuffer.allocate(12).order(ByteOrder.BIG_ENDIAN)
        buffer.put(version.toByte())
        val flagBits = ((availableSlots and 0x0F) shl 4) or if (isLeader) 0x01 else 0x00
        buffer.put(flagBits.toByte())
        buffer.put(memberCount.toByte())
        val score = when {
            leaderScore.isNaN() -> 0
            leaderScore <= 0.0 -> 0
            leaderScore >= 1.0 -> 255
            else -> (leaderScore * 255.0).roundToInt().coerceIn(0, 255)
        }
        buffer.put(score.toByte())
        buffer.putInt(clusterHash)
        buffer.putInt(nodeHash)
        return buffer.array()
    }

    companion object {
        fun decode(bytes: ByteArray?): MeshAdvertisementData? {
            if (bytes == null || bytes.size < 12) return null
            val buffer = ByteBuffer.wrap(bytes).order(ByteOrder.BIG_ENDIAN)
            val version = buffer.get().toInt() and 0xFF
            if (version != 1) return null
            val flags = buffer.get().toInt() and 0xFF
            val availableSlots = (flags shr 4) and 0x0F
            val isLeader = (flags and 0x01) == 0x01
            val memberCount = buffer.get().toInt() and 0xFF
            val leaderScore = (buffer.get().toInt() and 0xFF) / 255.0
            val clusterHash = buffer.int
            val nodeHash = buffer.int
            return MeshAdvertisementData(
                version = version,
                isLeader = isLeader,
                memberCount = memberCount,
                availableSlots = availableSlots,
                leaderScore = leaderScore,
                clusterHash = clusterHash,
                nodeHash = nodeHash
            )
        }
    }
}

/**
 * Coordinated cluster manager that keeps track of peers, leader election and connection budgets.
 *
 * All mutations are synchronized on [lock] to guarantee consistency across threads.
 */
class MeshController(
    private val selfId: String,
    private val config: MeshConfig = MeshConfig(),
    private val timeProvider: () -> Long = { System.currentTimeMillis() }
) {

    data class MeshConfig(
        val maxConnectionsPerDevice: Int = 3,
        val maxClusterSize: Int = 4,
        val leaderReselectionIntervalMs: Long = 30_000,
        val leaderDropScoreThreshold: Double = 0.25,
        val metadataTtlMs: Long = 60_000,
        val activePeerGraceMs: Long = 90_000
    )

    data class PeerMetrics(
        val rssi: Int? = null,
        val batteryPercent: Int? = null,
        val signalQuality: Int? = null,
        val hopCount: Int? = null,
        val stability: Double? = null
    )

    enum class MeshRole {
        LEADER,
        MEMBER,
        BRIDGE
    }

    enum class ConnectionIntent {
        INTRA_CLUSTER,
        INTER_CLUSTER,
        REJECTED
    }

    data class MeshDecision(
        val intent: ConnectionIntent,
        val reason: String,
        val evictPeerId: String? = null
    )

    data class ClusterSnapshot(
        val clusterId: String,
        val leaderId: String,
        val members: Map<String, PeerState>,
        val availableSlots: Int,
        val leaderScore: Double
    )

    data class PeerState(
        val deviceId: String,
        var role: MeshRole = MeshRole.MEMBER,
        var metrics: PeerMetrics = PeerMetrics(),
        var lastUpdated: Long = 0,
        var lastActivityAt: Long = 0
    ) {
        fun computeScore(): Double {
            val rssiScore = metrics.rssi?.let { ((it + 100).coerceIn(-100, -20) + 100) / 80.0 } ?: 0.5
            val batteryScore = (metrics.batteryPercent?.coerceIn(0, 100) ?: 60) / 100.0
            val signalScore = (metrics.signalQuality?.coerceIn(0, 100) ?: 50) / 100.0
            val stabilityScore = metrics.stability?.coerceIn(0.0, 1.0) ?: 0.5
            val hopScore = metrics.hopCount?.let { 1.0 / (1 + it) } ?: 1.0
            return (rssiScore * 0.3) +
                (batteryScore * 0.25) +
                (signalScore * 0.2) +
                (stabilityScore * 0.15) +
                (hopScore * 0.1)
        }
    }

    data class RemoteCluster(
        val metadata: MeshAdvertisementData,
        val observedAt: Long,
        val rssi: Int?
    )

    private val lock = Any()

    private val clusterId: String = generateDeterministicClusterId(selfId)
    private val selfNodeHash = hash32(selfId)
    private val members = mutableMapOf<String, PeerState>()
    private val activeConnections = mutableMapOf<String, MeshRole>()
    private val remoteClusters = ConcurrentHashMap<Int, RemoteCluster>()
    private var leaderId: String = selfId
    private var clusterVersion: Long = 0
    private var lastElectionAt: Long = 0
    private val leaderListeners = mutableSetOf<(String) -> Unit>()

    init {
        val now = timeProvider()
        members[selfId] = PeerState(
            deviceId = selfId,
            role = MeshRole.LEADER,
            metrics = PeerMetrics(),
            lastUpdated = now,
            lastActivityAt = now
        )
        leaderId = selfId
        clusterVersion = 1
        lastElectionAt = now
    }

    fun addOnLeaderChanged(listener: (String) -> Unit) {
        synchronized(lock) {
            leaderListeners.add(listener)
        }
    }

    fun removeOnLeaderChanged(listener: (String) -> Unit) {
        synchronized(lock) {
            leaderListeners.remove(listener)
        }
    }

    fun snapshot(): ClusterSnapshot {
        synchronized(lock) {
            val availableSlots = config.maxClusterSize - members.size
            val leaderScore = members[leaderId]?.computeScore() ?: 0.0
            return ClusterSnapshot(
                clusterId = clusterId,
                leaderId = leaderId,
                members = members.toMap(),
                availableSlots = max(availableSlots, 0),
                leaderScore = leaderScore
            )
        }
    }

    fun toAdvertisement(): MeshAdvertisementData {
        val snap = snapshot()
        val isLeader = snap.leaderId == selfId
        return MeshAdvertisementData(
            isLeader = isLeader,
            memberCount = snap.members.size.coerceIn(0, 255),
            availableSlots = snap.availableSlots.coerceIn(0, 15),
            leaderScore = snap.leaderScore.coerceIn(0.0, 1.0),
            clusterHash = hash32(snap.clusterId),
            nodeHash = selfNodeHash
        )
    }

    fun noteSelfMetrics(metrics: PeerMetrics) {
        synchronized(lock) {
            members[selfId]?.apply {
                this.metrics = metrics
                this.lastUpdated = timeProvider()
                this.lastActivityAt = timeProvider()
            }
            maybeElectLeader("self_metrics")
        }
    }

    fun observeAdvertisement(data: MeshAdvertisementData?, rssi: Int?) {
        if (data == null) return
        val now = timeProvider()
        remoteClusters[data.clusterHash] = RemoteCluster(data, now, rssi)
        pruneRemoteClusters(now)
    }

    fun shouldAcceptInboundConnection(remoteId: String?): MeshDecision {
        synchronized(lock) {
            val now = timeProvider()
            val connectionBudgetLeft = config.maxConnectionsPerDevice - activeConnections.size
            if (connectionBudgetLeft <= 0) {
                return MeshDecision(ConnectionIntent.REJECTED, "connection_budget_exhausted")
            }

            val clusterSlotsLeft = config.maxClusterSize - members.size
            if (clusterSlotsLeft <= 0) {
                return MeshDecision(ConnectionIntent.INTER_CLUSTER, "cluster_full")
            }

            if (remoteId == null) {
                return MeshDecision(ConnectionIntent.INTRA_CLUSTER, "unknown_peer_id")
            }

            if (!members.containsKey(remoteId)) {
                members[remoteId] = PeerState(
                    deviceId = remoteId,
                    role = MeshRole.MEMBER,
                    metrics = PeerMetrics(),
                    lastUpdated = now,
                    lastActivityAt = now
                )
            } else {
                members[remoteId]?.lastActivityAt = now
            }

            markPeerActive(remoteId)

            return MeshDecision(ConnectionIntent.INTRA_CLUSTER, "slot_available")
        }
    }

    fun registerConnection(peerId: String, role: MeshRole) {
        synchronized(lock) {
            val now = timeProvider()
            activeConnections[peerId] = role
            val state = members.getOrPut(peerId) {
                PeerState(deviceId = peerId, role = role, lastUpdated = now, lastActivityAt = now)
            }
            state.role = role
            state.lastUpdated = now
            state.lastActivityAt = now
            if (role == MeshRole.BRIDGE && !members.containsKey(peerId)) {
                members[peerId] = state
            }
            maybeElectLeader("connection_registered")
        }
    }

    fun updatePeerMetrics(peerId: String, metrics: PeerMetrics) {
        synchronized(lock) {
            val now = timeProvider()
            val state = members.getOrPut(peerId) {
                PeerState(deviceId = peerId, role = MeshRole.MEMBER, metrics = PeerMetrics(), lastUpdated = now, lastActivityAt = now)
            }
            state.metrics = metrics
            state.lastUpdated = now
            state.lastActivityAt = now
            maybeElectLeader("peer_metrics")
        }
    }

    fun registerDisconnection(peerId: String) {
        synchronized(lock) {
            activeConnections.remove(peerId)
            val removed = members.remove(peerId)
            if (removed != null) {
                clusterVersion++
            }
            if (peerId == leaderId) {
                leaderId = selfId
            }
            maybeElectLeader("disconnect")
        }
    }

    fun markPeerActive(peerId: String) {
        synchronized(lock) {
            val now = timeProvider()
            val state = members.getOrPut(peerId) {
                PeerState(deviceId = peerId, role = MeshRole.MEMBER, metrics = PeerMetrics(), lastUpdated = now, lastActivityAt = now)
            }
            state.lastActivityAt = now
            if (peerId == selfId) {
                state.role = MeshRole.LEADER.takeIf { isSelfLeader() } ?: state.role
            }
        }
    }

    fun connectionBudgetAvailable(): Boolean {
        synchronized(lock) {
            return activeConnections.size < config.maxConnectionsPerDevice
        }
    }

    fun clusterHasCapacity(): Boolean {
        synchronized(lock) {
            return members.size < config.maxClusterSize
        }
    }

    fun currentLeaderId(): String = synchronized(lock) { leaderId }

    fun isSelfLeader(): Boolean = currentLeaderId() == selfId

    fun shouldInitiateOutbound(metadata: MeshAdvertisementData?, rssi: Int?): MeshDecision {
        val now = timeProvider()
        val connectionBudget = synchronized(lock) { config.maxConnectionsPerDevice - activeConnections.size }
        val remoteData = metadata?.also {
            remoteClusters[it.clusterHash] = RemoteCluster(it, now, rssi)
        }
        val selfLeader = isSelfLeader()
        val remoteHasCapacity = remoteData?.availableSlots ?: 0 > 0

        val (intent, reason) = when {
            remoteData == null -> ConnectionIntent.INTRA_CLUSTER to "no_metadata"
            selfLeader && remoteHasCapacity -> ConnectionIntent.INTER_CLUSTER to "link_neighbor_cluster"
            !selfLeader && remoteHasCapacity && clusterHasCapacity() -> ConnectionIntent.INTRA_CLUSTER to "join_remote_cluster"
            remoteHasCapacity -> ConnectionIntent.INTER_CLUSTER to "remote_capacity_only"
            else -> ConnectionIntent.REJECTED to "remote_cluster_full"
        }

        if (intent == ConnectionIntent.REJECTED) {
            return MeshDecision(intent, reason)
        }

        if (intent == ConnectionIntent.INTRA_CLUSTER && !clusterHasCapacity()) {
            return MeshDecision(ConnectionIntent.REJECTED, "cluster_full")
        }

        var evictionTarget: String? = null
        if (connectionBudget <= 0) {
            evictionTarget = when (intent) {
                ConnectionIntent.INTER_CLUSTER -> lowestPriorityMemberForEviction(now)?.deviceId
                else -> null
            }

            if (evictionTarget == null) {
                return MeshDecision(ConnectionIntent.REJECTED, "local_capacity_full")
            }
        }

        return MeshDecision(intent, reason, evictionTarget)
    }

    private fun maybeElectLeader(trigger: String) {
        synchronized(lock) {
            val now = timeProvider()
            if (now - lastElectionAt < config.leaderReselectionIntervalMs && trigger !in setOf("disconnect", "peer_metrics")) {
                return
            }

            var bestPeer: PeerState? = null
            var bestScore = Double.NEGATIVE_INFINITY
            val iterator = members.iterator()
            while (iterator.hasNext()) {
                val entry = iterator.next()
                val state = entry.value
                if (now - state.lastUpdated > config.metadataTtlMs) {
                    iterator.remove()
                    activeConnections.remove(entry.key)
                    continue
                }
                val score = state.computeScore()
                if (score > bestScore || (score == bestScore && entry.key < (bestPeer?.deviceId ?: ""))) {
                    bestPeer = state
                    bestScore = score
                }
            }

            if (bestPeer == null) {
                leaderId = selfId
                members[selfId] = PeerState(
                    deviceId = selfId,
                    role = MeshRole.LEADER,
                    metrics = members[selfId]?.metrics ?: PeerMetrics(),
                    lastUpdated = now
                )
                clusterVersion++
                lastElectionAt = now
                notifyLeaderChanged()
                return
            }

            val previousLeader = leaderId
            leaderId = bestPeer.deviceId
            members.values.forEach { state ->
                state.role = if (state.deviceId == leaderId) MeshRole.LEADER else MeshRole.MEMBER
            }

            if (previousLeader != leaderId ||
                bestPeer.computeScore() < config.leaderDropScoreThreshold
            ) {
                clusterVersion++
                lastElectionAt = now
                notifyLeaderChanged()
            }
        }
    }

    private fun pruneRemoteClusters(now: Long) {
        val iterator = remoteClusters.entries.iterator()
        while (iterator.hasNext()) {
            val entry = iterator.next()
            if (now - entry.value.observedAt > config.metadataTtlMs) {
                iterator.remove()
            }
        }
    }

    private fun notifyLeaderChanged() {
        val listeners = synchronized(lock) { leaderListeners.toList() }
        val leader = currentLeaderId()
        listeners.forEach { listener ->
            try {
                listener(leader)
            } catch (_: Exception) {
                // Listener errors should not crash the controller.
            }
        }
    }

    private fun lowestPriorityMemberForEviction(now: Long): PeerState? = synchronized(lock) {
        members.values
            .filter { state ->
                state.deviceId != selfId &&
                activeConnections[state.deviceId] != MeshRole.BRIDGE &&
                (now - state.lastActivityAt) > config.activePeerGraceMs
            }
            .minByOrNull { it.computeScore() }
    }

    companion object {
        private fun generateDeterministicClusterId(selfId: String): String {
            return UUID.nameUUIDFromBytes(selfId.toByteArray()).toString()
        }

        private fun hash32(input: String): Int {
            val digest = MessageDigest.getInstance("SHA-256")
            val bytes = digest.digest(input.toByteArray())
            var hash = 0
            for (i in 0 until 4) {
                hash = (hash shl 8) or (bytes[i].toInt() and 0xFF)
            }
            return hash
        }
    }
}


