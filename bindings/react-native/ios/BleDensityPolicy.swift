import Foundation

/// How crowded the mesh around this device is, and which mesh peers a scan
/// passes over while it is crowded. Mirrors android/.../BleDensityPolicy.kt;
/// keep in sync.
///
/// The count is of distinct mesh candidates, the peripherals the discovery
/// gate admits, not of discovery callbacks from every Bluetooth device in
/// range. Counted that way, a room with a television, earbuds and a watch
/// read as a dense mesh, and two phones a metre apart passed each other over.
///
/// The pass-over is keyed on the peripheral and a rotating slot: steady within
/// a slot, so a crowded scan does not churn, and reconsidered in the next.
/// Keyed on `peripheral.hashValue` alone, which Swift seeds once per process,
/// the same peers were passed over for the life of the process.
internal enum BleDensityPolicy {
    /// How long one pass-over decision holds for a peripheral.
    static let skipSlot: TimeInterval = 60

    /// The share of candidates passed over at the high threshold and above.
    static let maxSkipShare = 0.8

    /// Records `id` as seen at `now` and returns how many distinct candidates
    /// `seen` holds from the last `window`, dropping older ones.
    static func recordAndCount(_ seen: inout [String: Date], id: String, now: Date, window: TimeInterval) -> Int {
        // Pruned in place, as on Android: this runs on every discovery.
        seen[id] = now
        for (key, lastSeen) in seen where now.timeIntervalSince(lastSeen) > window {
            seen.removeValue(forKey: key)
        }
        return seen.count
    }

    /// The share of candidates to pass over with `meshPeerCount` mesh peers in range.
    static func skipShare(meshPeerCount: Int, lowThreshold: Int, highThreshold: Int) -> Double {
        guard meshPeerCount > lowThreshold else { return 0 }
        let density = Double(meshPeerCount - lowThreshold)
        let range = Double(highThreshold - lowThreshold)
        return min(maxSkipShare, density / range * maxSkipShare)
    }

    /// Whether to pass over the candidate `id` in the slot holding `now`.
    static func shouldSkip(id: String, meshPeerCount: Int, now: Date, lowThreshold: Int, highThreshold: Int) -> Bool {
        let share = skipShare(meshPeerCount: meshPeerCount, lowThreshold: lowThreshold, highThreshold: highThreshold)
        guard share > 0 else { return false }
        let slot = Int64((now.timeIntervalSince1970 * 1000).rounded(.down)) / Int64(skipSlot * 1000)
        return bucket(id: id, slot: slot) < share
    }

    /// Where `id` falls in [0, 1) for `slot`, the same as on Android: Java's
    /// String.hashCode over the UTF-16 units, folded with the slot and mixed
    /// by the murmur3 finalizer. Unmixed, consecutive slots of one id landed
    /// in neighbouring buckets, so a peer passed over once mostly stayed so.
    static func bucket(id: String, slot: Int64) -> Double {
        var stringHash: UInt32 = 0
        for unit in id.utf16 {
            stringHash = 31 &* stringHash &+ UInt32(unit)
        }
        var h = stringHash ^ (UInt32(truncatingIfNeeded: slot) &* 0x9E37_79B9)
        h ^= h >> 16
        h = h &* 0x85EB_CA6B
        h ^= h >> 13
        h = h &* 0xC2B2_AE35
        h ^= h >> 16
        return Double(h % 1000) / 1000
    }
}
