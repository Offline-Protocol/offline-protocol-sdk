package com.offlineprotocol

/**
 * How crowded the mesh around this device is, and which mesh peers a scan
 * passes over while it is crowded. Mirrors ios/BleDensityPolicy.swift.
 *
 * The count is of distinct mesh candidates, the devices the discovery gate
 * admits, not of scan callbacks from every Bluetooth device in range. Counted
 * that way, a room with a television, earbuds and a watch read as a dense
 * mesh, and two phones a metre apart passed each other over for minutes.
 *
 * The pass-over is keyed on the address and a rotating slot: steady within a
 * slot, so a crowded scan does not churn, and reconsidered in the next. Keyed
 * on the address alone, the same peers were passed over for as long as their
 * address lasted.
 */
internal object BleDensityPolicy {
    /** How long one pass-over decision holds for an address. */
    const val SKIP_SLOT_MS = 60_000L

    /** The share of candidates passed over at the high threshold and above. */
    const val MAX_SKIP_SHARE = 0.8

    /**
     * Records [address] as seen at [now] and returns how many distinct
     * candidates [seen] holds from the last [windowMs], dropping older ones.
     */
    fun recordAndCount(seen: MutableMap<String, Long>, address: String, now: Long, windowMs: Long): Int {
        seen[address] = now
        seen.entries.removeIf { now - it.value > windowMs }
        return seen.size
    }

    /** The share of candidates to pass over with [meshPeerCount] mesh peers in range. */
    fun skipShare(meshPeerCount: Int, lowThreshold: Int, highThreshold: Int): Double {
        if (meshPeerCount <= lowThreshold) return 0.0
        val density = (meshPeerCount - lowThreshold).toDouble()
        val range = (highThreshold - lowThreshold).toDouble()
        return minOf(MAX_SKIP_SHARE, density / range * MAX_SKIP_SHARE)
    }

    /** Whether to pass over the candidate at [address] in the slot holding [now]. */
    fun shouldSkip(
        address: String,
        meshPeerCount: Int,
        now: Long,
        lowThreshold: Int,
        highThreshold: Int,
    ): Boolean {
        val share = skipShare(meshPeerCount, lowThreshold, highThreshold)
        if (share == 0.0) return false
        return bucket(address, now / SKIP_SLOT_MS) < share
    }

    /**
     * Where [address] falls in [0, 1) for [slot], the same on iOS: the
     * address's String.hashCode, folded with the slot and mixed by the
     * murmur3 finalizer. Unmixed, consecutive slots of one address landed in
     * neighbouring buckets, so a peer passed over in one slot mostly stayed
     * passed over.
     */
    internal fun bucket(address: String, slot: Long): Double {
        var h = address.hashCode() xor (slot.toInt() * GOLDEN_GAMMA)
        h = h xor (h ushr 16)
        h *= MIX_1
        h = h xor (h ushr 13)
        h *= MIX_2
        h = h xor (h ushr 16)
        return (Integer.toUnsignedLong(h) % 1000L) / 1000.0
    }

    private const val GOLDEN_GAMMA = 0x9E3779B9.toInt()
    private const val MIX_1 = 0x85EBCA6B.toInt()
    private const val MIX_2 = 0xC2B2AE35.toInt()
}
