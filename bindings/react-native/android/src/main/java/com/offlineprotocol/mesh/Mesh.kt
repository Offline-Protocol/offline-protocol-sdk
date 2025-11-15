package com.offlineprotocol.mesh

import java.nio.ByteBuffer
import java.nio.ByteOrder
import java.security.MessageDigest
import java.util.UUID
import java.util.concurrent.ConcurrentHashMap
import kotlin.math.abs
import kotlin.math.max
import kotlin.math.roundToInt

/**
 * Compact representation of node metadata encoded into BLE advertisement service data.
 *
 * Byte layout (big endian):
 * 0   : version (currently 2)
 * 1   : degree (hi 4 bits) | freeSlotEstimate (lo 4 bits)
 * 2   : nodeScore (0-255 scaled)
 * 3   : batteryPercent (0-100, 0xFF = unknown)
 * 4   : loadPercent (0-100, 0xFF = unknown)
 * 5   : rssiToYou encoded (value + 127, 0xFF = unknown)
 * 6-9 : uptimeSeconds (uint32)
 * 10-17: nodeIdHash (uint64)
 * 18  : feature flags
 */
data class MeshAdvertisementData(
    val version: Int = CURRENT_VERSION,
    val degree: Int,
    val freeSlotEstimate: Int,
    val nodeScore: Double,
    val uptimeSeconds: Long,
    val batteryPercent: Int?,
    val loadPercent: Int?,
    val rssiToYou: Int?,
    val nodeIdHash: Long,
    val featureFlags: Int = FEATURE_SCORE_BASED
) {
    init {
        require(version == CURRENT_VERSION) { "Unsupported advertisement version: $version" }
        require(degree in 0..15) { "degree out of range" }
        require(freeSlotEstimate in 0..15) { "freeSlotEstimate out of range" }
        require(nodeScore.isNaN() || nodeScore in 0.0..1.0) { "nodeScore must be within [0,1]" }
        require(uptimeSeconds >= 0) { "uptimeSeconds must be non-negative" }
        batteryPercent?.let { require(it in 0..100) { "batteryPercent out of range" } }
        loadPercent?.let { require(it in 0..100) { "loadPercent out of range" } }
        rssiToYou?.let { require(it in -127..127) { "rssiToYou out of range" } }
        require(featureFlags in 0..0xFF) { "featureFlags out of range" }
    }

    fun encode(): ByteArray {
        val buffer = ByteBuffer.allocate(BYTE_LENGTH).order(ByteOrder.BIG_ENDIAN)
        buffer.put(version.toByte())
        val slotByte = ((degree.coerceIn(0, 15) and 0x0F) shl 4) or (freeSlotEstimate.coerceIn(0, 15) and 0x0F)
        buffer.put(slotByte.toByte())
        val scoreByte = when {
            nodeScore.isNaN() -> 0
            nodeScore <= 0.0 -> 0
            nodeScore >= 1.0 -> 255
            else -> (nodeScore * 255.0).roundToInt().coerceIn(0, 255)
        }
        buffer.put(scoreByte.toByte())
        buffer.put(encodeOptionalPercent(batteryPercent).toByte())
        buffer.put(encodeOptionalPercent(loadPercent).toByte())
        buffer.put(encodeOptionalRssi(rssiToYou).toByte())
        val uptime = uptimeSeconds.coerceIn(0, 0xFFFF_FFFFL).toInt()
        buffer.putInt(uptime)
        buffer.putLong(nodeIdHash)
        buffer.put(featureFlags.coerceIn(0, 0xFF).toByte())
        return buffer.array()
    }

    companion object {
        const val CURRENT_VERSION = 2
        const val FEATURE_SCORE_BASED = 0x01
        private const val BYTE_LENGTH = 19

        fun decode(bytes: ByteArray?): MeshAdvertisementData? {
            if (bytes == null || bytes.size < BYTE_LENGTH) return null
            val buffer = ByteBuffer.wrap(bytes).order(ByteOrder.BIG_ENDIAN)
            val version = buffer.get().toInt() and 0xFF
            if (version != CURRENT_VERSION) return null
            val slots = buffer.get().toInt() and 0xFF
            val degree = (slots shr 4) and 0x0F
            val freeSlotEstimate = slots and 0x0F
            val nodeScore = (buffer.get().toInt() and 0xFF) / 255.0
            val batteryPercent = decodeOptionalPercent(buffer.get().toInt() and 0xFF)
            val loadPercent = decodeOptionalPercent(buffer.get().toInt() and 0xFF)
            val rssiToYou = decodeOptionalRssi(buffer.get().toInt() and 0xFF)
            val uptimeSeconds = buffer.int.toLong() and 0xFFFF_FFFFL
            val nodeIdHash = buffer.long
            val featureFlags = buffer.get().toInt() and 0xFF
            return MeshAdvertisementData(
                version = version,
                degree = degree,
                freeSlotEstimate = freeSlotEstimate,
                nodeScore = nodeScore,
                uptimeSeconds = uptimeSeconds,
                batteryPercent = batteryPercent,
                loadPercent = loadPercent,
                rssiToYou = rssiToYou,
                nodeIdHash = nodeIdHash,
                featureFlags = featureFlags
            )
        }

        private fun encodeOptionalPercent(value: Int?): Int {
            return value?.coerceIn(0, 100) ?: 0xFF
        }

        private fun encodeOptionalRssi(value: Int?): Int {
            return value?.coerceIn(-127, 127)?.let { it + 127 } ?: 0xFF
        }

        private fun decodeOptionalPercent(value: Int): Int? {
            return if (value in 0..100) value else null
        }

        private fun decodeOptionalRssi(value: Int): Int? {
            return if (value in 0..254) value - 127 else null
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
        val minConnections: Int = 1,
        val maxConnections: Int = 4,
        val metadataTtlMs: Long = 120_000,
        val rebalanceIntervalMs: Long = 15_000,
        val connectionCooldownMs: Long = 7_500,
        val scoreWeights: ScoreWeights = ScoreWeights(),
        val uptimeSaturationSeconds: Long = 3_600,
        val scoreHysteresis: Double = 0.05,
        val scoreEquivalenceEpsilon: Double = 0.02,
        val bridgeFavor: Double = 0.1
    )

    data class ScoreWeights(
        val rssi: Double = 0.35,
        val availability: Double = 0.2,
        val uptime: Double = 0.15,
        val battery: Double = 0.15,
        val stability: Double = 0.1,
        val load: Double = 0.05
    ) {
        fun normalized(): ScoreWeights {
            val sum = rssi + availability + uptime + battery + stability + load
            if (sum == 0.0) return this
            return ScoreWeights(
                rssi / sum,
                availability / sum,
                uptime / sum,
                battery / sum,
                stability / sum,
                load / sum
            )
        }
    }

    data class PeerMetrics(
        val rssi: Int? = null,
        val batteryPercent: Int? = null,
        val signalQuality: Int? = null,
        val stability: Double? = null,
        val uptimeSeconds: Long? = null,
        val loadPercent: Int? = null
    )

    enum class MeshRole {
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

    data class RebalanceDirective(
        val decision: MeshDecision,
        val candidate: MeshAdvertisementData
    )

    private data class PeerState(
        var deviceId: String,
        var nodeHash: Long?,
        var role: MeshRole = MeshRole.MEMBER,
        var metrics: PeerMetrics = PeerMetrics(),
        var lastUpdated: Long = 0,
        var lastActivityAt: Long = 0,
        var advertisedDegree: Int = 0,
        var advertisedFreeSlots: Int = 0,
        var advertisedScore: Double = 0.0,
        var advertisedUptimeSeconds: Long = 0,
        var advertisedBatteryPercent: Int? = null,
        var advertisedLoadPercent: Int? = null,
        var advertisedRssiToUs: Int? = null,
        var observedRssi: Int? = null,
        var lastConnectionAttemptAt: Long = 0
    )

    private data class RemoteCandidate(
        val nodeHash: Long,
        var metadata: MeshAdvertisementData,
        var observedAt: Long,
        var rssi: Int?
    )

    private val lock = Any()
    private val peersById = mutableMapOf<String, PeerState>()
    private val peersByHash = mutableMapOf<Long, PeerState>()
    private val activeConnections = mutableMapOf<String, MeshRole>()
    private val candidatesByHash = ConcurrentHashMap<Long, RemoteCandidate>()
    private val recentEvictions = mutableMapOf<String, Long>()
    private var lastRebalanceAt: Long = 0
    private val startTimestamp = timeProvider()
    private var selfMetrics: PeerMetrics = PeerMetrics()
    private val normalizedWeights = config.scoreWeights.normalized()

    init {
        val now = startTimestamp
        val selfState = PeerState(
            deviceId = selfId,
            nodeHash = hash64(selfId),
            role = MeshRole.MEMBER,
            metrics = PeerMetrics(),
            lastUpdated = now,
            lastActivityAt = now
        )
        peersById[selfId] = selfState
        selfState.nodeHash?.let { peersByHash[it] = selfState }
    }

    fun updateSelfMetrics(metrics: PeerMetrics) {
        synchronized(lock) {
            val now = timeProvider()
            selfMetrics = metrics
            val state = peersById.getOrPut(selfId) {
                PeerState(
                    deviceId = selfId,
                    nodeHash = hash64(selfId),
                    role = MeshRole.MEMBER,
                    metrics = metrics,
                    lastUpdated = now,
                    lastActivityAt = now
                )
            }
            state.metrics = metrics
            state.lastUpdated = now
            state.lastActivityAt = now
            state.nodeHash = state.nodeHash ?: hash64(selfId)
            state.nodeHash?.let { peersByHash[it] = state }
        }
    }

    fun noteSelfMetrics(metrics: PeerMetrics) = updateSelfMetrics(metrics)

    fun toAdvertisement(): MeshAdvertisementData {
        val now = timeProvider()
        val uptimeSeconds = ((now - startTimestamp) / 1000).coerceAtLeast(0)
        val degree = synchronized(lock) { activeConnections.size.coerceAtLeast(0) }
        val freeSlots = (config.maxConnections - degree).coerceAtLeast(0)
        val availability = availabilityFactor(degree, freeSlots)
        val score = computeNodeScore(selfMetrics, availability, uptimeSeconds)
        val battery = selfMetrics.batteryPercent
        val load = selfMetrics.loadPercent
        val rssi = selfMetrics.rssi
        val hash = synchronized(lock) { peersById[selfId]?.nodeHash ?: hash64(selfId) }
        return MeshAdvertisementData(
            degree = degree.coerceIn(0, 15),
            freeSlotEstimate = freeSlots.coerceIn(0, 15),
            nodeScore = score,
            uptimeSeconds = uptimeSeconds.toLong(),
            batteryPercent = battery,
            loadPercent = load,
            rssiToYou = rssi,
            nodeIdHash = hash,
            featureFlags = MeshAdvertisementData.FEATURE_SCORE_BASED
        )
    }

    fun observeAdvertisement(data: MeshAdvertisementData?, rssi: Int?) {
        if (data == null) return
        val now = timeProvider()
        synchronized(lock) {
            val nodeHash = data.nodeIdHash
            val state = peersByHash[nodeHash] ?: PeerState(
                deviceId = "hash-${nodeHash.toString(16)}",
                nodeHash = nodeHash,
                metrics = PeerMetrics(),
                lastUpdated = now,
                lastActivityAt = now
            ).also {
                peersByHash[nodeHash] = it
            }

            state.advertisedDegree = data.degree
            state.advertisedFreeSlots = data.freeSlotEstimate
            state.advertisedScore = if (data.nodeScore.isNaN()) 0.0 else data.nodeScore.coerceIn(0.0, 1.0)
            state.advertisedUptimeSeconds = data.uptimeSeconds
            state.advertisedBatteryPercent = data.batteryPercent
            state.advertisedLoadPercent = data.loadPercent
            state.advertisedRssiToUs = data.rssiToYou
            state.observedRssi = rssi
            state.lastUpdated = now
            state.lastActivityAt = now
            candidatesByHash[nodeHash] = RemoteCandidate(nodeHash, data, now, rssi)
        }
        pruneExpiredCandidates(now)
    }

    fun shouldAcceptInboundConnection(
        remoteId: String?,
        metadata: MeshAdvertisementData?,
        rssi: Int?
    ): MeshDecision {
        metadata?.let { observeAdvertisement(it, rssi) }
        val now = timeProvider()
        synchronized(lock) {
            val degree = activeConnections.size
            val freeSlots = config.maxConnections - degree
            if (freeSlots > 0) {
                return MeshDecision(ConnectionIntent.INTRA_CLUSTER, "capacity_available")
            }

            val metadataSnapshot = metadata
            if (metadataSnapshot != null) {
                val candidateScore = computeCandidateScore(metadataSnapshot, rssi)
                val worstPeer = findWorstActivePeer(now)
                if (worstPeer != null && degree - 1 >= config.minConnections) {
                    val worstScore = computePeerScore(worstPeer, now)
                    val reason = evaluateSwapCandidate(
                        candidate = metadataSnapshot,
                        candidateScore = candidateScore,
                        worstPeer = worstPeer,
                        worstScore = worstScore,
                        selfDegree = degree
                    )
                    if (reason != null) {
                        return MeshDecision(ConnectionIntent.INTRA_CLUSTER, reason, worstPeer.deviceId)
                    }
                }
            }

            return if (degree < config.minConnections) {
                MeshDecision(ConnectionIntent.INTRA_CLUSTER, "protect_min_degree")
            } else {
                MeshDecision(ConnectionIntent.REJECTED, "local_links_preferred")
            }
        }
    }

    fun shouldAcceptInboundConnection(remoteId: String?): MeshDecision =
        shouldAcceptInboundConnection(remoteId, null, null)

    fun shouldInitiateOutbound(metadata: MeshAdvertisementData?, rssi: Int?): MeshDecision {
        if (metadata == null) return MeshDecision(ConnectionIntent.REJECTED, "no_metadata")
        observeAdvertisement(metadata, rssi)
        val now = timeProvider()
        synchronized(lock) {
            val degree = activeConnections.size
            val freeSlots = config.maxConnections - degree
            if (freeSlots > 0) {
                return MeshDecision(ConnectionIntent.INTRA_CLUSTER, "capacity_available")
            }

            val candidateScore = computeCandidateScore(metadata, rssi)
            val worstPeer = findWorstActivePeer(now) ?: return MeshDecision(ConnectionIntent.REJECTED, "no_active_links")
            val worstScore = computePeerScore(worstPeer, now)

            val reason = if (degree - 1 >= config.minConnections) {
                evaluateSwapCandidate(
                    candidate = metadata,
                    candidateScore = candidateScore,
                    worstPeer = worstPeer,
                    worstScore = worstScore,
                    selfDegree = degree
                )
            } else {
                null
            }

            return if (reason != null) {
                val intent = if (metadata.freeSlotEstimate <= 0) {
                    ConnectionIntent.INTER_CLUSTER
                } else {
                    ConnectionIntent.INTRA_CLUSTER
                }
                MeshDecision(intent, reason, worstPeer.deviceId)
            } else {
                MeshDecision(ConnectionIntent.REJECTED, "local_links_preferred")
            }
        }
    }

    fun registerConnection(peerId: String, role: MeshRole) {
        val now = timeProvider()
        synchronized(lock) {
            val state = peersById.getOrPut(peerId) {
                PeerState(
                    deviceId = peerId,
                    nodeHash = null,
                    role = role,
                    metrics = PeerMetrics(),
                    lastUpdated = now,
                    lastActivityAt = now
                )
            }
            state.role = role
            state.lastUpdated = now
            state.lastActivityAt = now
            activeConnections[peerId] = role
            state.nodeHash?.let { peersByHash[it] = state }
        }
    }

    fun updatePeerMetrics(peerId: String, metrics: PeerMetrics) {
        val now = timeProvider()
        synchronized(lock) {
            val state = peersById.getOrPut(peerId) {
                PeerState(
                    deviceId = peerId,
                    nodeHash = null,
                    metrics = metrics,
                    lastUpdated = now,
                    lastActivityAt = now
                )
            }
            state.metrics = metrics
            state.lastUpdated = now
            state.lastActivityAt = now
        }
    }

    fun registerDisconnection(peerId: String) {
        val now = timeProvider()
        synchronized(lock) {
            activeConnections.remove(peerId)
            recentEvictions[peerId] = now
            peersById[peerId]?.let { state ->
                state.lastActivityAt = now
            }
        }
    }

    fun markPeerActive(peerId: String) {
        val now = timeProvider()
        synchronized(lock) {
            val state = peersById.getOrPut(peerId) {
                PeerState(
                    deviceId = peerId,
                    nodeHash = null,
                    metrics = PeerMetrics(),
                    lastUpdated = now,
                    lastActivityAt = now
                )
            }
            state.lastActivityAt = now
        }
    }

    fun connectionBudgetAvailable(): Boolean = synchronized(lock) {
        activeConnections.size < config.maxConnections
    }

    fun clusterHasCapacity(): Boolean = true

    fun evaluateRebalance(): RebalanceDirective? {
        val now = timeProvider()
        synchronized(lock) {
            if (now - lastRebalanceAt < config.rebalanceIntervalMs) return null
            pruneExpiredCandidates(now)
            val degree = activeConnections.size
            val freeSlots = config.maxConnections - degree
            val bestCandidate = candidatesByHash.values
                .filter { now - it.observedAt <= config.metadataTtlMs }
                .maxByOrNull { computeCandidateScore(it.metadata, it.rssi) }
                ?: run {
                    lastRebalanceAt = now
                    return null
                }

            val candidateScore = computeCandidateScore(bestCandidate.metadata, bestCandidate.rssi)
            val decision = if (freeSlots > 0) {
                MeshDecision(ConnectionIntent.INTRA_CLUSTER, "rebalance_connect")
            } else {
                val worstPeer = findWorstActivePeer(now)
                if (worstPeer == null || degree - 1 < config.minConnections) {
                    MeshDecision(ConnectionIntent.REJECTED, "rebalance_no_capacity")
                } else {
                    val candidateScore = computeCandidateScore(bestCandidate.metadata, bestCandidate.rssi)
                    val worstScore = computePeerScore(worstPeer, now)
                    val reason = evaluateSwapCandidate(
                        candidate = bestCandidate.metadata,
                        candidateScore = candidateScore,
                        worstPeer = worstPeer,
                        worstScore = worstScore,
                        selfDegree = degree
                    )
                    if (reason != null) {
                        val intent = if (bestCandidate.metadata.freeSlotEstimate <= 0) {
                            ConnectionIntent.INTER_CLUSTER
                        } else {
                            ConnectionIntent.INTRA_CLUSTER
                        }
                        val mappedReason = when (reason) {
                            "swap_low_score_peer" -> "rebalance_swap"
                            "swap_bridge_capacity" -> "rebalance_bridge"
                            else -> "rebalance_equivalent"
                        }
                        MeshDecision(intent, mappedReason, worstPeer.deviceId)
                    } else {
                        MeshDecision(ConnectionIntent.REJECTED, "rebalance_local_preferred")
                    }
                }
            }

            lastRebalanceAt = now
            return decision.takeIf { it.intent != ConnectionIntent.REJECTED }?.let {
                RebalanceDirective(it, bestCandidate.metadata)
            }
        }
    }

    private fun findWorstActivePeer(now: Long): PeerState? {
        return activeConnections.keys
            .mapNotNull { peersById[it] }
            .filter { it.deviceId != selfId }
            .minByOrNull { computePeerScore(it, now) }
    }

    private fun computePeerScore(peer: PeerState, now: Long): Double {
        val uptimeSeconds = peer.metrics.uptimeSeconds ?: peer.advertisedUptimeSeconds
        val availability = availabilityFactor(
            peer.advertisedDegree,
            peer.advertisedFreeSlots
        )
        return computeNodeScore(peer.metrics, availability, uptimeSeconds)
            .coerceAtLeast(peer.advertisedScore)
    }

    private fun computeCandidateScore(metadata: MeshAdvertisementData, rssi: Int?): Double {
        val availability = availabilityFactor(metadata.degree, metadata.freeSlotEstimate)
        val metrics = PeerMetrics(
            rssi = rssi ?: metadata.rssiToYou,
            batteryPercent = metadata.batteryPercent,
            uptimeSeconds = metadata.uptimeSeconds,
            loadPercent = metadata.loadPercent
        )
        val computed = computeNodeScore(metrics, availability, metadata.uptimeSeconds)
        return maxOf(computed, if (metadata.nodeScore.isNaN()) 0.0 else metadata.nodeScore)
    }

    private fun evaluateSwapCandidate(
        candidate: MeshAdvertisementData,
        candidateScore: Double,
        worstPeer: PeerState,
        worstScore: Double,
        selfDegree: Int
    ): String? {
        val candidateRssiScore = normalizeRssi(candidate.rssiToYou)
        val worstRssiScore = peerRssiScore(worstPeer)
        val proximityAdvantage = candidateRssiScore + config.bridgeFavor >= worstRssiScore

        if (candidateScore > worstScore + config.scoreHysteresis) {
            return "swap_low_score_peer"
        }
        val peerFreeSlots = worstPeer.advertisedFreeSlots
        val availabilityGain = candidate.freeSlotEstimate - peerFreeSlots
        val candidateHasCapacity = candidate.freeSlotEstimate > 0
        val peerSaturated = peerFreeSlots <= 0
        val scoreWithinBridgeFavor = candidateScore + config.bridgeFavor >= worstScore
        if (scoreWithinBridgeFavor && proximityAdvantage && (availabilityGain > 0 || (candidateHasCapacity && peerSaturated))) {
            return "swap_bridge_capacity"
        }

        val candidateUnderserved = candidate.degree < config.minConnections
        val selfHasSurplus = selfDegree > config.minConnections

        if (candidateUnderserved && selfHasSurplus && proximityAdvantage) {
            return "swap_bridge_capacity"
        }

        val equivalentScore = abs(candidateScore - worstScore) <= config.scoreEquivalenceEpsilon
        if (equivalentScore && selfHasSurplus) {
            return "swap_equivalent_peer"
        }

        return null
    }

    private fun computeNodeScore(metrics: PeerMetrics, availability: Double, uptimeSeconds: Long): Double {
        val weights = normalizedWeights
        val rssiScore = normalizeRssi(metrics.rssi ?: metrics.signalQuality)
        val availabilityScore = availability.coerceIn(0.0, 1.0)
        val uptimeScore = normalizeUptime(uptimeSeconds, config.uptimeSaturationSeconds)
        val batteryScore = (metrics.batteryPercent?.coerceIn(0, 100)?.div(100.0)) ?: 0.6
        val stabilityScore = metrics.stability?.coerceIn(0.0, 1.0) ?: 0.6
        val loadScore = normalizeLoad(metrics.loadPercent)
        return (rssiScore * weights.rssi) +
            (availabilityScore * weights.availability) +
            (uptimeScore * weights.uptime) +
            (batteryScore * weights.battery) +
            (stabilityScore * weights.stability) +
            (loadScore * weights.load)
    }

    private fun normalizeRssi(rssi: Int?): Double {
        val value = rssi ?: return 0.5
        val clamped = value.coerceIn(-100, -20)
        return ((clamped + 100) / 80.0).coerceIn(0.0, 1.0)
    }

    private fun normalizeUptime(uptimeSeconds: Long, saturation: Long): Double {
        if (uptimeSeconds <= 0) return 0.0
        val saturated = saturation.coerceAtLeast(1)
        return (uptimeSeconds.coerceAtMost(saturated).toDouble() / saturated.toDouble()).coerceIn(0.0, 1.0)
    }

    private fun normalizeLoad(loadPercent: Int?): Double {
        return 1.0 - (loadPercent?.coerceIn(0, 100)?.toDouble() ?: 50.0) / 100.0
    }

    private fun availabilityFactor(degree: Int, freeSlots: Int): Double {
        val max = config.maxConnections.coerceAtLeast(1)
        val normalizedDegree = degree.coerceIn(0, max).toDouble() / max.toDouble()
        val normalizedFree = freeSlots.coerceIn(0, max).toDouble() / max.toDouble()
        return ((1.0 - normalizedDegree) + normalizedFree) / 2.0
    }

    private fun pruneExpiredCandidates(now: Long) {
        val iterator = candidatesByHash.entries.iterator()
        while (iterator.hasNext()) {
            val entry = iterator.next()
            if (now - entry.value.observedAt > config.metadataTtlMs) {
                iterator.remove()
            }
        }
    }

    private fun peerRssiScore(peer: PeerState): Double {
        val rssi = peer.metrics.rssi ?: peer.observedRssi ?: peer.advertisedRssiToUs
        return normalizeRssi(rssi)
    }

    companion object {
        private const val DEFAULT_REMOTE_SCORE = 0.55

        fun hash64(input: String): Long {
            val digest = MessageDigest.getInstance("SHA-256")
            val bytes = digest.digest(input.toByteArray())
            var hash = 0L
            for (i in 0 until 8) {
                hash = (hash shl 8) or (bytes[i].toLong() and 0xFF)
            }
            return hash
        }
    }
}


