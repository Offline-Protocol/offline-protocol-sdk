//
// PeripheralRestorationAgeOutPolicy.swift
// OfflineProtocol
//
// Gates the `CBCentralManager willRestoreState` reconnect fan-out on a
// persisted per-peripheral last-seen timestamp.
//
// iOS remembers every peripheral the app has ever requested a connect on for
// as long as the restore identifier is stable — the OS keeps trying to
// service that connect request across process relaunches. The only clear the
// app itself controls is `cancelPeripheralConnection` on the same peripheral
// instance the system hands back at restore time; short of that it takes an
// app uninstall.
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
//   - Every time the app actually observes a peripheral (advertisement seen,
//     GATT connect completed, or a characteristic value arriving on a live
//     link), record its UUID -> wall-clock timestamp in a small persistent
//     store.
//   - On `willRestoreState`, partition the peripherals iOS hands back into
//     "fresh" (worth re-issuing connect on and rediscovering services for) and
//     "stale" (cancel the connect request and drop the persisted record).
//
// A peripheral iOS hands back in `.connected` state is ALWAYS fresh, whatever
// the persisted map says. This is the case state restoration exists for: iOS
// relaunches the app precisely because a live link produced an event, and the
// restored list carries connected peripherals alongside pending ones. The
// timestamp cannot be trusted to prove that link is alive, because nothing
// refreshes it while the app is suspended and background discovery is
// throttled — but `.connected` proves it directly, at the instant of the
// decision. Ageing a live link out would cancel the very link that woke the
// process, a worse failure than the one this class exists to fix. The age-out
// therefore applies only to pending connects, where the connect request is a
// standing OS-side intention with no evidence behind it.
//
// The TTL is measured against the NEWEST sighting in the map, not against the
// restoration clock. Nothing writes the map while the app is dead, and iOS
// relaunches the app hours after it was terminated, so a `now - ttl` cutoff
// would mark every pending connect stale at every restoration and the age-out
// would stop being an age-out: it would be an unconditional cancel of every
// pending connect. That matters because a pending connect is the only way iOS
// wakes this app when a known peer reappears — a background scan with
// `withServices: nil` is ignored by the OS — so cancelling them all trades a
// visible bug for an invisible one.
//
// Anchoring to the newest record asks the answerable question instead: "was
// this peripheral seen within a minute of the last thing this app saw before
// it died?" The dev-loop case the class exists for still ages out, because
// every `node relay.js` restart mints a UUID that gets discovered and becomes
// the new anchor, leaving the previous ones behind the cutoff. What survives
// is at most the handful of peripherals seen in the final minute of the last
// process, which is exactly the set worth keeping a connect request for.
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
// CoreBluetooth delegate queue (BleManager's `nil` queue -> main).
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

/// One peripheral iOS handed back through `willRestoreState`, paired with the
/// state it was handed back in. `isConnected` is the decisive input for a live
/// link: see the file header for why a connected peripheral never ages out.
struct PeripheralRestorationCandidate: Equatable {
    let uuid: UUID
    let isConnected: Bool
}

/// Result of `partitionRestored`. `fresh` UUIDs are the ones the caller should
/// keep and reconnect; `stale` UUIDs are the ones the caller should cancel
/// the pending connect on and drop from any in-memory registries.
struct PeripheralRestorationPartition: Equatable {
    let fresh: [UUID]
    let stale: [UUID]
}

final class PeripheralRestorationAgeOutPolicy {
    /// The observation is considered fresh while `anchor - lastSeen < ttl`,
    /// where the anchor is the newest sighting in the map (see the file
    /// header — it is NOT the restoration clock). At or past the boundary the
    /// entry ages out: a peripheral we hadn't heard from for a full minute
    /// while we were still awake and watching is almost certainly gone, and
    /// the cost of a false age-out (one wasted rediscovery next time it
    /// advertises) is trivial next to the cost of a false-fresh (indefinite
    /// connect requests to a dead UUID). It bounds only pending connects; a
    /// live link is never judged on the clock.
    static let defaultTtlSeconds: TimeInterval = 60

    /// Cap on how many entries the persisted map is allowed to grow to. Real
    /// deployments see well under twenty peers in a session; the cap defends
    /// against a pathological dev loop that cycles thousands of peripheral
    /// UUIDs and wants to keep the blob in UserDefaults small either way.
    /// When the cap is exceeded, the oldest entries are evicted.
    static let defaultMaxRecords = 200

    /// Floor on how often an already-known peripheral's timestamp is flushed
    /// to the store. `recordSeen` runs from the scan callback, which fires on
    /// the CoreBluetooth delegate queue (main) roughly once a second per
    /// visible peer, and each flush serialises the whole map through
    /// `UserDefaults`. Throttling costs at most this much staleness in the
    /// persisted value, an order of magnitude inside the TTL that reads it,
    /// and the case where a stale read would actually hurt — a live link — no
    /// longer consults the clock at all. A first sighting, an eviction and a
    /// restoration all flush immediately regardless.
    static let defaultPersistIntervalSeconds: TimeInterval = 10

    private let store: PeripheralRestorationStore
    private let ttlSeconds: TimeInterval
    private let maxRecords: Int
    private let persistIntervalSeconds: TimeInterval

    private var records: [String: TimeInterval]
    private var lastPersistAtSeconds: TimeInterval?

    init(store: PeripheralRestorationStore,
         ttlSeconds: TimeInterval = PeripheralRestorationAgeOutPolicy.defaultTtlSeconds,
         maxRecords: Int = PeripheralRestorationAgeOutPolicy.defaultMaxRecords,
         persistIntervalSeconds: TimeInterval = PeripheralRestorationAgeOutPolicy.defaultPersistIntervalSeconds) {
        self.store = store
        self.ttlSeconds = ttlSeconds
        self.maxRecords = maxRecords
        self.persistIntervalSeconds = persistIntervalSeconds
        self.records = store.loadRestorationRecords()
    }

    /// Records a live observation of a peripheral — an advertisement seen in
    /// the scan callback, a successful GATT connection, or a characteristic
    /// value arriving on an established link. Idempotent: the latest timestamp
    /// always wins. The in-memory map is always current; the flush to the
    /// store is throttled, see `defaultPersistIntervalSeconds`.
    func recordSeen(uuid: UUID, at now: Date) {
        let key = uuid.uuidString
        let nowSeconds = now.timeIntervalSince1970
        let isFirstSighting = records[key] == nil
        records[key] = nowSeconds
        let evicted = evictIfOverCapacity()
        persist(nowSeconds: nowSeconds, force: isFirstSighting || evicted)
    }

    /// Partitions the peripherals iOS hands to `willRestoreState` into ones
    /// still worth reconnecting to and ones the caller should cancel.
    ///
    /// A candidate handed back in `.connected` state is fresh unconditionally
    /// and has its record refreshed: the link is alive at the instant of the
    /// call, which is stronger evidence than any timestamp. Everything else is
    /// a pending connect request, judged on the persisted last-seen map
    /// against the newest sighting in that map rather than against `now` (see
    /// the file header), and a candidate with no record at all is stale (this
    /// SDK never observed it, so the OS-side restore is the only evidence it
    /// exists, and that evidence is what's untrusted).
    ///
    /// A UUID repeated in `candidates` is decided once, on its first
    /// appearance, so neither output list can contain it twice.
    ///
    /// As a side effect, stale entries are dropped from the persistent store —
    /// a peripheral the caller is telling us to forget stays forgotten.
    func partitionRestored(candidates: [PeripheralRestorationCandidate], now: Date) -> PeripheralRestorationPartition {
        let nowSeconds = now.timeIntervalSince1970
        // Read the anchor BEFORE the loop. The connected branch below writes
        // `nowSeconds` into `records`, and anchoring to a map that already
        // holds it would restore the `now - ttl` cutoff this exists to avoid,
        // silently, on exactly the restorations that carry a live link.
        // `min` guards the other direction: a record written before a
        // backwards clock step sits in the future, and anchoring to it would
        // age out entries that are current.
        let anchorSeconds = min(nowSeconds, records.values.max() ?? nowSeconds)
        let cutoff = anchorSeconds - ttlSeconds
        var fresh: [UUID] = []
        var stale: [UUID] = []
        fresh.reserveCapacity(candidates.count)
        stale.reserveCapacity(candidates.count)

        var seenKeys = Set<String>()
        for candidate in candidates {
            let key = candidate.uuid.uuidString
            guard seenKeys.insert(key).inserted else { continue }
            if candidate.isConnected {
                // A live GATT link. Restoration exists to hand these back, and
                // the app was very likely relaunched because this link
                // produced an event. Refresh the record so the next
                // restoration still reads it as fresh even if the link has
                // dropped by then.
                records[key] = nowSeconds
                fresh.append(candidate.uuid)
            } else if let seenAt = records[key], seenAt > cutoff {
                fresh.append(candidate.uuid)
            } else {
                stale.append(candidate.uuid)
                records.removeValue(forKey: key)
            }
        }

        // Also age out anything else in the persistent store that has crossed
        // the TTL — otherwise a peripheral that iOS stops handing back at
        // restore time never has its record pruned. Iterating the whole map on
        // every restoration is cheap (bounded by `maxRecords`), and
        // restoration is a rare event.
        for (key, seenAt) in records where !seenKeys.contains(key) && seenAt <= cutoff {
            records.removeValue(forKey: key)
        }

        evictIfOverCapacity()
        persist(nowSeconds: nowSeconds, force: true)
        return PeripheralRestorationPartition(fresh: fresh, stale: stale)
    }

    /// Returns the current record count — used by tests to pin the eviction
    /// behavior, and by BleManager for a diagnostic breadcrumb.
    func recordedPeripheralCount() -> Int {
        return records.count
    }

    /// Writes the map through to the store, unless a write happened less than
    /// `persistIntervalSeconds` ago and the caller did not force one. A clock
    /// that steps backwards forces the write rather than blocking it: the
    /// throttle must never be the reason a record goes unpersisted
    /// indefinitely.
    private func persist(nowSeconds: TimeInterval, force: Bool) {
        if !force,
           let last = lastPersistAtSeconds,
           nowSeconds >= last,
           nowSeconds - last < persistIntervalSeconds {
            return
        }
        lastPersistAtSeconds = nowSeconds
        store.saveRestorationRecords(records)
    }

    /// Drops the oldest entries until the map is back under the cap. Returns
    /// whether anything was evicted, so the caller can force a flush: an
    /// eviction that stayed in memory would be undone by the next launch's
    /// load.
    @discardableResult
    private func evictIfOverCapacity() -> Bool {
        guard records.count > maxRecords else { return false }
        // Sort by ascending timestamp (oldest first) and drop the overflow.
        let sorted = records.sorted { $0.value < $1.value }
        let overflow = records.count - maxRecords
        for (key, _) in sorted.prefix(overflow) {
            records.removeValue(forKey: key)
        }
        return true
    }
}
