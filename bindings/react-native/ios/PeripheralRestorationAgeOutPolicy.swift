//
// PeripheralRestorationAgeOutPolicy.swift
// OfflineProtocol
//
// Gates the `CBCentralManager willRestoreState` reconnect fan-out on a
// persisted per-peripheral last-seen timestamp.
//
// iOS remembers every peripheral the app has ever requested a connect on for
// as long as the restore identifier is stable — the OS keeps trying to
// service that connect request across process relaunches, and only two things
// clear it: `cancelPeripheralConnection` on the same peripheral instance the
// system hands back at restore time, or a full app uninstall.
//
// In development that "connect target" list is very often full of dead
// peripheral UUIDs: every `node relay.js` restart yields a fresh
// `CBPeripheralManager` UUID, so the SDK ends up asking the OS to reach
// peripherals whose owners no longer exist. Blindly re-issuing `connect(...)`
// on the restored list keeps the OS burning battery chasing those UUIDs, and
// the visible symptom is a phone that will not discover a legitimate new
// peer until the app is reinstalled.
//
// The policy this class captures:
//
//   - Every time the app actually observes a peripheral (advertisement seen or
//     GATT connect completed), record its UUID → wall-clock timestamp in a
//     small persistent store.
//   - On `willRestoreState`, partition the peripherals iOS hands back into
//     "fresh" (last observation within the TTL — worth re-issuing connect on
//     and rediscovering services for) and "stale" (never observed by this
//     process, or observed too long ago — cancel the connect request and drop
//     the persisted record).
//
// Wall-clock is deliberate: state restoration crosses process boundaries, so a
// monotonic clock would reset to zero at every relaunch and every entry would
// read as "just seen." Wall-clock time is the only reading that survives the
// termination the class is designed to compensate for. A user manually
// stepping their clock backwards is not a concern here — a false-fresh reading
// costs one wasted reconnect attempt, and a false-stale reading costs one
// wasted rediscovery cycle. Neither leaks state or corrupts anything.
//
// Extracted as a standalone, unit-testable class (the same shape as the other
// policy helpers in this directory) so the age-out math is pinned by
// `swift test` without a running CoreBluetooth stack, and so the persistence
// layer is dependency-injected: production wires
// `UserDefaultsPeripheralRestorationStore`, tests inject an in-memory dict.
//
// Not internally synchronized: the caller confines every method call to the
// CoreBluetooth delegate queue (BleManager's `nil` queue → main).
//

import Foundation

/// Persistent key/value store for the peripheral last-seen map. The blob is a
/// dictionary keyed by peripheral UUID string with values that are seconds
/// since 1970 (matching `Date().timeIntervalSince1970`). The store returns an
/// empty dictionary when nothing has ever been persisted.
protocol PeripheralRestorationStore {
    func loadRestorationRecords() -> [String: TimeInterval]
    func saveRestorationRecords(_ records: [String: TimeInterval])
}

/// The default production store: a namespaced key in `UserDefaults.standard`.
/// Values persist across app relaunches, which is the whole point of the
/// policy — see the file header for why wall-clock time is the right clock
/// here.
final class UserDefaultsPeripheralRestorationStore: PeripheralRestorationStore {
    /// Namespaced under `mesh.blemanager.*` so a UserDefaults dump from
    /// support tools reads unambiguously. The `.v1` suffix reserves room for
    /// a future schema bump; a v2 rollout should read v1 then delete it.
    static let defaultKey = "mesh.blemanager.peripheralLastSeen.v1"

    private let userDefaults: UserDefaults
    private let key: String

    init(userDefaults: UserDefaults = .standard,
         key: String = UserDefaultsPeripheralRestorationStore.defaultKey) {
        self.userDefaults = userDefaults
        self.key = key
    }

    func loadRestorationRecords() -> [String: TimeInterval] {
        guard let raw = userDefaults.dictionary(forKey: key) else { return [:] }
        var records: [String: TimeInterval] = [:]
        records.reserveCapacity(raw.count)
        for (uuid, value) in raw {
            if let seconds = value as? TimeInterval {
                records[uuid] = seconds
            } else if let seconds = (value as? NSNumber)?.doubleValue {
                records[uuid] = seconds
            }
        }
        return records
    }

    func saveRestorationRecords(_ records: [String: TimeInterval]) {
        if records.isEmpty {
            userDefaults.removeObject(forKey: key)
        } else {
            userDefaults.set(records, forKey: key)
        }
    }
}

/// Result of `partitionRestored`. `fresh` UUIDs are the ones the caller should
/// keep and reconnect; `stale` UUIDs are the ones the caller should cancel
/// the pending connect on and drop from any in-memory registries.
struct PeripheralRestorationPartition: Equatable {
    let fresh: [UUID]
    let stale: [UUID]
}

final class PeripheralRestorationAgeOutPolicy {
    /// The observation is considered fresh while `now - lastSeen < ttl`. At or
    /// past the TTL boundary the entry ages out — a peripheral we haven't
    /// heard from in a full minute is almost certainly gone by now on a
    /// consumer BLE stack, and the cost of a false age-out (one wasted
    /// rediscovery next time it advertises) is trivial next to the cost of a
    /// false-fresh (indefinite connect requests to a dead UUID).
    static let defaultTtlSeconds: TimeInterval = 60

    /// Cap on how many entries the persisted map is allowed to grow to. Real
    /// deployments see well under twenty peers in a session; the cap defends
    /// against a pathological dev loop that cycles thousands of peripheral
    /// UUIDs and wants to keep the blob in UserDefaults small either way.
    /// When the cap is exceeded, the oldest entries are evicted on the next
    /// `recordSeen`.
    static let defaultMaxRecords = 200

    private let store: PeripheralRestorationStore
    private let ttlSeconds: TimeInterval
    private let maxRecords: Int

    private var records: [String: TimeInterval]

    init(store: PeripheralRestorationStore,
         ttlSeconds: TimeInterval = PeripheralRestorationAgeOutPolicy.defaultTtlSeconds,
         maxRecords: Int = PeripheralRestorationAgeOutPolicy.defaultMaxRecords) {
        self.store = store
        self.ttlSeconds = ttlSeconds
        self.maxRecords = maxRecords
        self.records = store.loadRestorationRecords()
    }

    /// Records a live observation of a peripheral — an advertisement seen in
    /// the scan callback, or a successful GATT connection. Idempotent: the
    /// latest timestamp always wins.
    func recordSeen(uuid: UUID, at now: Date) {
        let key = uuid.uuidString
        records[key] = now.timeIntervalSince1970
        evictIfOverCapacity(now: now)
        store.saveRestorationRecords(records)
    }

    /// Partitions the peripherals iOS hands to `willRestoreState` into ones
    /// still worth reconnecting to and ones the caller should cancel. As a
    /// side effect, stale entries are dropped from the persistent store —
    /// a peripheral the caller is telling us to forget stays forgotten.
    /// Peripherals with no persisted record are treated as stale (the SDK
    /// never observed them from this process, so the OS-side restore is our
    /// only evidence they exist, and that evidence is what's untrusted).
    func partitionRestored(candidates: [UUID], now: Date) -> PeripheralRestorationPartition {
        let cutoff = now.timeIntervalSince1970 - ttlSeconds
        var fresh: [UUID] = []
        var stale: [UUID] = []
        fresh.reserveCapacity(candidates.count)
        stale.reserveCapacity(candidates.count)

        var seenKeys = Set<String>()
        for uuid in candidates {
            let key = uuid.uuidString
            seenKeys.insert(key)
            if let seenAt = records[key], seenAt > cutoff {
                fresh.append(uuid)
            } else {
                stale.append(uuid)
                records.removeValue(forKey: key)
            }
        }

        // Also age out anything else in the persistent store that has crossed
        // the TTL — otherwise a peripheral that iOS stops handing back at
        // restore time (successful cancel, or reboot) never has its record
        // pruned. Iterating the whole map on every restoration is cheap
        // (bounded by `maxRecords`), and restoration is a rare event.
        for (key, seenAt) in records where !seenKeys.contains(key) && seenAt <= cutoff {
            records.removeValue(forKey: key)
        }

        store.saveRestorationRecords(records)
        return PeripheralRestorationPartition(fresh: fresh, stale: stale)
    }

    /// Returns the current record count — used by tests to pin the eviction
    /// behavior, and by BleManager for a diagnostic breadcrumb.
    func recordedPeripheralCount() -> Int {
        return records.count
    }

    private func evictIfOverCapacity(now: Date) {
        guard records.count > maxRecords else { return }
        // Sort by ascending timestamp (oldest first) and drop the overflow.
        let sorted = records.sorted { $0.value < $1.value }
        let overflow = records.count - maxRecords
        for (key, _) in sorted.prefix(overflow) {
            records.removeValue(forKey: key)
        }
    }
}
